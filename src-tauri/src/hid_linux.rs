// hid_linux.rs — Rulink 语音接收器的 Linux hidraw 实现（Arch 移植）
// 枚举：读 /sys/class/hidraw/*/device/uevent（HID_ID=bus:vid:pid）匹配 524D/4C52，
//       再读父接口 ../bInterfaceNumber 匹配 MI_00；Usage Page 0xFF02 用
//       report_descriptor（全局可读）里的 16-bit Usage Page item（06 02 FF）验证。
// 读流：/dev/hidrawN 以 O_RDWR|O_NONBLOCK 打开，poll(500ms) + read；拔出（POLLHUP/
//       POLLERR/EIO）→ 关闭 → 2 秒重连，语义与 Windows 版 hid-reader 线程一致。
// 归一化：Linux 无编号报告读出的 64B 不带 report id，读线程统一前置 0x00，
//       使 app.rs::on_hid_packet 看到的与 Windows 65B 缓冲完全相同（零业务改动）。
// 写：命令帧（首字节已是 0x00）补零到 65B 写入 —— 与 Windows WriteFile 缓冲一致
//       （实测设备对 64/65B 均接受，65B 为 Windows 精确等价）。
#![allow(dead_code)]

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::HidDeviceInfo;
use crate::applog;
use crate::util;

/// 从 report descriptor 字节流中解析首个 Usage Page（处理 05=1字节 / 06=2字节 item）
fn parse_usage_page(desc: &[u8]) -> Option<u16> {
    let mut i = 0;
    while i < desc.len() {
        let item = desc[i];
        let size = match item & 0x03 {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => 4,
        };
        if i + 1 + size > desc.len() {
            break;
        }
        // Global::Usage Page（0x05 一字节 / 0x06 两字节）
        if item == 0x05 && size == 1 {
            return Some(desc[i + 1] as u16);
        }
        if item == 0x06 && size == 2 {
            return Some(u16::from_le_bytes([desc[i + 1], desc[i + 2]]));
        }
        i += 1 + size;
    }
    None
}

/// 从 report descriptor 提取第一个 Usage（Local item 09）
fn parse_usage(desc: &[u8]) -> u16 {
    let mut i = 0;
    while i < desc.len() {
        let item = desc[i];
        let size = match item & 0x03 {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => 4,
        };
        if i + 1 + size > desc.len() {
            break;
        }
        if item == 0x09 && size >= 1 {
            return match size {
                1 => desc[i + 1] as u16,
                2 => u16::from_le_bytes([desc[i + 1], desc[i + 2]]),
                _ => desc[i + 1] as u16,
            };
        }
        i += 1 + size;
    }
    0
}

