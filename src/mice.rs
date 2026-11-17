//! Battery reading per brand, over Windows' own HID API (what hidapi wraps).
//! LAMZU: from lamzu.net's web driver (tested). Razer: from OpenRazer. Logitech: HID++ 2.0, from Solaar.
//! Logitech tested with a G Pro Wireless (receiver and cable). Razer is written from OpenRazer, untested.

use std::{mem::zeroed, ptr::null_mut, time::{Duration, Instant}};
use windows_sys::Win32::{
    Devices::{DeviceAndDriverInstallation::*, HumanInterfaceDevice::*},
    Foundation::*,
    Storage::FileSystem::*,
    System::{Threading::*, IO::*},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool, // plugged in (charging or full on the cable)
}

/// Each brand returns the battery if this HID interface is its mouse and the mouse answers.
type Brand = fn(&str, &[u16]) -> Option<Battery>;
const BRANDS: [Brand; 3] = [lamzu, razer, logitech]; // order used when several mice answer

/// Battery of the first supported mouse that answers, as a percent. None = no supported mouse answering.
/// (A plugged-in receiver whose mouse is off doesn't block another mouse that's in use.)
pub fn read_battery() -> Option<Battery> {
    BRANDS.iter().find_map(|brand| for_each_hid(brand))
}

// ---------- LAMZU (Maya X 8K dongle, or the mouse on its cable) ----------

fn lamzu(path: &str, wide: &[u16]) -> Option<Battery> {
    if !path.contains("vid_373e&") {
        return None; // any LAMZU device: the web app uses the same request wired and wireless
    }
    let dev = Hid::open(wide, false)?;
    let caps = dev.caps()?;
    if caps.UsagePage != 0xFFFF || caps.Usage != 0 {
        return None;
    }
    lamzu_read(&dev, caps.FeatureReportByteLength.max(65) as usize)
}

fn lamzu_read(dev: &Hid, n: usize) -> Option<Battery> {
    let mut req = vec![0u8; n]; // [0] = report id 0, then the web app's 64 bytes
    req[3] = 2;
    req[4] = 2;
    req[6] = 0x83;
    dev.set_feature(&req)?;
    std::thread::sleep(Duration::from_millis(100));
    lamzu_parse(&dev.get_feature(n)?).filter(|b| b.percent > 0) // 0 = mouse off/asleep
}

/// Reply layout: a1 .. 02 .. 83 charge-status percent (the web app's ChargeStatus: 1 = charging).
pub fn lamzu_parse(r: &[u8]) -> Option<Battery> {
    (r.len() > 8 && r[1] == 0xA1 && r[4] == 2 && r[6] == 0x83).then(|| Battery { percent: r[8], charging: r[7] == 1 })
}

// ---------- Razer (Viper V3 Pro, DeathAdder V3 Pro), wired or via HyperSpeed dongle ----------

const RAZER_PIDS: [&str; 6] = [
    "pid_00c0", "pid_00c1", // Viper V3 Pro wired / wireless
    "pid_00b6", "pid_00b7", // DeathAdder V3 Pro wired / wireless
    "pid_00c2", "pid_00c3", // DeathAdder V3 Pro (alt) wired / wireless
];

fn razer(path: &str, wide: &[u16]) -> Option<Battery> {
    if !path.contains("vid_1532&") || !RAZER_PIDS.iter().any(|p| path.contains(p)) {
        return None;
    }
    let dev = Hid::open(wide, false)?;
    if dev.caps()?.FeatureReportByteLength != 91 {
        return None; // the 90-byte control report (+ report id) lives on one interface only
    }
    razer_read(&dev)
}

fn razer_read(dev: &Hid) -> Option<Battery> {
    let level = razer_get(dev, 0x80)?; // battery level 0-255
    let percent = ((level as u32 * 100 + 127) / 255) as u8;
    let charging = razer_get(dev, 0x84).is_some_and(|c| c == 1); // charging status 0/1
    (percent > 0).then_some(Battery { percent, charging })
}

