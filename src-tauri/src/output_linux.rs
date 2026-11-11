// output_linux.rs — 剪贴板 + 键盘注入的 Linux 实现（Arch 移植）
// 剪贴板：arboard（X11 后端经 XWayland 在 GNOME/KDE Wayland 下与 Wayland 剪贴板
//         自动同步；wayland-data-control 后端用于 wlroots 系）。实例常驻（static），
//         保持剪贴板所有权直到进程退出 —— 「识别文本可粘贴」是产品恒定行为。
// 注入：uinput 内核级虚拟键盘（X11/KDE-Wayland/GNOME-Wayland/wlroots 全部生效，
//         合成器当物理键盘）；Ctrl+V 粘贴与回车与 Windows 版逐键等价（10ms 节奏）。
// unicode 直打：X11 会话且装有 xdotool → xdotool type；wlroots 系会话且装有 wtype
//         → wtype；否则自动降级为 Ctrl+V 粘贴（文本已在剪贴板，效果一致）。
#![allow(dead_code)]

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use crate::applog;

// ---------------- 剪贴板（arboard，实例常驻保持所有权） ----------------

static CLIPBOARD: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();

fn clipboard_cell() -> &'static Mutex<Option<arboard::Clipboard>> {
    CLIPBOARD.get_or_init(|| Mutex::new(None))
}

/// 把文本写入剪贴板。持有 Clipboard 实例（X11 选择权归属本进程）。
pub fn set_clipboard_text(text: &str) -> bool {
    let mut cell = clipboard_cell().lock().unwrap();
    for _ in 0..3 {
        let mut clip = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                // X11 短暂竞争时重试（对齐 Windows 版 20ms×20 次的等待语义）
                thread::sleep(Duration::from_millis(20));
                applog::warn(&format!("打开剪贴板失败: {}（重试中）", e));
                continue;
            }
        };
        return match clip.set_text(text.to_string()) {
            Ok(()) => {
                *cell = Some(clip); // 常驻持有，保证识别文本始终可粘贴
                true
            }
            Err(e) => {
                applog::warn(&format!("写入剪贴板失败: {}", e));
                false
            }
        };
    }
    false
}

// ---------------- uinput 虚拟键盘（内核级注入，全桌面通用） ----------------

const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564; // _IOW('U', 100, int)
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565; // _IOW('U', 101, int)
const UI_DEV_SETUP: libc::c_ulong = 0x405C_5503; // _IOW('U', 3, struct uinput_setup)
const UI_DEV_CREATE: libc::c_ulong = 0x5501; // _IO('U', 1)

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const KEY_LEFTCTRL: u16 = 29;
const KEY_V: u16 = 47; // input-event-codes.h: KEY_V=47（25 是 KEY_P！曾致 Ctrl+P 误触发打印）
const KEY_ENTER: u16 = 28;
const BUS_USB: u16 = 0x03;

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [std::os::raw::c_char; 80],
    ff_effects_max: u32,
}

#[repr(C)]
struct RawInputEvent {
    tv_sec: i64,
    tv_usec: i64,
    typ: u16,
    code: u16,
    value: i32,
}

static UINPUT_FD: OnceLock<i32> = OnceLock::new();
static UINPUT_FAILED: AtomicBool = AtomicBool::new(false);

/// 懒创建 uinput 虚拟键盘；不可用时返回 None（回退 xdotool/提示）
fn uinput_fd() -> Option<i32> {
    let fd = *UINPUT_FD.get_or_init(|| match create_uinput() {
        Ok(fd) => {
            applog::info("uinput 虚拟键盘已创建（内核级注入后端）");
            fd
        }
        Err(e) => {
            UINPUT_FAILED.store(true, Ordering::SeqCst);
            applog::warn(&format!("uinput 初始化失败（{}），注入走 xdotool 回退", e));
            -1
        }
    });
    if fd < 0 {
        None
    } else {
        Some(fd)
    }
}

