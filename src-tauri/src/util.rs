// util.rs — 通用工具：宽字符串、时间戳、控制台输出（跨平台）
// Linux 变更：tz 用 libc localtime_r（tm_gmtoff）替代 GetTimeZoneInformation；
// 新增 open_url（xdg-open / ShellExecuteW）供 app.rs 与 commands.rs 共用。
#![allow(dead_code)]

use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use windows::core::PCWSTR;

/// UTF-8 → UTF-16 空终止宽字符串（用于 Win32 W 系列 API）
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
pub fn pcw(s: &str) -> PCWSTR {
    PCWSTR::from_raw(wide(s).as_ptr())
}

/// 不含空终止的宽字符串
pub fn wide_nz(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// 本地时间字符串 "YYYY-MM-DD HH:MM:SS,mmm"（对应 Python logging 默认格式）
pub fn local_time_str() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as i64;
    let ms = now.subsec_millis();
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02},{:03}", y, mo, d, h, mi, s, ms)
}

/// 简短时间 "HH:MM:SS"（主控窗口日志区用）
pub fn short_time_str() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let (.., h, mi, s) = civil_from_unix(now.as_secs() as i64);
    format!("{:02}:{:02}:{:02}", h, mi, s)
}

/// unix 秒 → 本地民用时间（时区取系统当前偏移）
fn civil_from_unix(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let bias = tz_bias_seconds(); // 本地 = UTC + bias
    let t = secs + bias;
    let days = t.div_euclid(86400);
    let rem = t.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    (y, m, d, (rem / 3600) as u32, ((rem % 3600) / 60) as u32, (rem % 60) as u32)
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((y + if m <= 2 { 1 } else { 0 }) as i32, m as u32, d as u32)
}

/// 本地 - UTC 的偏移秒（local = utc + bias）
#[cfg(windows)]
fn tz_bias_seconds() -> i64 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    unsafe {
        let mut tzi = TIME_ZONE_INFORMATION::default();
        let rc = GetTimeZoneInformation(&mut tzi);
        let bias: i64 = tzi.Bias as i64; // UTC = local + bias(minutes)
        // rc==2（DST 生效）时 StandardBias→DaylightBias
        let extra = if rc == 2 { tzi.DaylightBias as i64 } else { tzi.StandardBias as i64 };
        -((bias + extra) * 60) // 返回 local = utc + offset
    }
}

/// Linux：libc localtime_r 的 tm_gmtoff（含 DST，等价 GetTimeZoneInformation 语义）
#[cfg(not(windows))]
fn tz_bias_seconds() -> i64 {
    unsafe {
        let now: libc::time_t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

/// UTC - 本地 的偏移秒（UTC = local + offset；app.rs 录音文件名时间戳用）
pub fn tz_offset_secs() -> i64 {
    -tz_bias_seconds()
}

/// 高精度单调时钟（秒）
pub fn monotonic() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static T0: OnceLock<Instant> = OnceLock::new();
    let t0 = T0.get_or_init(Instant::now);
    t0.elapsed().as_secs_f64()
}

/// epoch 秒（time.time() 等价，用于录音时间戳）
pub fn epoch_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

/// 用系统默认方式打开 URL（https/http）；成功返回 true
#[cfg(windows)]
pub fn open_url(url: &str) -> bool {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    unsafe {
        let verb = wide("open");
        let u = wide(url);
        !ShellExecuteW(
            None,
            windows::core::PCWSTR(verb.as_ptr()),
            windows::core::PCWSTR(u.as_ptr()),
            windows::core::PCWSTR::null(),
            windows::core::PCWSTR::null(),
            SW_SHOWNORMAL,
        )
        .0
        .is_null()
    }
}

/// Linux：xdg-open（URL 与本地目录通用）
#[cfg(not(windows))]
pub fn open_url(target: &str) -> bool {
    match std::process::Command::new("xdg-open").arg(target).spawn() {
        Ok(_) => true,
        Err(e) => {
            crate::applog::error(&format!("xdg-open 失败（{}）: {}", target, e));
            false
        }
    }
}
