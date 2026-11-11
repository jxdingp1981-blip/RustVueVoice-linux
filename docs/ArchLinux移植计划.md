# RustVueVoice → Arch Linux 移植计划（详细说明）

> 基线：Windows exe `RustVueVoice.exe`（Tauri 2 + Vue 3，commit `165da5c 修改翻译键做为回车`）。
> 本文是**计划文档，未改动任何代码**。目标：在 Arch Linux 上实现一套功能对等的应用。

---

## 1. 总体

| 类别 | 模块 | 占比（行数口径约 5700 行后端） |
|---|---|---|
| ✅ 直接复用（平台无关） | msbc.rs、wav.rs、doubao.rs、volc_asr.rs、asr_engine.rs、creds.rs、app.rs 状态机/电池/云链路大部分、commands.rs 大部分、前端 Vue 全部 | ≈ 60% |
| 🔧 小改 | main.rs（AttachConsole/子系统属性）、config.rs（base_dir 语义）、util.rs（时区/UTF-16 辅助）、applog.rs（控制台段）、前端字体与文案 | ≈ 10% |
| ♻️ 整层重写（Windows 专属） | hid.rs、rawinput.rs、output.rs、overlay.rs、autostart.rs、browser_cookies.rs 的解密段 | ≈ 30% |

三个最大的差异点（决定计划形状）：

1. **输入注入**：Windows `SendInput(KEYEVENTF_UNICODE)` 在 Linux Wayland 上没有等价的"向任意应用打任意 Unicode"的跨桌面 API。用 **uinput（内核虚拟键盘）发 Ctrl+V 粘贴**作为全桌面兜底，X11 下可选 unicode 直打。产品本来就"识别文本写入剪贴板"，粘贴语义天然一致。
2. **悬浮胶囊 overlay**：GDI+ 分层窗口 → 重写为 **X11 窗口 + cairo/pango 绘制 + XShape 点击穿透**；Wayland 会话下自动经 **XWayland** 显示（GNOME/KDE 默认带 XWayland），不改架构。
3. **浏览器 cookie 收割**：Windows 的 DPAPI + AES-256-GCM → Linux Chromium 的 **PBKDF2("peanuts")+AES-128-CBC（v10）** 或 **keyring 密钥（v11）**。

系统层面需要新增 Windows 上不存在的东西：**udev 规则**（hidraw/evdev/uinput 权限）、**WebKitGTK 依赖**、**.desktop 入口与打包（PKGBUILD）**。

---

## 2. Windows 版功能

### 2.1 核心链路
- 接收器（VID `524D` / PID `4C52`，MI_00 厂商接口，UsagePage `0xFF02`）上：
  - 监听 HID 输入报告（65B = 报告ID 00 + 64B payload），识别 音频帧，取 57B 帧体；
- 云端 ASR 双引擎（热切换）：豆包 WSS（cookie 鉴权，累计全量文本，静默 1.2s/2.5s 判尾，auth 失败码 709599054）与火山引擎 `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async`（X-Api-Key 头 + 4B 二进制协议头 + gzip，seq 收尾负包）。PCM s16le/16k/mono、8192B 分块。
- 上屏：识别结果**恒写剪贴板**，再按 `output.input_mode`（默认 `unicode` 键入 / 可选 `ctrl_v` 粘贴）注入前台；悬浮层显示 Done 文案（show_final_seconds=1.2s）。
- 录音落盘：`recordings/YYYYMMDD_HHMMSS.wav`（16k/mono/16bit）+ 同名 `.txt`。
- 电池：`A5 5A 09 54 06 [电压LE] ff`，电压 10mV 小端，钳位 320–420 → (V-320)%；连接后即查 + 每 1800s 保底 + 连接成功/按键/主窗口聚焦触发（30s 节流）；存 `pct+1`（0=未知），UI 绿 ≥20% / 红 <20%。

