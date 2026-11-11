// autostart_linux.rs — XDG autostart 实现（Arch 移植）
// 写 ~/.config/autostart/rustvuevoice.desktop：
//   is_enabled = 文件存在且非 Hidden=true
//   set(true)  = 写入当前 exe 绝对路径（已开启但 exe 移动过也会顺带刷新，与 Windows 版一致）
//   set(false) = 删除文件（不存在视为已关闭，返回 true）
#![allow(dead_code)]

use std::path::PathBuf;

const DESKTOP_NAME: &str = "rustvuevoice.desktop";

fn desktop_path() -> PathBuf {
    let config_home = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from(".config"));
    config_home.join("autostart").join(DESKTOP_NAME)
}

/// 当前是否已注册开机自启
pub fn is_enabled() -> bool {
    let path = desktop_path();
    if !path.is_file() {
        return false;
    }
    // Hidden=true 视为关闭（XDG 语义）
    match std::fs::read_to_string(&path) {
        Ok(text) => !text.lines().any(|l| {
            let l = l.trim();
            l.eq_ignore_ascii_case("Hidden=true")
        }),
        Err(_) => false,
    }
}

/// 启用/停用开机自启。启用时写入当前 exe 路径。
pub fn set_enabled(on: bool) -> bool {
    if !on {
        let path = desktop_path();
        match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        }
    } else {
        enable()
    }
}

fn enable() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let path = desktop_path();
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            crate::applog::error(&format!("创建 autostart 目录失败: {}", e));
            return false;
        }
    }
    let content = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=RustVueVoice\n\
         Comment=Rulink 语音鼠标 · 语音打字\n\
         Exec=\"{}\"\n\
         Terminal=false\n\
         Hidden=false\n\
         X-GNOME-Autostart-enabled=true\n\
         Categories=Utility;\n\
         Icon=rustvuevoice\n",
        exe.display()
    );
    match std::fs::write(&path, content) {
        Ok(()) => {
            crate::applog::info(&format!("开机自启已写入 {}", path.display()));
            true
        }
        Err(e) => {
            crate::applog::error(&format!("写入 autostart 失败: {}", e));
            false
        }
    }
}
