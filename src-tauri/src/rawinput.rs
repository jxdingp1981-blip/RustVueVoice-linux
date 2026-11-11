// rawinput.rs — 语音鼠标按键监听（平台层）
// Windows = Raw Input 隐藏窗口 + RIDEV_INPUTSINK（rawinput_windows.rs）
// Linux   = evdev /dev/input/event*（rawinput_linux.rs），按父设备 VID/PID 过滤
// 键值映射：VK 129/130/131（F18/F19/F20）↔ evdev KEY_F18/19/20 = 188/189/190
// （本机实测：鼠标键盘接口 event18 上报 188/189/190，长按会产生 value=2 重复事件，
//   已在 Linux 读线程丢弃，down/up 语义与 Windows RI_KEY_BREAK/BREAK 对齐）
#![allow(dead_code)]

/// F19 录音（Windows VK 130）
pub const VK_VOICE: u16 = 130;
/// F18 翻译（Windows VK 129，当前实现 = 向前台发回车）
pub const VK_TRANSLATE: u16 = 129;
/// F20 AI（Windows VK 131）
pub const VK_AI: u16 = 131;

pub fn key_name(vk: u16) -> String {
    match vk {
        VK_VOICE => "录音键(F19)".into(),
        VK_TRANSLATE => "翻译键(F18)".into(),
        VK_AI => "AI键(F20)".into(),
        _ => format!("VK{}", vk),
    }
}

#[cfg(windows)]
#[path = "rawinput_windows.rs"]
mod imp;
#[cfg(not(windows))]
#[path = "rawinput_linux.rs"]
mod imp;

pub use imp::RawInputKeyListener;
