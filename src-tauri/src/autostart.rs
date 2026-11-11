// autostart.rs — 开机自启（平台层）
// Windows = HKCU\...\Run 注册表（autostart_windows.rs）
// Linux   = XDG autostart ~/.config/autostart/rustvuevoice.desktop（autostart_linux.rs）
//           X11/Wayland 通用（GNOME/KDE 均支持 XDG autostart）
#![allow(dead_code)]

#[cfg(windows)]
#[path = "autostart_windows.rs"]
mod imp;
#[cfg(not(windows))]
#[path = "autostart_linux.rs"]
mod imp;

pub use imp::{is_enabled, set_enabled};
