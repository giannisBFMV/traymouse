//! TrayMouse: wireless mouse battery on the Windows taskbar (mouse glyph + "71%").
//! Supported mice and how each is read: see mice.rs.
//! Below 25% the mouse and percent turn red; ⚡ + small percent while plugged in; red X when no mouse answers.
//! Floats over the taskbar left of the tray, follows the tray's edge, hides over fullscreen apps.
//! Drag to change the spacing (remembered). Right-click: Refresh / Mouse color / Reset color / Reset position / Quit.
#![windows_subsystem = "windows"]

mod mice;
use mice::{read_battery, Battery};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use std::{cell::RefCell, mem::zeroed, path::PathBuf, ptr::null_mut, time::Duration};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

const COLOR: [f32; 3] = [150.0, 140.0, 255.0]; // default mouse colour: lavender
const WHITE: [f32; 3] = [255.0; 3];
const RED: [f32; 3] = [255.0, 70.0, 70.0];
const LOW_BATTERY: u8 = 25; // mouse turns red below this
const POLL_MS: u32 = 5_000; // short, so switching the mouse off/on shows within seconds
const TEXT: f32 = 14.0; // font size, like tray text
const TEXT_SMALL: f32 = 11.0; // next to the charging bolt
const DEFAULT_GAP: i32 = 2; // px between the widget and the tray arrow
const SNAP: f32 = 24.0; // dropped this close (in 96-dpi px) to the tray, it keeps following the tray

/// Where the widget sits: glued to the tray (follows it as icons come and go) or parked anywhere on the taskbar.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pos {
    Tray(i32), // px between our right edge and the tray's left edge
    Free(i32), // px from the taskbar's left edge to ours
}

impl Pos {
    /// `gap` and `free` describe the same spot; near the tray we follow it, otherwise stay put.
    fn pick(gap: i32, free: i32, snap: i32) -> Pos {
        if gap.abs() <= snap { Pos::Tray(gap) } else { Pos::Free(free) }
    }

    fn save(self) -> String {
        match self {
            Pos::Tray(g) => g.to_string(),
            Pos::Free(x) => format!("free {x}"),
        }
    }

    /// Plain number = tray gap (older versions only wrote that), "free N" = parked.
    fn load(s: &str) -> Option<Pos> {
        let s = s.trim();
        match s.strip_prefix("free ") {
            Some(x) => x.trim().parse().ok().map(Pos::Free),
            None => s.parse().ok().map(Pos::Tray),
        }
    }
}
const TIMER_POLL: usize = 1;
const TIMER_FULLSCREEN: usize = 2;

// ---------- drawing (premultiplied RGBA floats, 0..1) ----------

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn blend(&mut self, x: i32, y: i32, rgb: [f32; 3], cov: f32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h || cov <= 0.0 {
            return;
        }
        let a = cov.min(1.0);
        let p = &mut self.px[y as usize * self.w + x as usize];
        for i in 0..3 {
            p[i] = rgb[i] / 255.0 * a + p[i] * (1.0 - a);
        }
        p[3] = a + p[3] * (1.0 - a);
    }

    /// Premultiplied BGRA bytes, as UpdateLayeredWindow wants.
    fn bgra(&self) -> Vec<u8> {
        let to = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        self.px.iter().flat_map(|p| [to(p[2]), to(p[1]), to(p[0]), to(p[3])]).collect()
    }
}

struct Look {
    color: [f32; 3], // mouse colour, chosen from the right-click menu
    font: FontVec,
    scale: f32, // DPI scale, 1.0 = 96 dpi
}

impl Look {
    /// `em` = font size in px at 96 dpi (PIL/CSS style); ab_glyph's PxScale is ascent-descent height.
    fn px_scale(&self, em: f32) -> PxScale {
        PxScale::from(em * self.scale * self.font.height_unscaled() / self.font.units_per_em().unwrap_or(2048.0))
    }