### 2.2 登录与凭证
- **扫码登录**：应用内窗口（label `doubao-login`，520×680）加载 `https://www.doubao.com/chat`，每 2s 收割 cookie（含 httpOnly），SSO cookie 名 `sid_guard/sid_tt/uid_tt/SESSIONID/sessionid` 任一在场即成功，附 `ttwid` 与 `mcs.zijieapi.com/webid` 生成的 device_id/web_id；600s 超时；`--login` 模式登录完退出。
- **浏览器登录**：收割本机浏览器 doubao.com cookie（候选：pywebview EBWebView、exe 旁 WebView2、Edge、Chrome；SQLite `Network\Cookies`，`host_key LIKE '%doubao.com'`；DPAPI 解 `Local State os_crypt.encrypted_key`，v10=AES-256-GCM，v20 不支持）；失败则 `ShellExecuteW` 打开系统浏览器并 5s 轮询收割最长 300s。
- **凭证存储**：`~/.config/doubao-input/asr_params.json`（已是 XDG 风格路径，明文 JSON：cookies + device_id + web_id，tmp+rename 原子写）。

### 2.3 窗口/UI/系统集成
- 主窗口 1060×760（min 880×620）居中，深色主题；关闭即退出（无托盘）；单实例（二次启动唤起已有窗口）。
- 5 张状态卡（引擎/登录/接收器/电量/监听）+ 实时识别条 + 8 个按钮（扫码登录/浏览器登录/云链路测试/打开录音目录/使用说明/引擎设置/自启开关/退出）+ 运行日志面板（600 条上限、智能吸底、级别配色）+ 电量圆环。
- 悬浮胶囊：全屏置顶分层窗口、点击穿透、不抢焦点、不进任务栏；GDI+ 抗锯齿绘制（胶囊 560×128 圆角 18、24 根波形条 0xFF7FF2A6、底色 0xFF12141A、边 0xFF585A5E、Microsoft YaHei UI 15/22px、呼吸 Alpha 0.78~0.98 周期 2.4s、流光 hold 0.22s/decay 0.40s、60ms tick、文字截断 100/200 字、DPI 缩放）。
- 13 个 Tauri 命令：`get_status/start_login/start_browser_login/run_link_test/open_recordings_dir/get_autostart/set_autostart/exit_app/get_help/frontend_ready/get_asr_config/set_asr_config/open_url`；6 个事件：`status/login-done/test-done/partial/session/log`。
- 自启：写 `HKCU\...\Run`（值名 RustVueVoice = exe 路径）；查询/删除。
- config.json（exe 旁）：device.vid/pid/mi、keys.min_hold_ms=200/drain_ms=300、asr.engine= doubao|volc + volc.api_key/resource_id/url/model_name/enable_*/end_window_size=800、output.input_mode/show_final_seconds=1.2、paths.recordings_dir/logs；JSON 顶层浅合并、tmp+rename 原子写。
- 日志：exe 旁 `logs/rustvuevoice.log` 追加 + GUI emit + 控制台。
- CLI 子命令：`--check`（自检，找不到设备 exit 2）、`--probe <wav>`、`--login`、`--import-browser`、`--overlay-demo`、`--help`。

---

## 3. 架构策略

不重写业务层，只做**平台层抽换**。在 `src-tauri/src/` 下新增 platform 抽象（trait + `#[cfg]` 目录）：

```
platform/
  mod.rs        // trait 定义 + 工厂（按 target_os 选择实现）
  windows/      // 现有 5 个 Win32 模块迁入（hid.rs rawinput.rs output.rs overlay.rs autostart.rs…）
  linux/        // 新实现
```

建议的 trait 边界（对应现有调用点）：

| trait | Windows 现实现 | Linux 实现 |
|---|---|---|
| `HidLink`（open/find/read/write/stop） | hid.rs（SetupDi+CreateFile+overlapped） | hidraw + udev（或 hidapi crate） |
| `KeySource`（(vk,down) 流） | rawinput.rs（WM_INPUT） | evdev |
| `InputSink`（type_text/paste/press_enter） | output.rs（SendInput） | uinput / XTEST / xdotool·wtype |
| `TextClipboard`（set/get） | output.rs 内嵌（Win32 剪贴板） | arboard(X11) + wl-clipboard-rs(Wayland) |
| `OverlayPresenter`（OverlayEvent mpsc 接口不变） | overlay.rs（GDI+ 分层窗） | x11rb + pangocairo + XShape |
| `Opener`（open_url/open_dir） | ShellExecuteW | xdg-open |
| `AutostartStore`（is/set） | 注册表 HKCU Run | XDG autostart .desktop |
| `TzBias`/时间 | GetTimeZoneInformation | chrono |

app.rs 的状态机、命令帧常量、电池算法、finalize 流程、commands.rs 的命令签名、Tauri 事件名与 payload **全部保持不变**（前端零逻辑改动）。