/// Class 0x07 (power) query; returns arguments[1] of the reply.
fn razer_get(dev: &Hid, id: u8) -> Option<u8> {
    let req = razer_request(0x07, id);
    for _ in 0..3 {
        dev.set_feature(&req)?;
        std::thread::sleep(Duration::from_millis(60)); // OpenRazer waits ~60ms for wireless receivers
        let reply = dev.get_feature(91)?;
        match reply[1] {
            0x02 => return Some(reply[10]),
            0x01 => continue, // busy, ask again
            _ => return None, // 0x04 timeout = mouse off
        }
    }
    None
}

/// Report id 0 + OpenRazer's 90-byte razer_report.
pub fn razer_request(class: u8, id: u8) -> [u8; 91] {
    let mut r = [0u8; 91];
    let b = &mut r[1..];
    b[1] = 0x1F; // transaction id for the V3 Pro family
    b[5] = 0x02; // data size
    b[6] = class;
    b[7] = id;
    b[88] = b[2..88].iter().fold(0, |c, x| c ^ x);
    r
}

// ---------- Logitech: Lightspeed receivers (G Pro Wireless, Superlight 1/2, ...) or the mouse on its cable ----------

const LOGI_RECEIVERS: [&str; 8] = [
    "pid_c539", "pid_c53a", "pid_c53d", "pid_c53f", "pid_c541", "pid_c545", "pid_c547", "pid_c54d",
];
const LOGI_WIRED: [&str; 2] = ["pid_c088", "pid_c094"]; // G Pro Wireless, Superlight - while charging by cable
const SW_ID: u8 = 0x0B;

fn logitech(path: &str, wide: &[u16]) -> Option<Battery> {
    let is = |list: &[&str]| path.contains("vid_046d&") && list.iter().any(|p| path.contains(p));
    // HID++ device index: 1 = the mouse paired to a receiver, 0xFF = the device itself (wired)
    let d = if is(&LOGI_RECEIVERS) { 0x01 } else if is(&LOGI_WIRED) { 0xFF } else { return None };
    let dev = Hid::open(wide, true)?;
    let caps = dev.caps()?;
    if caps.UsagePage != 0xFF00 || caps.Usage != 0x0002 {
        return None; // HID++ long-report (20 byte) collection
    }
    // only claim receivers whose paired device is a mouse (not your keyboard or headset)
    let name = hidpp_feature(&dev, d, 0x0005)?; // DEVICE_NAME
    if hidpp(&dev, d, name, 2)?[0] != 3 {
        return None; // device type 3 = mouse
    }
    logitech_read(&dev, d)
}

fn logitech_read(dev: &Hid, d: u8) -> Option<Battery> {
    let wired = d == 0xFF; // on its cable = plugged in
    let b = if let Some(idx) = hidpp_feature(dev, d, 0x1004) {
        let p = hidpp(dev, d, idx, 1)?; // UNIFIED_BATTERY get_status: percent, level flags, status
        let approx = match p[1] {
            8 => 100, 4 => 50, 2 => 20, 1 => 5, _ => 0,
        };
        // status: 1 recharging, 2 almost full, 3 full, 4 slow recharge (Solaar's BatteryStatus)
        Battery { percent: if p[0] > 0 { p[0] } else { approx }, charging: wired || (1..=4).contains(&p[2]) }
    } else {
        let idx = hidpp_feature(dev, d, 0x1001)?; // BATTERY_VOLTAGE (G Pro Wireless, original Superlight)
        let p = hidpp(dev, d, idx, 0)?;
        // flags bit 7 = external power (Solaar: RECHARGING)
        Battery { percent: voltage_to_percent(u16::from_be_bytes([p[0], p[1]])), charging: wired || p[2] & 0x80 != 0 }
    };
    (b.percent > 0).then_some(b)
}

/// Root feature (index 0), function 0 = getFeature: where `feature` lives on this device.
fn hidpp_feature(dev: &Hid, d: u8, feature: u16) -> Option<u8> {
    let p = hidpp_call(dev, d, 0, 0, &feature.to_be_bytes())?;
    (p[0] != 0).then_some(p[0]) // index 0 = not supported
}