/// 枚举全部 hidraw 设备（读 sysfs，不打开设备节点）
fn enumerate_hidraw() -> Vec<HidDeviceInfo> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir("/sys/class/hidraw") {
        Ok(rd) => rd,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let node = entry.file_name().to_string_lossy().to_string();
        let dev_path = entry.path().join("device");
        // uevent: DRIVER=hid-generic / HID_ID=0003:0000524D:00004C52 / HID_NAME=...
        let uevent = match std::fs::read_to_string(dev_path.join("uevent")) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let mut hid_id = String::new();
        for line in uevent.lines() {
            if let Some(v) = line.strip_prefix("HID_ID=") {
                hid_id = v.trim().to_string();
            }
        }
        // HID_ID = bus:vid:pid（8 位十六进制）
        let parts: Vec<&str> = hid_id.split(':').collect();
        if parts.len() != 3 {
            continue;
        }
        let vendor_id = match u16::from_str_radix(parts[1].trim_start_matches('0'), 16) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let product_id = match u16::from_str_radix(parts[2].trim_start_matches('0'), 16) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // 父 USB 接口号（…/1-2:1.0/0003:524D:4C52.K → ../bInterfaceNumber）
        let iface = std::fs::read_to_string(dev_path.join("..").join("bInterfaceNumber"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        // Usage Page：sysfs 的 report_descriptor 全局可读，无需打开设备节点
        let (usage_page, usage) = match std::fs::read(dev_path.join("report_descriptor")) {
            Ok(desc) => (parse_usage_page(&desc).unwrap_or(0), parse_usage(&desc)),
            Err(_) => (0, 0),
        };
        out.push(HidDeviceInfo {
            path: format!("/dev/{}", node),
            vendor_id,
            product_id,
            usage_page,
            usage,
            input_report_len: 64,  // hidraw 无编号报告读出 64B（不含 ID）
            output_report_len: 65, // 写缓冲 = 0x00 + 64B（与 Windows 语义一致）
            iface,
        });
    }
    out
}

/// 找到接收器的所有 HID 接口（不区分 MI）
pub fn find_rulink_devices(vid: u16, pid: u16) -> Vec<HidDeviceInfo> {
    enumerate_hidraw()
        .into_iter()
        .filter(|d| d.vendor_id == vid && d.product_id == pid)
        .collect()
}

/// 找到语音数据所在的厂商自定义 HID 接口（复合设备 MI_00，UsagePage 0xFF02）
pub fn find_voice_device(vid: u16, pid: u16, mi: &str) -> Option<HidDeviceInfo> {
    for info in find_rulink_devices(vid, pid) {
        if info.iface != mi {
            continue;
        }
        // Linux 直接可验证 Usage Page；描述符解析失败（=0）时退化为接口号匹配
        if info.usage_page != 0xFF02 && info.usage_page != 0 {
            continue;
        }
        return Some(info);
    }
    None
}

/// 后台 hidraw 流：常驻 poll+read，每包以 Windows 65B 形态回调；write() 发送命令。
pub struct HidStream {
    info: HidDeviceInfo,
    stop: Arc<AtomicBool>,
    file: Arc<Mutex<Option<File>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HidStream {
    pub fn start(info: HidDeviceInfo, packet_tx: Sender<(Vec<u8>, f64)>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let file = Arc::new(Mutex::new(None));
        let stop2 = stop.clone();
        let file2 = file.clone();
        let info2 = info.clone();
        let thread = std::thread::Builder::new()
            .name("hid-reader".into())
            .spawn(move || HidStream::run(&info2, &stop2, &file2, &packet_tx))
            .ok();
        // 等读线程完成设备打开（Windows 侧 CreateFile 在 run 内同步完成，这里
        // 需要显式对齐，否则紧随其后的 INFO_QUERY/电量查询会写空）
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if file.lock().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Self { info, stop, file, thread }
    }

    /// 向设备写一个输出报告（含 report id 0x00 前缀，补齐到 65B）
    pub fn write(&self, data: &[u8]) -> bool {
        let mut buf = vec![0u8; 65];
        let n = std::cmp::min(data.len(), 65);
        buf[..n].copy_from_slice(&data[..n]);
        for _ in 0..5 {
            let guard = self.file.lock().unwrap();
            let Some(f) = guard.as_ref() else {
                return false;
            };
            match (&*f).write_all(&buf) {
                Ok(()) => return true,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    drop(guard);
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(_) => return false, // 设备拔出等
            }
        }
        false
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // 关闭文件使 reader 线程的下一次 poll/read 返回错误（poll 有 500ms 超时兜底）
        if let Ok(mut g) = self.file.lock() {
            *g = None;
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn run(
        info: &HidDeviceInfo,
        stop: &AtomicBool,
        file_cell: &Mutex<Option<File>>,
        packet_tx: &Sender<(Vec<u8>, f64)>,
    ) {
        while !stop.load(Ordering::SeqCst) {
            // 打开（O_RDWR | O_NONBLOCK | O_CLOEXEC）
            let opened = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(&info.path);
            let file = match opened {
                Ok(f) => f,
                Err(_) => {
                    applog::warn("HID 打开失败，2 秒后重试");
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };
            let fd = file.as_raw_fd();
            *file_cell.lock().unwrap() = Some(file);

            loop {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                // poll 500ms（等价 Windows WaitForSingleObject 超时模型，便于 stop）
                let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
                let rc = unsafe { libc::poll(&mut pfd, 1, 500) };
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                if rc <= 0 {
                    continue; // 超时 / EINTR
                }
                if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                    break; // 设备拔出
                }
                if pfd.revents & libc::POLLIN == 0 {
                    continue;
                }
                let mut buf = [0u8; 4096];
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if n <= 0 {
                    if n < 0 && errno_is_retry() {
                        continue;
                    }
                    break; // EIO（拔出）或 EOF
                }
                // 归一化为 Windows 形态：0x00 report id + 64B payload
                let mut packet = Vec::with_capacity(n as usize + 1);
                packet.push(0u8);
                packet.extend_from_slice(&buf[..n as usize]);
                let _ = packet_tx.send((packet, util::epoch_now()));
            }

            *file_cell.lock().unwrap() = None;
            if stop.load(Ordering::SeqCst) {
                return;
            }
            applog::warn("HID 读取中断（设备可能已拔出），2 秒后重连");
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

fn errno_is_retry() -> bool {
    let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    e == libc::EAGAIN || e == libc::EINTR || e == libc::EWOULDBLOCK
}