---

## 4. 逐功能点移植方案（F1–F16）

### F1 HID 语音通道（hid.rs → hidraw）【重写】
- **枚举**：用 `udev` crate 枚举 `subsystem=hidraw`，读父设备属性 `ID_VENDOR_ID/ID_PRODUCT_ID`（或 sysfs `../../uevent`）匹配 `524D/4C52`，并要求接口号 = config `device.mi`（sysfs 设备名形如 `0003:524D:4C52.000K`，其父 usb 接口 `bInterfaceNumber` = "00"）。排除逻辑对齐现版"路径含 VID_524D&PID_4C52 且含 MI_00 且不含 COL"。
- **UsagePage 校验**：`ioctl(HIDIOCGRDESC)` 解析 report descriptor 中的 Global Usage Page = `0xFF02`；或改用 `hidapi` crate（`DeviceInfo::usage_page()` 直接给出，代码量最小，推荐先走这条）。
- **打开/读**：hidraw 节点阻塞 `read()` 放专用线程（模型对齐现 hid-reader 线程；无 overlapped，无需 CreateEvent/WaitForSingleObject/CancelIoEx）。拔出 → read 返回错误/EIO → 关闭 → 2s 重连（对齐现版）。500ms 超时可用 `O_NONBLOCK`+poll 替代，便于 stop。
- **report-id 归一化（关键坑）**：Windows 读缓冲恒带 1 字节报告 ID（00）在前；Linux hidraw 对"无编号报告"读出的数据**不带** ID 字节（首字节即 payload）。移植时在读入口做归一：若首字节为 0x00 且次字节非 0x9D/A5 → 视为 ID 剥离失败场景，统一成 `payload = 归一后的 64B` 再进 `on_hid_packet`。解析侧沿用现有"尝试 offset 0/1"的容忍逻辑。
- **写**：hidraw `write()` 缓冲首字节必须是报告号；无编号报告填 0x00 —— 与现命令帧首字节 0x00 完全一致，65B（帧 + 补零）原样写入即可。写超时用 poll + O_NONBLOCK 或独立 fd（hidraw 同一 fd 可读写）。
- **权限（系统级，见 §6）**：`/dev/hidraw*` 默认 root 0660，需 udev 规则 `TAG+="uaccess"`（logind 授给活跃会话用户），插拔一次或 `udevadm trigger` 生效。
- **内核侧无冲突**：接收器是 generic HID，`hid-generic` 自动绑定并出 hidraw 节点；Linux 无官方驱动，不存在"Rulink.exe 抢通道"问题。蓝牙模式（HFP/PipeWire）不在支持范围，与 Windows 版一致（仅 2.4G）。
- **验收**：`--check` 找到设备。

### F2 按键监听（rawinput.rs → evdev）【重写】
- **枚举**：udev 枚举 `subsystem=input, subsystem-devnode=event*`，匹配父设备 `ID_VENDOR_ID=524d / ID_PRODUCT_ID=4c52`；接收器复合设备会出多个 evdev 节点（鼠标/键盘/consumer），语音键在**键盘接口**（MI_01）节点上。
- **键值映射**：Windows VK 129/130/131 = F18/F19/F20 ↔ Linux evdev `KEY_F18=188 / KEY_F19=189 / KEY_F20=190`（input-event-codes.h，高置信对应；实施第一步用 `evtest` 实测鼠标上报的 keycode 并固化映射表，可放进 config）。
- **读取**：`evdev` crate（阻塞 poll 循环线程）过滤 `EV_KEY` code ∈ {188,189,190}，`value 1/0` ↔ down/up；**不 EVIOCGRAB**（对齐 RIDEV_INPUTSINK：只听不拦）。只接受来自 524D 设备节点的事件（对齐现版设备路径过滤）。
- **权限**：udev 规则对该设备的 input 节点 `uaccess`（或用户加入 `input` 组）。
- **验收**：X11 与 Wayland 下长按/松开/双击手势全部进入 key_worker；普通键盘按 F19 不触发（设备过滤生效）。

### F3 文字上屏 + 剪贴板（output.rs → 注入层）【重写，风险最高】
- **剪贴板写入**（产品恒定行为）：
  - X11：`arboard`（x11rb 后端；进程常驻持有剪贴板所有权，满足"识别文本可粘贴"）。
  - Wayland：`wl-clipboard-rs`（自身成为 source 并 fork 持有者线程）。
  - 统一 `TextClipboard` trait，按 `XDG_SESSION_TYPE`/`WAYLAND_DISPLAY` 选择。
