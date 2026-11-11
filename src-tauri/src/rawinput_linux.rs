// rawinput_linux.rs — 语音鼠标按键监听的 Linux evdev 实现（Arch 移植）
// 枚举：/sys/class/input/input*/，id/vendor=524d && id/product=4c52 的输入节点，
//       对应 /dev/input/eventY（接收器复合设备会出 Mouse/Keyboard 两个节点，语音键
//       在 Keyboard 节点上；全部监听、按键码过滤，等价 Windows 的设备路径过滤）。
// 键值：evdev KEY_F18=188/KEY_F19=189/KEY_F20=190 → VK 129/130/131
//       （本机实测 60 秒：三键全部从 event18 上报，映射与计划 F2 预测一致）。
// 语义：EV_KEY value 1=按下 0=松开 2=长按自动重复（丢弃，否则会打乱
//       <200ms 误触/双击手势状态机）；等价 Windows RIDEV_INPUTSINK 的只听不拦
//       （不 EVIOCGRAB）。热插拔：扫描线程每 2 秒补开新节点，拔出节点读线程自然退出。
#![allow(dead_code)]

use std::collections::HashSet;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::sync::mpsc::Sender;

use super::{VK_AI, VK_TRANSLATE, VK_VOICE};
use crate::applog;

/// evdev 键码 → Windows VK（录音/翻译/AI）
fn evdev_to_vk(code: u16) -> Option<u16> {
    match code {
        188 => Some(VK_TRANSLATE), // KEY_F18
        189 => Some(VK_VOICE),     // KEY_F19
        190 => Some(VK_AI),        // KEY_F20
        _ => None,
    }
}