    fn text_width(&self, text: &str, em: f32) -> f32 {
        let f = self.font.as_scaled(self.px_scale(em));
        text.chars().map(|c| f.h_advance(f.glyph_id(c))).sum()
    }

    /// White text starting at `x`, vertically centred on `cy` (like PIL anchor "lm").
    fn draw_text(&self, c: &mut Canvas, text: &str, mut x: f32, cy: f32, em: f32, color: [f32; 3]) {
        let f = self.font.as_scaled(self.px_scale(em));
        let baseline = cy + (f.ascent() + f.descent()) / 2.0;
        for ch in text.chars() {
            let id = f.glyph_id(ch);
            let glyph = id.with_scale_and_position(f.scale(), ab_glyph::point(x, baseline));
            x += f.h_advance(id);
            if let Some(g) = self.font.outline_glyph(glyph) {
                let b = g.px_bounds();
                g.draw(|gx, gy, cov| c.blend(b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32, color, cov));
            }
        }
    }

    // Same proportions as tray icons: 16px mouse glyph, ~9pt semibold text.
    fn dims(&self) -> (f32, f32, f32, f32, f32) {
        let s = self.scale;
        (10.0 * s, 16.0 * s, 6.0 * s, 4.0 * s, 1.5 * s) // mouse w, mouse h, gap, pad, stroke
    }

    /// Just wide enough for what's shown, so the X or bolt doesn't leave a gap before the tray.
    fn width(&self, battery: Option<Battery>) -> usize {
        let (mw, _, gap, pad, _) = self.dims();
        let content = match battery {
            None => 8.0 * self.scale,                    // red X
            Some(b) if b.charging => {
                let (gap, bw, _) = self.bolt();
                bw + gap + self.text_width(&format!("{}%", b.percent), TEXT_SMALL)
            }
            Some(b) => self.text_width(&format!("{}%", b.percent), TEXT),
        };
        (pad + mw + gap + content + pad).ceil() as usize
    }

    fn bolt(&self) -> (f32, f32, f32) {
        let s = self.scale;
        (2.0 * s, 7.0 * s, 11.0 * s) // gap before, width, height
    }