- **注入（InputSink）三后端**：
  1. **uinput（主后端，全桌面通用）**：进程内直接 `ioctl(UI_SET_EVBIT/KEYBIT)` 创建虚拟键盘，注入 `Ctrl+V`（paste）与 `Enter`。只依赖 `/dev/uinput` 可写（udev 规则 + input 组），**无外部守护进程**，X11/KDE-Wayland/GNOME-Wayland/wlroots 全部生效（内核级注入，合成器当物理键盘）。对齐现版 ctrl_v 模式与 `press_enter`。
  2. **X11 XTEST（x11rb）**：Ctrl+V / Enter 直发；unicode 直打首版可调用 `xdotool type --delay 10`（用户可选装），或后续自实现 XKB 重映射（对应 KEYEVENTF_UNICODE 的 32 键/批、10ms 间隔节奏参数保留到后端配置）。
  3. **wtype / ydotool（可选兜底）**：wlroots 系下 `wtype` 支持 Unicode 直打；检测到可用二进制才启用。
- **input_mode 语义映射**（config 不变，行为落地）：
  - `unicode` → auto：有可用的打字后端（X11+xdotool / wtype）则逐字打；否则**自动降级为剪贴板+Ctrl+V**并在日志说明（最终上屏效果一致，因文本本就进了剪贴板）。
  - `ctrl_v` → uinput/XTEST 粘贴。
  - F18 回车 → uinput `KEY_ENTER` / XTEST `XK_Return`。
- **粘贴的个位数毫秒节奏**：Ctrl down → V down → V up → Ctrl up，10ms 间隔对齐现版；uinput 需 `UI_SET_...` 后 `write(EV_SYN)`。
- **验收**：记事本类目标（gedit/kate/浏览器地址栏/微信 Electron 输入框）在 X11 与两种 Wayland 桌面下上屏成功；中文含标点/换行正确。

### F4 悬浮胶囊（overlay.rs → x11rb+pangocairo）【重写，工作量最大】
- **保留接口**：`OverlayEvent` 枚举（Show/Hide/Status/Partial/Level/Done/Cancel/Recognizing）、`OverlayHandle::post`、mpsc、overlay 线程模型、60ms tick（glib timeout 或线程内自绘循环）、seq 代际号防串台、Done/Cancel 延时隐藏 —— 全部不变，只换窗口与绘制底座。
- **窗口**：X11（Wayland 会话自动经 XWayland）：`override_redirect=true`（等价 WS_POPUP+TOPMOST，不受 WM 摆布）、窗口尺寸=主屏（对齐 GetSystemMetrics(SM_CXSCREEN/CYSCREEN)；多屏增强：XRandR 取焦点屏中心）、`XShape` 输入区域置空 = 点击穿透（等价 WS_EX_TRANSPARENT）、不设 WM_HINTS input = 不抢焦点（等价 WS_EX_NOACTIVATE）、不设 `_NET_WM_STATE` 进任务栏集合（等价 TOOLWINDOW）。置顶用 OR + 可选 `_NET_WM_STATE_ABOVE` 兜底。
- **绘制**：32bpp ARGB visual + cairo；胶囊圆角多边形（填充 #12141A、描边 #585A5E 1.5px）、24 根波形条 #7FF2A6、pango/pangocairo 文本（**字体 Noto Sans CJK SC** 15/22px 替代 Microsoft YaHei UI；`GdipMeasureString` 的向下扩高逻辑用 pango layout 高度复刻）；`MeasureString`→`pango_cairo_layout` 尺寸。
- **呼吸/流光**：全局 alpha 0.78~0.98（2.4s 周期）、流光 hold 0.22s/decay 0.40s 参数原样保留（cairo `set_source_rgba`）。
- **DPI**：读 `Xft.dpi`/`GDK_SCALE` 做全局 scale（替代 GetDpiForSystem），首版提供 config 缩放系数。
- **备选增强（不做为第一期目标）**：wlroots 下走 layer-shell（gtk-layer-shell）原生 Wayland overlay。
- **验收**：`--overlay-demo` 在 X11 KDE/GNOME、Wayland KDE/GNOME/Hyprland 下：置顶、点击穿透（下层可操作）、不抢输入焦点、动画与文字与 Windows 版观感一致；双击隐藏/再显。