fn hidpp(dev: &Hid, d: u8, idx: u8, func: u8) -> Option<[u8; 16]> {
    hidpp_call(dev, d, idx, func, &[])
}

/// One HID++ 2.0 request to device index `d`; returns the 16 parameter bytes of the matching reply.
fn hidpp_call(dev: &Hid, d: u8, idx: u8, func: u8, params: &[u8]) -> Option<[u8; 16]> {
    let mut req = [0u8; 20];
    req[..4].copy_from_slice(&[0x11, d, idx, func << 4 | SW_ID]);
    req[4..4 + params.len()].copy_from_slice(params);
    dev.write(&req)?;
    let deadline = Instant::now() + Duration::from_millis(250); // a wireless round trip is ~20ms
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let r = dev.read(20, left)?;
        if r[0] == 0x11 && r[1] == d && r[2] == idx && r[3] == req[3] {
            return r[4..20].try_into().ok();
        }
        if r[0] == 0x11 && r[1] == d && r[2] == 0xFF && r[3] == idx {
            return None; // HID++ error (e.g. mouse off)
        }
        // otherwise a notification or someone else's reply (G HUB); keep reading
    }
    None
}

/// Solaar's Li-ion curve: millivolts -> percent, linear between points.
pub fn voltage_to_percent(mv: u16) -> u8 {
    const CURVE: [(u16, u8); 14] = [
        (4186, 100), (4067, 90), (3989, 80), (3922, 70), (3859, 60), (3811, 50), (3778, 40),
        (3751, 30), (3717, 20), (3671, 10), (3646, 5), (3579, 2), (3500, 0), (0, 0),
    ];
    if mv >= CURVE[0].0 {
        return 100;
    }
    CURVE.windows(2).find(|w| mv >= w[1].0).map_or(0, |w| {
        let ((vh, ph), (vl, pl)) = (w[0], w[1]);
        let t = (mv - vl) as f32 / (vh - vl) as f32;
        (pl as f32 + (ph - pl) as f32 * t).round() as u8
    })
}

// ---------- Windows HID plumbing ----------

/// Calls `f` with each HID interface's lowercased path and its NUL-terminated wide path, until it returns Some.
fn for_each_hid<T>(mut f: impl FnMut(&str, &[u16]) -> Option<T>) -> Option<T> {
    unsafe {
        let mut guid = zeroed();
        HidD_GetHidGuid(&mut guid);
        let set = SetupDiGetClassDevsW(&guid, null_mut(), null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
        if set == INVALID_HANDLE_VALUE as _ {
            return None;
        }
        let mut result = None;
        let mut iface: SP_DEVICE_INTERFACE_DATA = zeroed();
        iface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        let mut i = 0;
        while result.is_none() && SetupDiEnumDeviceInterfaces(set, null_mut(), &guid, i, &mut iface) != 0 {
            i += 1;
            let mut needed = 0u32;
            SetupDiGetDeviceInterfaceDetailW(set, &iface, null_mut(), 0, &mut needed, null_mut());
            let mut buf = vec![0u64; needed as usize / 8 + 1]; // u64 for alignment
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &iface, detail, needed, null_mut(), null_mut()) == 0 {
                continue;
            }
            let path = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
            let len = (0..).take_while(|&k| *path.add(k) != 0).count();
            let wide = std::slice::from_raw_parts(path, len + 1);
            result = f(&String::from_utf16_lossy(&wide[..len]).to_lowercase(), wide);
        }
        SetupDiDestroyDeviceInfoList(set);
        result
    }
}

struct Hid(HANDLE);

impl Drop for Hid {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

impl Hid {
    fn open(wide: &[u16], overlapped: bool) -> Option<Hid> {
        let flags = if overlapped { FILE_FLAG_OVERLAPPED } else { 0 };
        let h = unsafe {
            CreateFileW(wide.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE,
                        null_mut(), OPEN_EXISTING, flags, null_mut())
        };
        (h != INVALID_HANDLE_VALUE).then_some(Hid(h))
    }

