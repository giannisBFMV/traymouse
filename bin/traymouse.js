#!/usr/bin/env node
// Starts TrayMouse in the background and returns to the prompt; it keeps running after this exits.
const { spawn } = require("child_process");
const path = require("path");

if (process.platform !== "win32") {
  console.error("TrayMouse is a Windows taskbar widget, so it only runs on Windows.");
  process.exit(1);
}

const exe = path.join(__dirname, "traymouse.exe");
spawn(exe, process.argv.slice(2), { detached: true, stdio: "ignore" }).unref();
console.log("TrayMouse is on your taskbar, next to the tray. Drag it to move it; right-click it for colors or to quit.");