    /// `battery`: Some(reading), or None when no mouse is answering (red X).
    fn render(&self, battery: Option<Battery>, w: usize, h: usize) -> Canvas {
        // near-invisible background so clicks between glyphs still hit the window
        let mut c = Canvas { w, h, px: vec![[0.0, 0.0, 0.0, 4.0 / 255.0]; w * h] };
        let (mw, mh, gap, pad, sw) = self.dims();
        let (cx, cy) = (pad + mw / 2.0, h as f32 / 2.0);
        let top = cy - mh / 2.0;
        let low = battery.is_some_and(|b| b.percent < LOW_BATTERY);
        // low battery: mouse AND percent turn red, so it still shows if the chosen mouse colour is red
        let mouse = if low { RED } else { self.color };
        let text_color = if low { RED } else { WHITE };
        let x_arm = 4.0 * self.scale; // half-size of the red X
        let x_c = (pad + mw + gap + x_arm, cy);

        // mouse outline: signed distance to a capsule-shaped rounded box, anti-aliased by distance
        let (hw, hh) = (mw / 2.0 - sw / 2.0, mh / 2.0 - sw / 2.0);
        let r = hw;
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let (qx, qy) = (px.abs() - hw + r, py.abs() - hh + r);
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let d = outside + qx.max(qy).min(0.0) - r;
                c.blend(x as i32, y as i32, mouse, 0.5 - (d.abs() - sw / 2.0));
                // scroll wheel: short vertical segment
                let (y0, y1) = (top + 3.0 * self.scale, top + 6.0 * self.scale);
                let dy = (py + cy).clamp(y0, y1) - (py + cy);
                let ds = (px * px + dy * dy).sqrt();
                c.blend(x as i32, y as i32, mouse, 0.5 - (ds - sw / 2.0));
                if battery.is_none() {
                    // red X: distance to the two diagonals, limited to the X's square
                    let (ux, uy) = (x as f32 + 0.5 - x_c.0, y as f32 + 0.5 - x_c.1);
                    let inside = ux.abs().max(uy.abs()) - x_arm;
                    let diag = (ux.abs() - uy.abs()).abs() / std::f32::consts::SQRT_2;
                    let d = diag.max(inside) - 1.0 * self.scale;
                    c.blend(x as i32, y as i32, RED, 0.5 - d);
                }
            }
        }
        let Some(battery) = battery else { return c };
        let text = format!("{}%", battery.percent);
        let x = pad + mw + gap;
        if battery.charging {
            // bolt, then the percent in a smaller size to sit with the bolt
            let (bolt_gap, bw, _) = self.bolt();
            self.draw_bolt(&mut c, x - bolt_gap, cy);
            self.draw_text(&mut c, &text, x + bw + bolt_gap, cy, TEXT_SMALL, text_color);
        } else {
            self.draw_text(&mut c, &text, x, cy, TEXT, text_color);
        }
        c
    }

    /// Lightning bolt starting `gap` after `text_end`; coverage from 4x4 samples per pixel (point-in-polygon).
    fn draw_bolt(&self, c: &mut Canvas, text_end: f32, cy: f32) {
        const SHAPE: [(f32, f32); 6] = [(0.62, 0.0), (0.0, 0.58), (0.45, 0.58), (0.36, 1.0), (1.0, 0.4), (0.55, 0.4)];
        let (gap, bw, bh) = self.bolt();
        let (x0, y0) = (text_end + gap, cy - bh / 2.0);
        let pts: Vec<(f32, f32)> = SHAPE.iter().map(|&(u, v)| (x0 + u * bw, y0 + v * bh)).collect();
        let inside = |x: f32, y: f32| {
            let mut odd = false;
            for i in 0..pts.len() {
                let ((ax, ay), (bx, by)) = (pts[i], pts[(i + 1) % pts.len()]);
                if (ay > y) != (by > y) && x < ax + (y - ay) * (bx - ax) / (by - ay) {
                    odd = !odd;
                }
            }
            odd
        };
        for py in y0.floor() as i32..(y0 + bh).ceil() as i32 {
            for px in x0.floor() as i32..(x0 + bw).ceil() as i32 {
                let hits = (0..16)
                    .filter(|k| inside(px as f32 + (k % 4) as f32 / 4.0 + 0.125, py as f32 + (k / 4) as f32 / 4.0 + 0.125))
                    .count();
                c.blend(px, py, [255.0; 3], hits as f32 / 16.0);
            }
        }
    }
}

// ---------- window ----------