    fn caps(&self) -> Option<HIDP_CAPS> {
        unsafe {
            let mut pre = zeroed();
            if HidD_GetPreparsedData(self.0, &mut pre) == 0 {
                return None;
            }
            let mut caps: HIDP_CAPS = zeroed();
            let ok = HidP_GetCaps(pre, &mut caps) == HIDP_STATUS_SUCCESS;
            HidD_FreePreparsedData(pre);
            ok.then_some(caps)
        }
    }

    fn set_feature(&self, data: &[u8]) -> Option<()> {
        (unsafe { HidD_SetFeature(self.0, data.as_ptr() as _, data.len() as u32) } != 0).then_some(())
    }

    fn get_feature(&self, len: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len]; // buf[0] = report id 0
        (unsafe { HidD_GetFeature(self.0, buf.as_mut_ptr() as _, len as u32) } != 0).then_some(buf)
    }

    /// Overlapped I/O with a timeout, so a silent device can't hang the widget.
    fn io(&self, buf: *mut u8, len: usize, write: bool, timeout: Duration) -> Option<usize> {
        unsafe {
            let event = CreateEventW(null_mut(), 1, 0, null_mut());
            if event.is_null() {
                return None;
            }
            let mut ov: OVERLAPPED = zeroed();
            ov.hEvent = event;
            let started = if write {
                WriteFile(self.0, buf, len as u32, null_mut(), &mut ov)
            } else {
                ReadFile(self.0, buf, len as u32, null_mut(), &mut ov)
            };
            let mut done = 0u32;
            // an immediate failure (e.g. wrong report size) never signals the event, so don't wait on it
            let in_flight = started != 0 || GetLastError() == ERROR_IO_PENDING;
            let ok = in_flight
                && WaitForSingleObject(event, timeout.as_millis() as u32) == WAIT_OBJECT_0
                && GetOverlappedResult(self.0, &ov, &mut done, 0) != 0;
            if in_flight && !ok {
                CancelIo(self.0);
                GetOverlappedResult(self.0, &ov, &mut done, 1); // wait for the cancel before `ov` goes away
            }
            CloseHandle(event);
            ok.then_some(done as usize)
        }
    }

    fn write(&self, data: &[u8]) -> Option<()> {
        let mut buf = data.to_vec();
        self.io(buf.as_mut_ptr(), buf.len(), true, Duration::from_millis(500)).map(drop)
    }

    fn read(&self, len: usize, timeout: Duration) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len];
        let n = self.io(buf.as_mut_ptr(), len, false, timeout)?;
        buf.truncate(n);
        (n >= 4).then_some(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lamzu_reply() {
        assert_eq!(lamzu_parse(&[0x00, 0xa1, 0x00, 0x02, 0x02, 0x00, 0x83, 0x00, 0x47, 0x00]),
                   Some(Battery { percent: 71, charging: false }));
        assert_eq!(lamzu_parse(&[0x00, 0xa1, 0x00, 0x02, 0x02, 0x00, 0x83, 0x01, 0x47, 0x00]).map(|b| b.charging), Some(true));
        assert_eq!(lamzu_parse(&[0u8; 10]), None);
    }

    #[test]
    fn razer_battery_request_matches_openrazer() {
        let r = razer_request(0x07, 0x80);
        assert_eq!(&r[..9], &[0x00, 0x00, 0x1F, 0x00, 0x00, 0x00, 0x02, 0x07, 0x80]);
        assert_eq!(r[89], 0x02 ^ 0x07 ^ 0x80); // crc = xor of bytes 2..88 of the 90-byte report
    }

    #[test]
    #[ignore = "prints each Logitech mouse that answers: cargo test probe -- --ignored --nocapture"]
    fn probe_logitech() {
        for_each_hid(|path, wide| {
            if let Some(b) = logitech(path, wide) {
                println!("{}: battery {b:?}", &path[..path.find("&mi").unwrap_or(path.len())]);
            }
            None::<()>
        });
    }

    #[test]
    fn logitech_voltage_curve() {
        assert_eq!(voltage_to_percent(4200), 100);
        assert_eq!(voltage_to_percent(3811), 50);
        assert_eq!(voltage_to_percent(3835), 55);
        assert_eq!(voltage_to_percent(3400), 0);
    }
}
