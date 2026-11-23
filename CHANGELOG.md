# Changelog

## v1.0.4

**New: an app icon** 🎨

`traymouse.exe` now has its own icon (the purple TrayMouse logo), so it's easy to spot in File Explorer, the Start menu, Task Manager and your Downloads folder.

## v1.0.3

**Fixed: the widget moved when tray icons changed** 🐛

If you dragged TrayMouse somewhere else on the taskbar (for example next to the Start button), it still moved whenever an app with a tray icon opened or closed (Discord, Spotify, Steam...).

- **Next to the tray:** drop it within a few pixels of the tray icons and it follows them as icons come and go, like before.
- **Anywhere else:** drop it anywhere else on the taskbar and it now stays exactly where you put it.
- **Your position is kept:** a spot you saved with an older version is converted automatically.
- **Reset position** (right-click) still puts it back next to the tray.

## v1.0.2

**New: install with npm** 📦

- **npm package:** `npx traymouse` puts TrayMouse on your taskbar if you have Node.js. `npx traymouse@latest` updates it.
- **Only one widget:** launching it again no longer opens a second copy.
- **Settings survive updates:** your mouse color and position are saved in `%APPDATA%\TrayMouse`.

## v1.0.1

**Squeak is now TrayMouse** 🖱️, a name that says what it does. Same app, same features.

## v1.0.0

First release (as Squeak) 🐭

- Battery percentage next to your tray, for LAMZU, Logitech and Razer wireless mice
- Pick any mouse color (right-click → Mouse color…)
- Red warning below 25%, ⚡ while charging, ✕ when the mouse is off
- Hides over fullscreen games and videos