struct State {
    hwnd: HWND,
    tray: HWND,
    look: Look,
    notify: HWND,
    pos: Pos,
    top: i32,
    w: usize,
    h: usize,
    last: Option<Battery>, // None = no mouse answering
    drag_dx: Option<i32>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// %APPDATA%\TrayMouse: survives updates (npx unpacks each version into a new cache folder).
fn settings_dir() -> PathBuf {
    let dir = std::env::var_os("APPDATA").map_or_else(
        || std::env::current_exe().unwrap_or_default().with_file_name(""),
        |d| PathBuf::from(d).join("TrayMouse"),
    );
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn pos_file() -> PathBuf {
    settings_dir().join("gap.txt") // Pos::save(); the name predates free positions
}

fn rect(hwnd: HWND) -> RECT {
    let mut r: RECT = unsafe { zeroed() };
    unsafe { GetWindowRect(hwnd, &mut r) };
    r
}

impl State {
    /// Near the tray it follows the tray's left edge (shifts when tray icons come and go); elsewhere it stays put.
    fn x(&self) -> i32 {
        match self.pos {
            Pos::Tray(g) => rect(self.notify).left - g - self.w as i32,
            Pos::Free(x) => rect(self.tray).left + x,
        }
    }

    fn snap(&self) -> i32 {
        (SNAP * self.look.scale) as i32
    }

    fn draw(&mut self) {
        self.w = self.look.width(self.last);
        let bgra = self.look.render(self.last, self.w, self.h).bgra();
        let x = self.x();
        unsafe {
            let screen = GetDC(null_mut());
            let mem = CreateCompatibleDC(screen);
            let mut bmi: BITMAPINFO = zeroed();
            bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = self.w as i32;
            bmi.bmiHeader.biHeight = -(self.h as i32); // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            let mut bits = null_mut();
            let bmp = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if !bmp.is_null() {
                std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
                let old = SelectObject(mem, bmp);
                let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: 1 };
                let (dst, src) = (POINT { x, y: self.top }, POINT { x: 0, y: 0 });
                let size = SIZE { cx: self.w as i32, cy: self.h as i32 };
                UpdateLayeredWindow(self.hwnd, screen, &dst, &size, mem, &src, 0, &blend, ULW_ALPHA);
                SelectObject(mem, old);
                DeleteObject(bmp);
            }
            DeleteDC(mem);
            ReleaseDC(null_mut(), screen);
        }
    }

    fn refresh(&mut self) {
        // one retry so a single busy moment doesn't flash the X
        self.last = read_battery().or_else(|| {
            std::thread::sleep(Duration::from_millis(300));
            read_battery()
        });
        self.draw();
    }

    fn move_to(&mut self, x: i32) {
        let gap = rect(self.notify).left - x - self.w as i32;
        self.pos = Pos::pick(gap, x - rect(self.tray).left, self.snap());
        unsafe {
            SetWindowPos(self.hwnd, null_mut(), x, self.top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)
        };
    }
}

/// True when the foreground window covers its whole monitor (video, game), so we should hide.
fn fullscreen_app_in_front(tray: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() || fg == tray {
            return false;
        }
        let mut cls = [0u16; 32];
        let n = GetClassNameW(fg, cls.as_mut_ptr(), 32) as usize;
        let cls = String::from_utf16_lossy(&cls[..n]);
        if cls == "Progman" || cls == "WorkerW" {
            return false; // the desktop
        }
        let win = rect(fg);
        let mut mi: MONITORINFO = zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST), &mut mi);
        let m = mi.rcMonitor;
        win.left <= m.left && win.top <= m.top && win.right >= m.right && win.bottom >= m.bottom
    }
}

const MENU_REFRESH: usize = 1;
const MENU_RESET: usize = 2;
const MENU_QUIT: usize = 3;
const MENU_COLOR: usize = 4;
const MENU_COLOR_RESET: usize = 5;

fn color_file() -> PathBuf {
    settings_dir().join("color.txt")
}

/// "rrggbb" -> colour
fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let v = u32::from_str_radix(s.trim().trim_start_matches('#'), 16).ok().filter(|_| s.trim().trim_start_matches('#').len() == 6)?;
    Some([(v >> 16) as f32, (v >> 8 & 0xFF) as f32, (v & 0xFF) as f32])
}