### F5 主窗口 / 登录窗口（Tauri → WebKitGTK）【小改】
- Tauri 2 在 Linux = GTK3 + **webkit2gtk-4.1**（Arch 包名 `webkit2gtk-4.1`）。主窗口/登录窗口创建代码、尺寸、min size、center、单实例插件（Linux 走 DBus）**零改动**。
- **登录 cookie 收割**：`WebviewWindow::cookies_for_url` 在 wry 的 Linux 实现走 WebKitCookieManager（含 httpOnly）。**列入第一周验证点**；若该 API 在 Linux 缺失/不完整，备选：`with_webview` 拿 webkit2gtk 原生 `WebKitCookieManager::get_all_cookies`（异步），再按域名过滤 —— 二选一即可，收割逻辑（SSO cookie 名集合、ttwid、webid）不变。
- capabilities/default.json 保持（登录窗口不调 IPC）。
- **前端零逻辑改动**，仅两处表现层：字体栈补 `Noto Sans CJK SC`（style.css）；UI 文案改路径表述（`logs\rustvuevoice.log` → `logs/rustvuevoice.log`、注册表表述 → XDG autostart、`%USERPROFILE%` → `~/.config`），Rust 端 HELP_TEXT 同步。

### F6 浏览器 cookie 收割（browser_cookies.rs）【重写解密段】
- **路径候选（Linux 版）**：`~/.config/google-chrome`、`~/.config/chromium`、`~/.config/microsoft-edge`、（可选 Brave `~/.config/BraveSoftware/Brave-Browser`）；Profile 选 `Default` 或最近修改的 `Profile *`；库文件 `Network/Cookies`（旧版 `Cookies`）。pywebview/EBWebView 候选删除。运行中锁库 → 复制到 `/tmp/vm_cookies_{pid}.db` 再只读打开（逻辑不变）。
- **解密**：
  - `v10`：AES-128-CBC，key = PBKDF2-HMAC-SHA1(pass="peanuts", salt="saltysalt", iter=1, dklen=16)，IV = 16×0x20，密文剥 3B 前缀（新增 RustCrypto 依赖：`aes`+`cbc`+`pbkdf2`+`sha1`；PKCS7 去填充）。
  - `v11`：同上算法，但 key 取自 keyring 条目 `Chrome Safe Storage` / `Chromium Safe Storage` / `Brave Safe Storage`（GNOME: Secret Service —— `secret-service` 或 `keyring` crate；KDE: KWallet —— keyring crate v3 的 kwallet 后端或提示改用扫码登录）。首版支持 Secret Service，KWallet 列为第二期。
  - 无前缀旧值：Linux 上不存在 DPAPI 场景，直接按明文/报错处理。
  - `v20` App-Bound：与 Windows 版一致，明确不支持（报错提示走扫码登录）。
- `fetch_webid()`（mcs.zijieapi.com）平台无关，原样保留。
- **验收**：Chrome/Chromium Linux 版登录 doubao.com 后 `--import-browser` 成功收割；浏览器运行中也能收割（temp copy 生效）。

### F7 凭证存储（creds.rs）【基本零改】
- 路径 `~/.config/doubao-input/asr_params.json` 已是 XDG 语义，代码不用改。**新增小改进**：Linux 下落盘后 `chmod 600`（cookies 明文）。

### F8 配置（config.rs）【小改】
- **base_dir 语义决策**：Windows = exe 所在目录。Linux 改为：
  - 默认 **XDG 化**：config → `~/.config/rustvuevoice/config.json`；数据（recordings/logs）→ `~/.local/share/rustvuevoice/`；
  - 保留**便携模式**：若 exe 同目录存在 `config.json`，则整体沿用 exe 目录（绿色部署兼容）。
  - 原因：exe 若装在 `/usr/bin` 不可写，原语义必然失败。
- 其余键、默认值、浅合并、原子写全部不变。

### F9 开机自启（autostart.rs）【重写】
- 写 `~/.config/autostart/rustvuevoice.desktop`（`Exec=/abs/path/RustVueVoice`、`Type=Application`、`X-GNOME-Autostart-enabled=true`、Hidden=false）；`is_enabled`=文件存在且非 Hidden；关闭=删文件。X11/Wayland 通用（GNOME/KDE 均支持 XDG autostart）。
- UI 文案与日志同步改为「写入 ~/.config/autostart」。