pub struct RawInputKeyListener {
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RawInputKeyListener {
    /// 启动监听；按键事件 (vk, down) 经 tx 发送。dev_tag 形如 VID_524D&PID_4C52
    /// （Linux 侧按 sysfs vendor/product 匹配，tag 仅用于日志）。
    pub fn start(tx: Sender<(u16, bool)>, dev_tag: String) -> Self {
        let thread = std::thread::Builder::new()
            .name("evmon".into())
            .spawn(move || run_scanner(tx, dev_tag))
            .ok();
        Self { thread }
    }

    pub fn stop(&mut self) {
        // 读线程为阻塞 read，随进程退出即可（与 Windows 版 WM_CLOSE 语义等效）
        let _ = self.thread.take();
    }
}

/// 扫描线程：周期性补开新出现的 Rulink evdev 节点
fn run_scanner(tx: Sender<(u16, bool)>, dev_tag: String) {
    let mut known: HashSet<String> = HashSet::new();
    let mut announced = false;
    loop {
        for node in find_rulink_event_nodes() {
            if known.contains(&node) {
                continue;
            }
            known.insert(node.clone());
            let tx = tx.clone();
            let node_for_log = node.clone();
            let ok = std::thread::Builder::new()
                .name("evdev".into())
                .spawn(move || read_events(&node, tx))
                .is_ok();
            if ok && announced {
                applog::info(&format!("evdev: 新增按键节点 {}", node_for_log));
            }
        }
        if !announced {
            if known.is_empty() {
                applog::warn(&format!(
                    "evdev: 未找到 {} 的输入节点（接收器未插？），每 2 秒自动重试",
                    dev_tag
                ));
            } else {
                applog::info(&format!(
                    "evdev 按键监听已启动（{} 个节点，F18=188/F19=189/F20=190，重复事件已过滤）",
                    known.len()
                ));
                announced = true;
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

/// 找出 Rulink（VID 524D / PID 4C52）的所有 evdev 节点
fn find_rulink_event_nodes() -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/sys/class/input") else {
        return out;
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("input") {
            continue;
        }
        let dir = entry.path();
        let vendor = std::fs::read_to_string(dir.join("id/vendor"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let product = std::fs::read_to_string(dir.join("id/product"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if !vendor.eq_ignore_ascii_case("524d") || !product.eq_ignore_ascii_case("4c52") {
            continue;
        }
        // inputX/eventY
        let Ok(ev) = std::fs::read_dir(&dir) else { continue };
        for e in ev.flatten() {
            let en = e.file_name().to_string_lossy().to_string();
            if en.starts_with("event") {
                out.push(format!("/dev/input/{}", en));
            }
        }
    }
    out
}

/// 单节点读线程：键盘型节点 EVIOCGRAB 独占抓取后阻塞读，过滤语音键后转发。
/// 抓取的语义：Rulink 键盘接口的按键只进入本应用，不再同时泄漏给桌面环境
/// —— 否则 F18/F19/F20 会触达 KDE 全局快捷键（实测 F20 长按令系统麦克风
/// 静音开关随自动重复疯狂切换）。应用退出/拔出时内核自动释放 grab。
/// 鼠标节点（只有按键/滚轮，无 F18-F20 键位）绝不抓取，否则指针失灵。
fn read_events(dev: &str, tx: Sender<(u16, bool)>) {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(dev)
    {
        Ok(f) => f,
        Err(_) => return, // 无权限/已消失：静默退出（扫描线程会重试新节点）
    };
    let mut f = file;
    // EVIOCGBIT(EV_KEY) 查键位表：只有带 F18/F19/F20 键位的节点才值得抓
    // （_IOC(_IOC_READ, 'E', 0x20 + EV_KEY, 24) = 0x80184521）
    const EVIOCGBIT_KEY: libc::c_ulong = 0x8018_4521;
    const EVIOCGRAB: libc::c_ulong = 0x4004_4590; // _IOW('E', 0x90, int)
    let mut keybits = [0u8; 24]; // 覆盖键码 0..191
    let has_voice_keys = unsafe {
        if libc::ioctl(f.as_raw_fd(), EVIOCGBIT_KEY, keybits.as_mut_ptr()) < 0 {
            false
        } else {
            let has = |code: u16| keybits[(code / 8) as usize] & (1 << (code % 8)) != 0;
            has(188) || has(189) || has(190)
        }
    };
    if has_voice_keys {
        unsafe {
            if libc::ioctl(f.as_raw_fd(), EVIOCGRAB, 1 as libc::c_int) == 0 {
                applog::info(&format!("evdev: 已独占抓取键盘节点 {}（按键不再进入系统）", dev));
            }
        }
    }
    let ev_size = std::mem::size_of::<InputEvent>();
    let mut buf = [0u8; 64];
    loop {
        // input_event 体积小且内核按整事件写入，这里逐事件读取
        let mut got = 0usize;
        while got < ev_size {
            match f.read(&mut buf[got..ev_size]) {
                Ok(0) => return, // 设备拔出（EOF）
                Ok(n) => got += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return, // ENODEV 等
            }
        }
        let ev = InputEvent::from_bytes(&buf[..ev_size]);
        if ev.typ != 1 {
            continue; // EV_KEY
        }
        let Some(vk) = evdev_to_vk(ev.code) else { continue };
        match ev.value {
            1 => {
                let _ = tx.send((vk, true));
            }
            0 => {
                let _ = tx.send((vk, false));
            }
            _ => {} // 2 = 长按自动重复，丢弃
        }
    }
}

/// struct input_event（timeval 大小随指针宽度变化）
#[repr(C)]
struct InputEvent {
    #[cfg(target_pointer_width = "64")]
    tv_sec: i64,
    #[cfg(target_pointer_width = "64")]
    tv_usec: i64,
    #[cfg(target_pointer_width = "32")]
    tv_sec: i32,
    #[cfg(target_pointer_width = "32")]
    tv_usec: i32,
    typ: u16,
    code: u16,
    value: i32,
}

impl InputEvent {
    fn from_bytes(b: &[u8]) -> Self {
        #[cfg(target_pointer_width = "64")]
        {
            Self {
                tv_sec: i64::from_le_bytes(b[0..8].try_into().unwrap()),
                tv_usec: i64::from_le_bytes(b[8..16].try_into().unwrap()),
                typ: u16::from_le_bytes(b[16..18].try_into().unwrap()),
                code: u16::from_le_bytes(b[18..20].try_into().unwrap()),
                value: i32::from_le_bytes(b[20..24].try_into().unwrap()),
            }
        }
        #[cfg(target_pointer_width = "32")]
        {
            Self {
                tv_sec: i32::from_le_bytes(b[0..4].try_into().unwrap()),
                tv_usec: i32::from_le_bytes(b[4..8].try_into().unwrap()),
                typ: u16::from_le_bytes(b[8..10].try_into().unwrap()),
                code: u16::from_le_bytes(b[10..12].try_into().unwrap()),
                value: i32::from_le_bytes(b[12..16].try_into().unwrap()),
            }
        }
    }
}
