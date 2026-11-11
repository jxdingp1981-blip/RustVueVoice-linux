// output.rs — 剪贴板写入 + 键盘注入（平台层）
// Windows = Win32 剪贴板 + SendInput（output_windows.rs）
// Linux   = arboard 剪贴板 + uinput 虚拟键盘注入（output_linux.rs）；
//           unicode 直打在 X11 用 xdotool、wlroots 系用 wtype，其余自动降级 Ctrl+V
//           （文本恒先写剪贴板，降级后上屏效果一致 —— 计划 F3 语义）
#![allow(dead_code)]

#[cfg(windows)]
#[path = "output_windows.rs"]
mod imp;
#[cfg(not(windows))]
#[path = "output_linux.rs"]
mod imp;

pub use imp::{paste_clipboard, press_enter, set_clipboard_text, type_text_unicode};