### F10 命令与事件（commands.rs / app.rs）【小改】
- `open_url` / `open_recordings_dir`：`ShellExecuteW` → `std::process::Command::new("xdg-open")`；http(s) 白名单校验保留。
- `get_status` 等其余 11 个命令、6 个事件 payload **不变**。
- app.rs 仅两处：`ShellExecuteW`（浏览器登录打开 doubao.com）→ xdg-open；`tz_offset_secs`（文件名时间戳）→ `chrono::Local`。状态机/命令帧/电池/链路测试/引擎切换逻辑不动。

### F11 入口与控制台（main.rs / applog.rs）【小改】
- 删 `AttachConsole`/`GetStdHandle`/`WriteConsoleW`（Linux 子命令天然打印到 stdout）；`#![windows_subsystem]` 加 `#[cfg(windows)]`。
- `--check/--probe/--login/--import-browser/--overlay-demo/--help` 全部保留，行为一致（`--check` 找不到设备 exit 2）。
- 日志文件追加逻辑不变；路径随 F8 的 data 目录。

### F12 util.rs【小改】
- `wide()/pcw()` 删除或 cfg(windows)；`tz_bias_seconds` → chrono；`local_time_str/monotonic/epoch_now` 不变。

### F13 云端 ASR 与解码（doubao.rs / volc_asr.rs / msbc.rs / wav.rs / asr_engine.rs）【零改动】
- 纯 Rust + tokio-tungstenite + native-tls（Linux 落 OpenSSL，Arch 基本系统自带）。mSBC 定点解码器、WAV RIFF、AsrSession trait 全部原样。唯一工程动作是 Cargo.toml 的 `windows` 依赖加 `#[cfg(windows)]` 的 target 限定（`[target.'cfg(windows)'.dependencies]`）。

### F14 单实例 / 窗口生命周期【零改动】
- `tauri-plugin-single-instance` Linux 端用 DBus，直接可用；关窗即退出（prevent_close + shutdown + destroy overlay + exit(0)）流程不变（overlay destroy 走新 OverlayPresenter）。

### F15 系统配套（Linux 新增，Windows 没有的部分）
1. **udev 规则**（随包安装 `/usr/lib/udev/rules.d/70-rustvuevoice.rules`）：
   ```
   SUBSYSTEM=="hidraw", ATTRS{idVendor}=="524d", ATTRS{idProduct}=="4c52", TAG+="uaccess"
   SUBSYSTEM=="input",  ATTRS{idVendor}=="524d", ATTRS{idProduct}=="4c52", TAG+="uaccess"
   KERNEL=="uinput", MODE="0660", GROUP="input", TAG+="uaccess"
   ```
   （uaccess 由 logind 授予活跃座席用户；`udevadm control --reload && udevadm trigger` 生效。）
2. **.desktop 入口 + 图标**（保证 Wayland 下窗口归属/图标正确，WMStartupID）：`/usr/share/applications/rustvuevoice.desktop` + hicolor 图标（icons/ 已有 png 可复用）。
3. **依赖声明**（详见 §6）。
4. **打包**：`bundle.active=false` 保持"单二进制 + 说明"分发方式可行；建议补 **PKGBUILD**（装 bin + rules + desktop + icon）与可选 AppImage。release 剥离/panic=abort 设置不变。

### F16 文档与帮助文本
- README/使用说明/HELP_TEXT 同步：WebView2 → WebKitGTK；注册表 → XDG autostart；路径与控制台命令（evtest、groups、udevadm）；F18 行为修正为"发送回车"（修复现有文档-实现不一致）。

## 5. 显示服务器支持矩阵

