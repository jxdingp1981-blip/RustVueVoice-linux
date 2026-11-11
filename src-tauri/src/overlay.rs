// overlay.rs — 屏幕居中圆角胶囊悬浮层（平台层）
// Windows = Win32 分层窗口 + GDI+（overlay_windows.rs）
// Linux   = x11rb ARGB 窗口 + cairo 绘制 + XShape 点击穿透（overlay_linux.rs）；
//           Wayland 会话经 XWayland 显示（GNOME/KDE 默认带 XWayland）
// 视觉规格与 overlay.py 逐项一致：胶囊 560×128 圆角 18、24 根波形条、
// 呼吸 0.78~0.98 / 2.4s、流光 hold 0.22s / decay 0.40s、60ms tick、
// 正文 100/200 字截断、多行时胶囊底部向下扩行。
#![allow(dead_code)]

// ---- 几何 / 视觉常量（照抄 overlay.py） ----
pub const OVERLAY_WIDTH: i32 = 560;
pub const OVERLAY_HEIGHT: i32 = 128;
pub const CORNER_PX: f64 = 18.0;
pub const WAVE_BARS: usize = 24;
pub const WAVE_BAR_W: f64 = 4.0;
pub const WAVE_BAR_GAP: f64 = 3.0;
pub const WAVE_BAR_MAX_H: f64 = 24.0; // 有文字时 18
pub const WAVE_CY: f64 = 30.0;
pub const STATUS_Y: f64 = 58.0;
pub const TEXT_Y: f64 = 88.0;
pub const TEXT_KEEP_TAIL: usize = 100;

// ARGB（0xAARRGGBB）
pub const CAPSULE_FILL: u32 = 0xFF12141A;
pub const BORDER_COLOR: u32 = 0xFF585A5E;
pub const WAVE_COLOR: u32 = 0xFF7FF2A6;
pub const TEXT_COLOR: u32 = 0xFFFFFFFF;
pub const STATUS_COLOR: u32 = 0xFFC0C6D4;
pub const STATUS_OK_COLOR: u32 = 0xFF7DD48A;
pub const STATUS_ERR_COLOR: u32 = 0xFFFF7A80;

// ---- 动画参数（照抄 overlay.py） ----
pub const TICK_MS: u64 = 60;
pub const BREATH_PERIOD_S: f64 = 2.4;
pub const OPACITY_BREATH_MIN: f64 = 0.78;
pub const OPACITY_BREATH_MAX: f64 = 0.98;
pub const GLINT_PEAK: f64 = 1.00;
pub const GLINT_HOLD_S: f64 = 0.22;
pub const GLINT_DECAY_S: f64 = 0.40;

// ---- 事件（对应 overlay.post 的元组） ----
#[derive(Debug, Clone)]
pub enum OverlayEvent {
    Show,
    Hide,
    Status(String),
    Partial(String),
    Level(f64),
    Done(String, u32),   // (text, hide_ms)
    Cancel(String, u32), // (text, hide_ms)
    Recognizing,         // 已松手，正在识别（停止计时，显示“识别中…”）
}

#[cfg(windows)]
#[path = "overlay_windows.rs"]
mod imp;
#[cfg(not(windows))]
#[path = "overlay_linux.rs"]
mod imp;

#[allow(unused_imports)]
pub use imp::{create_overlay, destroy_overlay, start_overlay_thread, OverlayHandle};