/// Windows' colour picker (swatches + custom colours). None if cancelled.
fn pick_color(owner: HWND, current: [f32; 3]) -> Option<[f32; 3]> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut custom = [0u32; 16];
    let mut cc: CHOOSECOLORW = unsafe { zeroed() };
    cc.lStructSize = size_of::<CHOOSECOLORW>() as u32;
    cc.hwndOwner = owner;
    cc.rgbResult = current[0] as u32 | (current[1] as u32) << 8 | (current[2] as u32) << 16; // COLORREF is 0x00BBGGRR
    cc.lpCustColors = custom.as_mut_ptr();
    cc.Flags = CC_RGBINIT | CC_FULLOPEN | CC_ANYCOLOR;
    (unsafe { ChooseColorW(&mut cc) } != 0).then(|| {
        let c = cc.rgbResult;
        [(c & 0xFF) as f32, (c >> 8 & 0xFF) as f32, (c >> 16 & 0xFF) as f32]
    })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let cursor_x = || {
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        p.x
    };
    match msg {
        WM_TIMER if wp == TIMER_POLL => {
            with(|s| s.refresh());
        }
        WM_TIMER if wp == TIMER_FULLSCREEN => {
            with(|s| {
                if fullscreen_app_in_front(s.tray) {
                    ShowWindow(hwnd, SW_HIDE);
                } else {
                    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    if s.drag_dx.is_none() && rect(hwnd).left != s.x() {
                        s.move_to(s.x()); // tray width changed
                    }
                    // clicking the taskbar raises it above us; push back on top
                    SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            });
        }
        WM_LBUTTONDOWN => {
            let cx = cursor_x();
            with(|s| s.drag_dx = Some(cx - s.x()));
            SetCapture(hwnd);
        }
        WM_MOUSEMOVE => {
            let cx = cursor_x();
            with(|s| {
                if let Some(dx) = s.drag_dx {
                    s.move_to(cx - dx);
                }
            });
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            with(|s| {
                if s.drag_dx.take().is_some() {
                    let _ = std::fs::write(pos_file(), s.pos.save());
                }
            });
        }
        WM_RBUTTONUP => {
            let menu = CreatePopupMenu();
            for (id, label) in [
                (MENU_REFRESH, "Refresh now"),
                (MENU_COLOR, "Mouse color…"),
                (MENU_COLOR_RESET, "Reset color"),
                (MENU_RESET, "Reset position"),
                (0, ""),
                (MENU_QUIT, "Quit"),
            ] {
                let flags = if id == 0 { MF_SEPARATOR } else { MF_STRING };
                AppendMenuW(menu, flags, id, wide(label).as_ptr());
            }
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            SetForegroundWindow(hwnd); // required so the menu closes when clicking elsewhere
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY, p.x, p.y, 0, hwnd, null_mut());
            DestroyMenu(menu);
            match cmd as usize {
                MENU_REFRESH => {
                    with(|s| s.refresh());
                }
                MENU_RESET => {
                    let _ = std::fs::remove_file(pos_file());
                    with(|s| {
                        s.pos = Pos::Tray(DEFAULT_GAP);
                        s.move_to(s.x());
                    });
                }
                MENU_COLOR => {
                    let current = with(|s| s.look.color).unwrap_or(COLOR);
                    if let Some(c) = pick_color(hwnd, current) {
                        let hex = format!("{:02x}{:02x}{:02x}", c[0] as u8, c[1] as u8, c[2] as u8);
                        let _ = std::fs::write(color_file(), hex);
                        with(|s| {
                            s.look.color = c;
                            s.draw();
                        });
                    }
                }
                MENU_COLOR_RESET => {
                    let _ = std::fs::remove_file(color_file());
                    with(|s| {
                        s.look.color = COLOR;
                        s.draw();
                    });
                }
                MENU_QUIT => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
        }
        WM_DESTROY => PostQuitMessage(0),
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}

fn main() {
    unsafe {
        // one widget only: a second launch (e.g. running `npx traymouse` again) just exits
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let _guard = CreateMutexW(null_mut(), 0, wide(r"Local\TrayMouseSingleInstance").as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2); // real pixels
        let tray = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null_mut());
        let notify = FindWindowExW(tray, null_mut(), wide("TrayNotifyWnd").as_ptr(), null_mut());
        let bar = rect(tray);

        let font_path = r"C:\Windows\Fonts\seguisb.ttf"; // Segoe UI Semibold, like tray text
        let Ok(font) = std::fs::read(font_path).map_err(drop).and_then(|b| FontVec::try_from_vec(b).map_err(drop))
        else {
            return;
        };
        let color = std::fs::read_to_string(color_file()).ok().and_then(|s| parse_hex(&s)).unwrap_or(COLOR);
        let look = Look { color, font, scale: GetDpiForSystem() as f32 / 96.0 };
        let (w, h) = (look.width(None), (bar.bottom - bar.top) as usize);

        let class = wide("TrayMouseBattery");
        let inst = GetModuleHandleW(null_mut());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: inst,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_HAND),
            ..zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class.as_ptr(),
            wide("TrayMouse").as_ptr(),
            WS_POPUP,
            0, 0, w as i32, h as i32,
            null_mut(), null_mut(), inst, null_mut(),
        );

        let saved = std::fs::read_to_string(pos_file()).ok().and_then(|s| Pos::load(&s)).unwrap_or(Pos::Tray(DEFAULT_GAP));
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                hwnd, tray, look, notify, pos: saved, top: bar.top, w, h,
                last: None, drag_dx: None,
            })
        });
        with(|s| {
            // older versions saved every spot as a tray gap; a far one is really a parked position
            if let Pos::Tray(g) = s.pos {
                if g.abs() > s.snap() {
                    s.pos = Pos::Free(s.x() - rect(s.tray).left);
                }
            }
            s.refresh()
        });
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(hwnd, TIMER_POLL, POLL_MS, None);
        SetTimer(hwnd, TIMER_FULLSCREEN, 500, None);

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_snaps_to_tray_only_when_close() {
        assert_eq!(Pos::pick(2, 1500, 24), Pos::Tray(2));
        assert_eq!(Pos::pick(-10, 1500, 24), Pos::Tray(-10));
        assert_eq!(Pos::pick(900, 60, 24), Pos::Free(60));
        for p in [Pos::Tray(2), Pos::Tray(-5), Pos::Free(60)] {
            assert_eq!(Pos::load(&p.save()), Some(p));
        }
        assert_eq!(Pos::load("37
"), Some(Pos::Tray(37))); // file from older versions
        assert_eq!(Pos::load("junk"), None);
    }

    #[test]
    #[ignore = "needs a supported mouse connected: cargo test -- --ignored"]
    fn reads_real_mouse() {
        let b = read_battery().expect("no supported mouse answered");
        assert!((1..=100).contains(&b.percent), "got {b:?}");
        println!("battery: {b:?}");
    }

    #[test]
    #[ignore = "writes preview.ppm (normal, low, disconnected) on a dark background"]
    fn preview() {
        let font = FontVec::try_from_vec(std::fs::read(r"C:\Windows\Fonts\seguisb.ttf").unwrap()).unwrap();
        let look = Look { color: COLOR, font, scale: 1.0 };
        let (w, h) = (look.width(Some(Battery { percent: 100, charging: false })), 46);
        let b = |percent, charging| Some(Battery { percent, charging });
        let shots: Vec<Canvas> = [b(70, false), b(20, false), b(100, true), b(15, true), None].map(|b| look.render(b, w, h)).into();
        let mut out = format!("P6 {} {h} 255\n", w * shots.len()).into_bytes();
        for y in 0..h {
            for c in &shots {
                for p in &c.px[y * w..(y + 1) * w] {
                    let bg = 28.0 / 255.0 * (1.0 - p[3]); // premultiplied over dark grey
                    out.extend(p[..3].iter().map(|v| ((v + bg) * 255.0).round() as u8));
                }
            }
        }
        std::fs::write(std::env::temp_dir().join("lamzu_preview.ppm"), out).unwrap();
    }

    #[test]
    #[ignore = "writes 3x renders for the README to %TEMP%/traymouse_showcase (see tools/showcase.py)"]
    fn showcase() {
        let font = std::fs::read(r"C:\Windows\Fonts\seguisb.ttf").unwrap();
        let dir = std::env::temp_dir().join("traymouse_showcase");
        std::fs::create_dir_all(&dir).unwrap();
        let b = |percent, charging| Some(Battery { percent, charging });
        let (lav, cyan, mint, pink, gold, red) =
            (COLOR, [64.0, 200.0, 255.0], [80.0, 230.0, 150.0], [255.0, 110.0, 200.0], [255.0, 196.0, 60.0], RED);
        let shots = [
            ("87_lavender", b(87, false), lav), ("87_cyan", b(87, false), cyan), ("87_mint", b(87, false), mint),
            ("87_pink", b(87, false), pink), ("87_gold", b(87, false), gold), ("87_red", b(87, false), red),
            ("18_low", b(18, false), lav), ("18_low_red", b(18, false), red),
            ("64_charging", b(64, true), lav), ("12_charging", b(12, true), lav), ("x_off", None, lav),
        ];
        for (name, battery, color) in shots {
            let look = Look { color, font: FontVec::try_from_vec(font.clone()).unwrap(), scale: 3.0 };
            let (w, h) = (look.width(battery), 48 * 3);
            let c = look.render(battery, w, h);
            // straight (non-premultiplied) RGBA, preceded by a "w h" line
            let mut out = format!("{w} {h}
").into_bytes();
            for p in &c.px {
                let a = p[3].clamp(0.0, 1.0);
                let un = |v: f32| if a > 0.0 { (v / a * 255.0).round().clamp(0.0, 255.0) as u8 } else { 0 };
                out.extend([un(p[0]), un(p[1]), un(p[2]), if a < 5.0 / 255.0 { 0 } else { (a * 255.0).round() as u8 }]);
            }
            std::fs::write(dir.join(format!("{name}.rgba")), out).unwrap();
        }
    }

    #[test]
    fn saved_color_format() {
        assert_eq!(parse_hex("ff8000
"), Some([255.0, 128.0, 0.0]));
        assert_eq!(parse_hex("#0a0b0c"), Some([10.0, 11.0, 12.0]));
        assert_eq!(parse_hex("xyz"), None);
        assert_eq!(parse_hex("fff"), None);
    }

    #[test]
    fn low_battery_shows_even_with_red_mouse() {
        let font = FontVec::try_from_vec(std::fs::read(r"C:\Windows\Fonts\seguisb.ttf").unwrap()).unwrap();
        let look = Look { color: RED, font, scale: 1.0 };
        let b = |percent| Some(Battery { percent, charging: false });
        let red = |b| look.render(b, look.width(b), 46).px.iter().filter(|p| p[3] > 0.5 && p[0] > p[2] * 2.0).count();
        assert!(red(b(20)) > red(b(80)) + 20, "the percent turns red when low");
    }

    #[test]
    fn renders_visible_glyphs() {
        let font = FontVec::try_from_vec(std::fs::read(r"C:\Windows\Fonts\seguisb.ttf").unwrap()).unwrap();
        let look = Look { color: COLOR, font, scale: 1.0 };
        let visible = |b| look.render(b, look.width(b), 46).px.iter().filter(|p| p[3] > 0.5).count();
        let red = |b| look.render(b, look.width(b), 46).px.iter().filter(|p| p[3] > 0.5 && p[0] > p[2] * 2.0).count();
        let b = |percent, charging| Some(Battery { percent, charging });
        assert!(visible(b(71, false)) > 50);
        assert_eq!(red(b(71, false)), 0, "normal battery has no red");
        assert!(red(b(20, false)) > 20, "low battery mouse is red");
        assert!(visible(b(100, true)) < visible(b(100, false)), "charging percent is smaller");
        assert!(visible(b(100, true)) > visible(None) - red(None) + 30, "charging shows bolt + percent");
        assert!(red(None) > 10, "disconnected shows a red X");
        assert!(visible(None) < visible(b(71, false)), "no text when disconnected");

        // width fits the content: narrower for X / bolt, and nothing drawn in the right padding
        assert!(look.width(None) < look.width(b(71, false)));
        for case in [b(100, false), b(7, false), b(50, true), b(100, true), None] {
            let w = look.width(case);
            let c = look.render(case, w, 46);
            let edge = (0..46).flat_map(|y| [w - 2, w - 1].map(|x| c.px[y * w + x][3])).fold(0f32, f32::max);
            assert!(edge < 0.05, "{case:?} touches the right edge");
        }
    }
}