| 功能 | X11（KDE/GNOME/i3…） | KDE Wayland | GNOME Wayland | wlroots（Hyprland/sway…） |
|---|---|---|---|---|
| HID 音频/命令/电池 | ✅ | ✅ | ✅ | ✅ |
| 语音键监听（evdev） | ✅ | ✅ | ✅ | ✅ |
| 主窗口/登录窗口 | ✅ | ✅ | ✅ | ✅ |
| 剪贴板写入 | ✅ arboard | ✅ wl-clipboard-rs | ✅ | ✅ |
| Ctrl+V 粘贴 / Enter 注入 | ✅ XTEST 或 uinput | ✅ uinput | ✅ uinput | ✅ uinput |
| Unicode 逐字直打 | ✅（xdotool/XKB，可选） | ⚠️ wtype（视 KWin 实测） | ❌ → 自动降级粘贴 | ✅ wtype |
| 悬浮胶囊置顶+穿透 | ✅ 原生 X11 | ✅ 经 XWayland | ✅ 经 XWayland | ✅ 经 XWayland（layer-shell 为可选增强） |
| 浏览器 cookie（v10/v11） | ✅ | ✅（v11 需 KWallet，二期） | ✅（v11 走 Secret Service） | ✅ |
| 自启 | ✅ | ✅ | ✅ | ✅（需该桌面处理 XDG autostart） |

第一期承诺：X11 全功能 + Wayland 全功能（unicode 直打在 GNOME Wayland 自动降级为粘贴，其余不变）。

---

## 6. Arch 依赖与安装面

- **构建**：`base-devel`、`rust`、`nodejs`、`npm`、`webkit2gtk-4.1`（Tauri Linux 必需）。
- **运行**：`gtk3`、`pango`、`cairo`、`libx11`、`libxrandr`、`libxfixes`、`libxext`（XShape）、`openssl`、`dbus`（单实例/Secret Service）、`systemd-libs`（libudev）、**`noto-fonts-cjk`**（界面与胶囊中文字体）。
- **可选**：`xdotool`（X11 unicode 直打）、`wtype`（wlroots 直打）、`wl-clipboard`（调试）、`evtest`（排查按键）。
- **新增 Rust crate**（Linux 侧）：`udev`、`hidapi`（或纯 hidraw 自研，二选一）、`evdev`、`x11rb`、`arboard`、`wl-clipboard-rs`、`aes`/`cbc`/`pbkdf2`/`sha1`、`secret-service`（或 keyring）、`chrono`、`nix`（uinput/hidraw ioctl）。
- **用户一次性动作**：安装 udev 规则（包内自动）→ 重插接收器；自研 uinput 后端需要 `/dev/uinput` 可写（规则已给 uaccess/input 组）。

---

## 7. 实施阶段（P0–P8，每阶段有验收）

| 阶段 | 内容 | 验收标准 |
|---|---|---|
| **P0 骨架** | cfg 平台目录拆分；Cargo 依赖 target 化；Linux 下编译通过、Tauri 空窗口起、前端渲染（webkit2gtk-4.1）；字体/文案修正 | `cargo tauri dev` 出主界面，日志面板活着 |
| **P1 HID** | F1 hidraw 层 + udev 规则；`--check`；电量查询/解析（含 report-id 归一化回归帧） | `--check` exit 0；电量帧 `a55a0954069901ff`→89%；8s 监听见音频流 |
| **P2 按键** | F2 evdev；evtest 实测键值；手势状态机接通（START/STOP 命令下发） | 长按→`CMD_START_RECORD` 写入成功且 HID 流出现音频；双击/误触手势正确 |
| **P3 语音链路** | mSBC→PCM→WAV + doubao/volc 联调（代码零改，主要是环境验证：OpenSSL 握手、分块节拍） | 说话→`partial` 事件→finish 全文；wav/txt 落盘 |
| **P4 上屏** | F3 剪贴板双后端 + uinput/XTEST 注入 + input_mode 语义与降级 | X11/两 Wayland 桌面中文上屏、换行正确；剪贴板恒有文本 |
| **P5 悬浮层** | F4 x11rb+pangocairo 重写，事件接口不变 | `--overlay-demo` 全桌面观感对齐；穿透/置顶/不抢焦点 |
| **P6 登录** | F5 cookies_for_url 验证（含备选实现）；F6 Linux 浏览器 cookie 解密（v10 第一期、v11 Secret Service） | 扫码登录成功；`--import-browser` 从 Chrome/Chromium 收割成功；auth 失败→自动提示重登 |
| **P7 集成** | F8 XDG/便携 base_dir；F9 autostart.desktop；F10 xdg-open；F11/F12 清理；PKGBUILD/.desktop/icon | 全部 13 个命令行为一致；`--login` 登录即退；自启生效 |
| **P8 矩阵测试** | §5 矩阵全格跑一遍 + 热插拔（拔接收器 5s 重连）、HiDPI、多屏、长录音、官方日志回归帧、单实例、关窗退出 | 测试清单全绿；打包产物可装可卸 |