fn create_uinput() -> Result<i32, String> {
    let path = std::env::var("RUSTVUEVOICE_UINPUT").unwrap_or_else(|_| "/dev/uinput".into());
    let fd = unsafe {
        libc::open(
            std::ffi::CString::new(path).unwrap().as_ptr(),
            libc::O_WRONLY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(format!(
            "打开 /dev/uinput 失败: {}（需要 udev 规则或 input 组权限）",
            std::io::Error::last_os_error()
        ));
    }
    unsafe {
        let bits: [(libc::c_ulong, u16); 5] = [
            (UI_SET_EVBIT, EV_KEY),
            (UI_SET_EVBIT, EV_SYN),
            (UI_SET_KEYBIT, KEY_LEFTCTRL),
            (UI_SET_KEYBIT, KEY_V),
            (UI_SET_KEYBIT, KEY_ENTER),
        ];
        for (req, bit) in bits {
            if libc::ioctl(fd, req, bit as libc::c_int) < 0 {
                let err = std::io::Error::last_os_error().to_string();
                libc::close(fd);
                return Err(format!("UI_SET_*BIT 失败: {}", err));
            }
        }
        let mut setup = UinputSetup {
            id: InputId { bustype: BUS_USB, vendor: 0x524D, product: 0x0001, version: 1 },
            name: [0; 80],
            ff_effects_max: 0,
        };
        for (i, &b) in b"RustVueVoice Virtual Keyboard".iter().enumerate() {
            setup.name[i] = b as std::os::raw::c_char;
        }
        if libc::ioctl(fd, UI_DEV_SETUP, &setup) < 0 {
            let err = std::io::Error::last_os_error().to_string();
            libc::close(fd);
            return Err(format!("UI_DEV_SETUP 失败: {}", err));
        }
        if libc::ioctl(fd, UI_DEV_CREATE) < 0 {
            let err = std::io::Error::last_os_error().to_string();
            libc::close(fd);
            return Err(format!("UI_DEV_CREATE 失败: {}", err));
        }
    }
    // 等内核注册 evdev 节点，避免首批事件丢失
    thread::sleep(Duration::from_millis(30));
    Ok(fd)
}

fn emit(fd: i32, typ: u16, code: u16, value: i32) {
    let ev = RawInputEvent { tv_sec: 0, tv_usec: 0, typ, code, value };
    let syn = RawInputEvent { tv_sec: 0, tv_usec: 0, typ: EV_SYN, code: 0, value: 0 };
    unsafe {
        libc::write(
            fd,
            &ev as *const RawInputEvent as *const libc::c_void,
            std::mem::size_of::<RawInputEvent>(),
        );
        libc::write(
            fd,
            &syn as *const RawInputEvent as *const libc::c_void,
            std::mem::size_of::<RawInputEvent>(),
        );
    }
}

fn send_key_uinput(fd: i32, code: u16, down: bool) {
    emit(fd, EV_KEY, code, if down { 1 } else { 0 });
    thread::sleep(Duration::from_millis(10));
}

// ---------------- 对外 API（语义对齐 Windows output.rs） ----------------

/// 模拟 Ctrl+V 粘贴（uinput；不可用时回退 xdotool）
pub fn paste_clipboard() {
    if let Some(fd) = uinput_fd() {
        send_key_uinput(fd, KEY_LEFTCTRL, true);
        send_key_uinput(fd, KEY_V, true);
        send_key_uinput(fd, KEY_V, false);
        send_key_uinput(fd, KEY_LEFTCTRL, false);
        return;
    }
    // 回退：X11 的 xdotool（含 XWayland）
    if run_xdotool(&["key", "--clearmodifiers", "ctrl+v"]) {
        return;
    }
    applog::error("粘贴注入失败：uinput 与 xdotool 均不可用");
}

/// 模拟按下并松开一次回车键，作用于当前聚焦窗口
pub fn press_enter() {
    if let Some(fd) = uinput_fd() {
        send_key_uinput(fd, KEY_ENTER, true);
        send_key_uinput(fd, KEY_ENTER, false);
        return;
    }
    if run_xdotool(&["key", "--clearmodifiers", "Return"]) {
        return;
    }
    applog::error("回车注入失败：uinput 与 xdotool 均不可用");
}

/// 模拟键盘逐字输入到光标位置。
/// 后端选择：X11 会话 → xdotool；wlroots 系 Wayland → wtype；
/// 其余（GNOME/KDE Wayland，无跨桌面 Unicode 直打 API）→ 自动降级 Ctrl+V 粘贴
/// （文本恒先写剪贴板，上屏效果一致；与计划 F3 的降级语义一致）。
pub fn type_text_unicode(text: &str) {
    let text = text.replace('\r', "");
    if text.is_empty() {
        return;
    }
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default().to_lowercase();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    if session == "x11" && has_binary("xdotool") {
        run_xdotool_type(&text);
        return;
    }
    if session == "wayland" && has_binary("wtype") && is_wlroots_desktop(&desktop) {
        match Command::new("wtype").arg(&text).status() {
            Ok(s) if s.success() => return,
            other => applog::warn(&format!("wtype 直打失败（{:?}），降级为粘贴", other.map(|s| s.code()))),
        }
    }
    // 降级：剪贴板 + Ctrl+V（文本已在剪贴板，最终上屏一致）
    let s = if session.is_empty() { "未知".to_string() } else { session.clone() };
    applog::info(&format!("unicode 直打后端不可用（{} 会话），已降级为剪贴板+Ctrl+V", s));
    paste_clipboard();
}

fn is_wlroots_desktop(desktop: &str) -> bool {
    ["sway", "hypr", "wlroots", "niri", "river", "labwc", "wayfire"]
        .iter()
        .any(|k| desktop.contains(k))
}

fn has_binary(bin: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else { return false };
    path.split(':').any(|dir| {
        let p = std::path::Path::new(dir).join(bin);
        p.is_file()
    })
}

fn run_xdotool(args: &[&str]) -> bool {
    match Command::new("xdotool").args(args).stdout(Stdio::null()).stderr(Stdio::null()).status() {
        Ok(s) if s.success() => true,
        other => {
            applog::warn(&format!("xdotool 执行失败: {:?}", other.map(|s| s.code())));
            false
        }
    }
}

fn run_xdotool_type(text: &str) {
    // --delay 10：键间 10ms（对齐 Windows 32键/批·10ms 的节奏）
    match Command::new("xdotool")
        .args(["type", "--delay", "10", "--clearmodifiers", "--", text])
        .status()
    {
        Ok(s) if s.success() => {}
        other => {
            applog::warn(&format!("xdotool type 失败（{:?}），降级为粘贴", other.map(|s| s.code())));
            paste_clipboard();
        }
    }
}
