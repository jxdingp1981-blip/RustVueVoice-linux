// hid.rs — Rulink/MR08 语音鼠标厂商 HID 接口：枚举 + 后台读流 + 命令写入（平台层）
// Windows = SetupDi 枚举 + CreateFile/overlapped ReadFile（hid_windows.rs，MI 由路径匹配）
// Linux   = /sys/class/hidraw 枚举 + hidraw poll/read（hid_linux.rs，MI 由 bInterfaceNumber 匹配）
// 两端数据形态一致：读侧 65B（首字节 report id 0x00 + 64B payload）；
// Linux hidraw 读出不含 ID 字节，由 hid_linux.rs 统一前置 0x00 归一化。
#![allow(dead_code)]

#[derive(Debug, Clone)]
pub struct HidDeviceInfo {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub input_report_len: u16,
    pub output_report_len: u16,
    /// Linux：USB 接口号（"00"）；Windows：空（接口匹配走设备路径）
    pub iface: String,
}

impl HidDeviceInfo {
    pub fn describe(&self) -> String {
        format!(
            "HID(VID_{:04X}&PID_{:04X} UsagePage=0x{:04X} Usage=0x{:04X} in={}B out={}B)",
            self.vendor_id, self.product_id, self.usage_page, self.usage,
            self.input_report_len, self.output_report_len
        )
    }
}

#[cfg(windows)]
#[path = "hid_windows.rs"]
mod imp;
#[cfg(not(windows))]
#[path = "hid_linux.rs"]
mod imp;

#[allow(unused_imports)]
pub use imp::{find_rulink_devices, find_voice_device, HidStream};