---

## 8. 风险与待实测清单（按优先级）

1. **wry `cookies_for_url` 的 Linux 实现完整性**（P6 第一周验证；备选 webkit2gtk CookieManager 原生 API）。
2. **鼠标 evdev 实际 keycode**（可能是 KEY_F18/19/190，也可能走 consumer 页；evtest 实测后固化映射）。
3. **uinput 权限模型**：GNOME Wayland 会话的 uaccess 对 /dev/uinput 生效性；不生效则回退 `GROUP="input"` + 用户组方案（安装文档写明）。
4. **XWayland overlay 的置顶层级**（GNOME 对 override-redirect 窗口的 stacking 策略；KDE 无碍；实测 + 必要时加 _NET_WM_STATE_ABOVE 兜底）。
5. **Chrome v11/keyring 变体**（KWallet 二期；首版文案提示"KDE 下若收割失败请用扫码登录"）。
6. **WebKitGTK 对前端构建目标（chrome105/Tailwind v4/reka-ui）的兼容**（Tauri Linux 官方支持路径，风险低，P0 验证）。
7. **hidraw 读侧 report-id 归一化**与现有 offset 0/1 容忍逻辑的组合回归（用官方帧样本建单测）。
8. **剪贴板所有权**：Wayland 下 wl-clipboard-rs 需持有 source（进程常驻 OK，注意退出时机）。
9. **多字节节奏**：KEYEVENTF_UNICODE 32键/批·10ms 与 xdotool --delay 的等效性（观感差异可接受）。
10. **仓库卫生**：`Release/dist/.../config.json` 含真实火山 API Key，任何打包/迁移动作前先清除（与本移植无直接关系，但分发时必查）。

---

## 9. 明确不需要改动的清单

- `msbc.rs`（mSBC 解码）、`wav.rs`、`doubao.rs`（WSS 协议）、`volc_asr.rs`（二进制协议）、`asr_engine.rs`（trait）、`creds.rs`（XDG 路径天然匹配）
- `app.rs` 的状态机/命令帧常量/电池算法/云链路测试/引擎热切换；`commands.rs` 命令签名与 6 个事件名/payload
- 前端全部组件逻辑（App.vue、StatusCard、BatteryGauge、LogPanel、EngineSettingsDialog、HelpDialog、store.ts、capabilities）
- Tauri 窗口尺寸/标题/单实例/关窗即退出行为

## 10. 附：Windows API → Linux 对应速查

| Windows | Linux/Arch | 所在功能 |
|---|---|---|
| SetupDi* 枚举 HID | udev 枚举 hidraw / hidapi | F1 |
| CreateFileW + overlapped ReadFile/WriteFile | hidraw read/write + poll | F1 |
| HidD_GetAttributes / HidP_GetCaps | udev 属性 + HIDIOCGRDESC（或 hidapi DeviceInfo） | F1 |
| RegisterRawInputDevices + WM_INPUT | evdev 读 /dev/input/event* | F2 |
| SendInput KEYEVENTF_UNICODE | xdotool/wtype（X11/wlroots）→ 降级粘贴 | F3 |
| SendInput VK_CONTROL+V / VK_RETURN | uinput（全桌面）或 XTEST（X11） | F3 |
| OpenClipboard/SetClipboardData CF_UNICODETEXT | arboard（X11）/ wl-clipboard-rs（Wayland） | F3 |
| GDI+ 分层窗 + UpdateLayeredWindow + WS_EX_TRANSPARENT | X11 ARGB 窗 + cairo/pango + XShape 空输入区（经 XWayland） | F4 |
| GetDpiForSystem | Xft.dpi / GDK_SCALE | F4 |
| WebView2 + ICoreWebView2CookieManager | WebKitGTK + WebKitCookieManager | F5 |
| CryptUnprotectData + AES-256-GCM(v10) | PBKDF2("peanuts")+AES-128-CBC(v10) / keyring(v11) | F6 |
| HKCU\...\Run | ~/.config/autostart/*.desktop | F9 |
| ShellExecuteW open | xdg-open | F10 |
| AttachConsole/WriteConsoleW | stdout（天然） | F11 |
| GetTimeZoneInformation | chrono::Local | F12 |
| tauri-plugin-single-instance(DBus on Linux) | 不变 | F14 |
