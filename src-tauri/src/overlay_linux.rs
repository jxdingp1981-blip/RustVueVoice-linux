// overlay_linux.rs — 悬浮胶囊的 Linux 实现：x11rb ARGB 窗口 + cairo 绘制 + XShape 点击穿透
//
// 窗口语义等价 Windows 分层窗口：
//   override_redirect=1     ← WS_POPUP + TOPMOST（不受 WM 摆布，天然置顶）
//   XShape input 区域置空   ← WS_EX_TRANSPARENT（点击穿透，下层可操作）
//   OR 窗口不获焦           ← WS_EX_NOACTIVATE（不抢输入焦点）
//   不进任务栏集合          ← WS_EX_TOOLWINDOW（OR 窗口本就不进）
// Wayland 会话经 XWayland 显示（GNOME/KDE 默认带 XWayland），不改架构。
//
// 绘制：cairo ImageSurface（ARGB32）逐元素乘呼吸 alpha（等价 GDI+
// SourceConstantAlpha），PutImage 直传窗口（x86 小端下 BGRA 与 ZPixmap 对齐）。
// 状态机（Show/Hide/Partial/Level/Done/Cancel/Recognizing、seq 代际号、流光、
// 多行扩高顶部固定）与 Windows 版逐字对齐；文字用 cairo toy API + Noto Sans CJK SC。
#![allow(dead_code)]

use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cairo::{Context, FontSlant, FontWeight, ImageSurface};
use x11rb::connection::Connection;
use x11rb::protocol::shape::ConnectionExt as _;
use x11rb::protocol::shape::{SK, SO};
use x11rb::protocol::xproto::{
    ColormapAlloc, ConnectionExt as XcbExt, CreateGCAux, CreateWindowAux, EventMask, ImageFormat,
    Visualtype, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use super::{
    BORDER_COLOR, BREATH_PERIOD_S, CAPSULE_FILL, CORNER_PX, GLINT_DECAY_S, GLINT_HOLD_S,
    GLINT_PEAK, OPACITY_BREATH_MAX, OPACITY_BREATH_MIN, OVERLAY_HEIGHT, OVERLAY_WIDTH,
    STATUS_COLOR, STATUS_ERR_COLOR, STATUS_OK_COLOR, STATUS_Y, TEXT_COLOR, TEXT_KEEP_TAIL,
    TEXT_Y, TICK_MS, WAVE_BAR_GAP, WAVE_BAR_MAX_H, WAVE_BAR_W, WAVE_BARS, WAVE_COLOR, WAVE_CY,
    OverlayEvent,
};
use crate::applog;

const FONT_FAMILY: &str = "Noto Sans CJK SC";

// ---------------- 句柄 / 全局 ----------------

/// 句柄：任意线程 post 事件
#[derive(Clone)]
pub struct OverlayHandle {
    tx: Sender<OverlayEvent>,
}

impl OverlayHandle {
    pub fn post(&self, ev: OverlayEvent) {
        let _ = self.tx.send(ev); // Linux 线程 60ms 轮询消费，无需唤醒机制
    }
}

struct OverlayShared {
    rx: Mutex<Option<Receiver<OverlayEvent>>>,
}

static OVERLAY: OnceLock<OverlayShared> = OnceLock::new();
static CLOSED: AtomicBool = AtomicBool::new(false);

/// 在独立线程创建悬浮层并运行其绘制循环。返回事件句柄，任意线程可 post。
pub fn start_overlay_thread() -> OverlayHandle {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<OverlayHandle>();
    std::thread::Builder::new()
        .name("overlay".into())
        .spawn(move || {
            CLOSED.store(false, Ordering::SeqCst);
            let handle = create_overlay();
            let _ = ready_tx.send(handle);
            run_loop();
            applog::info("悬浮层线程已退出");
        })
        .ok();
    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| {
            applog::error("悬浮层线程未在 5 秒内就绪，悬浮层不可用（录音功能不受影响）");
            OverlayHandle { tx: channel().0 }
        })
}

/// 初始化事件通道（X 窗口创建在 run_loop 线程内完成）
pub fn create_overlay() -> OverlayHandle {
    let (tx, rx) = channel::<OverlayEvent>();
    OVERLAY.get_or_init(|| OverlayShared { rx: Mutex::new(Some(rx)) });
    OverlayHandle { tx }
}

/// 请求关闭悬浮层（线程安全；绘制循环 ≤60ms 内退出；进程退出时 X 连接关闭，
/// XWayland 会回收全部窗口，无残留）
pub fn destroy_overlay() {
    CLOSED.store(true, Ordering::SeqCst);
}

// ---------------- 状态机（与 Windows 版 drain_event 对齐） ----------------

struct OverlayState {
    visible: bool,
    recording: bool,
    t0: Instant,
    text: String,
    status: String,
    status_color: u32,
    levels: Vec<f64>,
    glint: Option<Instant>,
    breath_epoch: Option<Instant>,
    seq: u64,
    /// 定时隐藏：(代际号, 期限)；主循环到期且 seq 未变时隐藏
    pending_hide: Option<(u64, Instant)>,
    mapped: bool,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            visible: false,
            recording: false,
            t0: Instant::now(),
            text: String::new(),
            status: String::new(),
            status_color: STATUS_COLOR,
            levels: vec![0.0; WAVE_BARS],
            glint: None,
            breath_epoch: None,
            seq: 0,
            pending_hide: None,
            mapped: false,
        }
    }
}

fn st_lock() -> std::sync::MutexGuard<'static, OverlayState> {
    static STATE: OnceLock<Mutex<OverlayState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(OverlayState::default())).lock().unwrap()
}

/// 一个事件的处理（照 Windows drain_event 语义）
fn drain_event(ev: OverlayEvent) {
    let mut st = st_lock();
    match ev {
        OverlayEvent::Show => {
            st.status = "正在录音 · 松开结束".into();
            st.status_color = STATUS_COLOR;
            st.text.clear();
            st.levels = vec![0.0; WAVE_BARS];
            st.glint = None;
            st.visible = true;
            st.seq += 1;
            st.t0 = Instant::now();
            st.recording = true;
        }
        OverlayEvent::Hide => {
            st.recording = false;
            st.visible = false;
            st.pending_hide = None;
        }
        OverlayEvent::Status(s) => {
            st.status = s;
            st.status_color = STATUS_COLOR;
        }
        OverlayEvent::Partial(t) => {
            let grew = t.chars().count() > st.text.chars().count();
            st.text = t;
            let cnt = st.text.chars().count();
            if cnt > TEXT_KEEP_TAIL {
                let tail: String = st.text.chars().skip(cnt - TEXT_KEEP_TAIL).collect();
                st.text = format!("…{}", tail);
            }
            if grew && st.visible {
                st.glint = Some(Instant::now());
            }
        }
        OverlayEvent::Recognizing => {
            st.recording = false;
            st.status = "识别中…".into();
            st.status_color = STATUS_COLOR;
        }
        OverlayEvent::Level(v) => {
            let v = v.clamp(0.0, 1.0);
            st.levels.push(v);
            if st.levels.len() > WAVE_BARS {
                st.levels.remove(0);
            }
        }
        ev @ (OverlayEvent::Done(..) | OverlayEvent::Cancel(..)) => {
            let (text, hide_ms, is_done) = match ev {
                OverlayEvent::Done(t, ms) => (t, ms, true),
                OverlayEvent::Cancel(t, ms) => (t, ms, false),
                _ => unreachable!(),
            };
            st.recording = false;
            if !st.visible {
                st.visible = true;
                st.seq += 1;
            }
            st.status = if is_done { "✓ 已输出".into() } else { "✗ 已取消".into() };
            st.status_color = if is_done { STATUS_OK_COLOR } else { STATUS_ERR_COLOR };
            // 完整结果可能很长：最多保留尾部 200 字符，再多截断
            let cnt = text.chars().count();
            st.text = if cnt > TEXT_KEEP_TAIL * 2 {
                let tail: String = text.chars().skip(cnt - TEXT_KEEP_TAIL * 2).collect();
                format!("…{}", tail)
            } else {
                text
            };
            st.glint = None;
            let seq = st.seq;
            // 定时隐藏（新一轮 Show 会 ++seq 使其失效）——主循环轮询到期
            st.pending_hide =
                Some((seq, Instant::now() + Duration::from_millis(hide_ms as u64)));
        }
    }
}

// ---------------- 绘制循环（X 连接持有于本线程） ----------------

struct XState {
    conn: RustConnection,
    screen_idx: usize,
    window: u32,
    gc: u32,
    visual: Visualtype,
    depth: u8,
    sw: i32,
    sh: i32,
    cur_h: i32,
}

fn run_loop() {
    let mut xs = match setup_window() {
        Ok(x) => x,
        Err(e) => {
            applog::error(&format!("悬浮层 X11 初始化失败（{}）；录音功能不受影响", e));
            // 空转等待销毁请求，保持句柄语义（post 不报错）
            while !CLOSED.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(TICK_MS));
            }
            return;
        }
    };
    applog::info(&format!(
        "悬浮层已就绪（X11 {}×{}，ARGB OR 窗口 + XShape 穿透）",
        xs.sw, xs.sh
    ));

    let tick = Duration::from_millis(TICK_MS);
    loop {
        // 1) 消费事件（一批全清）
        if let Some(o) = OVERLAY.get() {
            let guard = o.rx.lock().unwrap();
            if let Some(rx) = guard.as_ref() {
                while let Ok(ev) = rx.try_recv() {
                    drain_event(ev);
                }
            }
        }
        if CLOSED.load(Ordering::SeqCst) {
            break;
        }
        // 2) 定时隐藏 / 显隐同步（先算好动作，放锁后再操作 X，避免重入死锁）
        enum Action {
            None,
            Map,
            Unmap,
        }
        let mut action = Action::None;
        {
            let mut st = st_lock();
            if let Some((seq, dl)) = st.pending_hide {
                if Instant::now() >= dl {
                    if st.seq == seq {
                        st.visible = false;
                        st.recording = false;
                    }
                    st.pending_hide = None;
                }
            }
            if st.visible && !st.mapped {
                st.mapped = true;
                action = Action::Map;
            } else if !st.visible && st.mapped {
                st.mapped = false;
                action = Action::Unmap;
            }
        }
        match action {
            Action::Map => {
                let _ = xs.conn.map_window(xs.window);
                let _ = xs.conn.flush();
            }
            Action::Unmap => {
                let _ = xs.conn.unmap_window(xs.window);
                let _ = xs.conn.flush();
            }
            Action::None => {}
        }
        // 3) X 事件（Expose 触发重绘）
        if let Ok(Some(ev)) = xs.conn.poll_for_event() {
            if matches!(ev, x11rb::protocol::Event::Expose(_)) {
                let st = st_lock();
                if st.visible {
                    let opacity = current_opacity(&st);
                    redraw(&mut xs, &st, opacity);
                }
            }
        }
        // 4) 动画 + 重绘
        {
            let mut st = st_lock();
            if st.visible {
                let opacity = current_opacity(&st);
                if st.recording {
                    let elapsed = (Instant::now() - st.t0).as_secs_f64();
                    st.status = format!("正在录音 {:.1}s · 松开结束", elapsed);
                    st.status_color = STATUS_COLOR;
                }
                redraw(&mut xs, &st, opacity);
            }
        }
        std::thread::sleep(tick);
    }

    let _ = xs.conn.unmap_window(xs.window);
    let _ = xs.conn.destroy_window(xs.window);
    let _ = xs.conn.flush();
}

/// 呼吸 + 流光 → 当前整体 alpha（与 Windows overlay_tick 公式一致）
fn current_opacity(st: &OverlayState) -> f64 {
    let now = Instant::now();
    let epoch = st.breath_epoch.unwrap_or_else(|| now);
    let phase = (now - epoch).as_secs_f64() / BREATH_PERIOD_S * 2.0 * PI;
    let b01 = 0.5 * (1.0 + phase.sin());
    let mut opacity = OPACITY_BREATH_MIN + (OPACITY_BREATH_MAX - OPACITY_BREATH_MIN) * b01;
    if let Some(g) = st.glint {
        let dt = (now - g).as_secs_f64();
        if dt < GLINT_HOLD_S {
            opacity = GLINT_PEAK;
        } else if dt < GLINT_HOLD_S + GLINT_DECAY_S {
            let t = (dt - GLINT_HOLD_S) / GLINT_DECAY_S;
            opacity = GLINT_PEAK + (opacity - GLINT_PEAK) * t;
        }
    }
    opacity.clamp(0.0, 1.0)
}

fn map(xs: &XState) {
    let _ = xs.conn.map_window(xs.window);
    let _ = xs.conn.flush();
}

fn unmap(xs: &XState) {
    let _ = xs.conn.unmap_window(xs.window);
    let _ = xs.conn.flush();
}

// ---------------- X11 窗口建立 ----------------

fn setup_window() -> Result<XState, String> {
    let (conn, screen_idx) = x11rb::connect(None).map_err(|e| format!("X11 连接失败: {:?}", e))?;
    let screen = &conn.setup().roots[screen_idx];
    let sw = screen.width_in_pixels as i32;
    let sh = screen.height_in_pixels as i32;

    // 找 32 位 ARGB visual（TrueColor）
    let (visual, depth) = screen
        .allowed_depths
        .iter()
        .find(|d| d.depth == 32)
        .and_then(|d32| d32.visuals.first().map(|v| (*v, 32u8)))
        .ok_or("未找到 32 位 ARGB visual（合成器环境必需）")?;

    let win_id = conn.generate_id().map_err(|e| e.to_string())?;
    let colormap = conn.generate_id().map_err(|e| e.to_string())?;
    conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual.visual_id)
        .map_err(|e| e.to_string())?;

    let w = si(OVERLAY_WIDTH);
    let h = si(OVERLAY_HEIGHT);
    let x = ((sw - w) / 2).max(0) as i16;
    let y = ((sh - h) / 2).max(0) as i16;
    conn.create_window(
        depth,
        win_id,
        screen.root,
        x, y,
        w as u16, h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        visual.visual_id,
        &CreateWindowAux::default()
            .background_pixel(0)
            .border_pixel(0)
            .override_redirect(1)
            .colormap(colormap)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY),
    )
    .map_err(|e| e.to_string())?;

    let gc = conn.generate_id().map_err(|e| e.to_string())?;
    conn.create_gc(gc, win_id, &CreateGCAux::default().foreground(0))
        .map_err(|e| e.to_string())?;

    // 点击穿透：input 形状置空（等价 WS_EX_TRANSPARENT）
    conn.shape_rectangles(
        SO::SET,
        SK::INPUT,
        x11rb::protocol::xproto::ClipOrdering::UNSORTED,
        win_id,
        0,
        0,
        &[],
    )
    .map_err(|e| e.to_string())?;
    conn.flush().map_err(|e| e.to_string())?;

    Ok(XState { conn, screen_idx, window: win_id, gc, visual, depth: 32, sw, sh, cur_h: h })
}

// ---------------- 尺寸 / 缩放 ----------------

/// 悬浮层缩放：RUSTVUEVOICE_OVERLAY_SCALE > GDK_SCALE > 1.0
fn scale() -> f64 {
    static S: OnceLock<f64> = OnceLock::new();
    *S.get_or_init(|| {
        if let Ok(v) = std::env::var("RUSTVUEVOICE_OVERLAY_SCALE") {
            if let Ok(f) = v.parse::<f64>() {
                if f >= 0.5 {
                    return f;
                }
            }
        }
        if let Ok(v) = std::env::var("GDK_SCALE") {
            if let Ok(f) = v.parse::<f64>() {
                if f >= 1.0 {
                    return f;
                }
            }
        }
        1.0
    })
}
fn sf(v: f64) -> f64 {
    v * scale()
}
fn si(v: i32) -> i32 {
    (v as f64 * scale()).round() as i32
}

// ---------------- cairo 绘制 ----------------

fn argb_to_rgba(c: u32, opacity: f64) -> (f64, f64, f64, f64) {
    let a = ((c >> 24) & 0xFF) as f64 / 255.0 * opacity;
    let r = ((c >> 16) & 0xFF) as f64 / 255.0;
    let g = ((c >> 8) & 0xFF) as f64 / 255.0;
    let b = (c & 0xFF) as f64 / 255.0;
    (r, g, b, a)
}

fn rounded_rect(cr: &Context, x0: f64, y0: f64, x1: f64, y1: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x1 - r, y0 + r, r, -PI / 2.0, 0.0); // 右上
    cr.arc(x1 - r, y1 - r, r, 0.0, PI / 2.0); // 右下
    cr.arc(x0 + r, y1 - r, r, PI / 2.0, PI); // 左下
    cr.arc(x0 + r, y0 + r, r, PI, 1.5 * PI); // 左上
    cr.close_path();
}

fn set_font(cr: &Context, bold: bool, size: f64) {
    cr.select_font_face(
        FONT_FAMILY,
        FontSlant::Normal,
        if bold { FontWeight::Bold } else { FontWeight::Normal },
    );
    cr.set_font_size(size);
}

/// 文本布局：按字符宽度断行（CJK 逐字换行）。返回 (每行文本, 单行总宽, 行高)
fn layout_text(cr: &Context, text: &str, max_w: f64) -> (Vec<String>, f64, f64) {
    let fe = match cr.font_extents() {
        Ok(fe) => fe,
        Err(_) => return (vec![text.to_string()], 0.0, sf(30.0)),
    };
    let line_h = fe.ascent() + fe.descent();
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0.0;
    let mut first_w = 0.0;
    for ch in text.chars() {
        let s = ch.to_string();
        let w = cr.text_extents(&s).map(|e| e.x_advance()).unwrap_or(0.0);
        if !cur.is_empty() && cur_w + w > max_w {
            if first_w == 0.0 {
                first_w = cur_w;
            }
            lines.push(std::mem::take(&mut cur));
            cur_w = 0.0;
        }
        cur.push(ch);
        cur_w += w;
    }
    if !cur.is_empty() || lines.is_empty() {
        if first_w == 0.0 {
            first_w = cur_w;
        }
        lines.push(cur);
    }
    (lines, first_w, line_h)
}

fn draw_text_line(cr: &Context, text: &str, color: u32, opacity: f64, cx: f64, cy: f64, bold: bool, size: f64) {
    if text.is_empty() {
        return;
    }
    set_font(cr, bold, size);
    let (r, g, b, a) = argb_to_rgba(color, opacity);
    cr.set_source_rgba(r, g, b, a);
    let ext = match cr.text_extents(text) {
        Ok(e) => e,
        Err(_) => return,
    };
    let fe = cr.font_extents().ok();
    let (asc, desc) = fe.map(|f| (f.ascent(), f.descent())).unwrap_or((size * 0.8, size * 0.2));
    let x = cx - ext.width() / 2.0 - ext.x_bearing();
    let y = cy + (asc - desc) / 2.0;
    cr.move_to(x, y);
    let _ = cr.show_text(text);
}

/// 全量重绘悬浮层（呼吸系数 opacity 0~1）。
/// 多行时胶囊底部向下扩行，顶部位置保持不变（与 Windows 版一致）。
fn redraw(xs: &mut XState, st: &OverlayState, opacity: f64) {
    let w = si(OVERLAY_WIDTH);
    let base_h = si(OVERLAY_HEIGHT);
    let text_max_w = si(OVERLAY_WIDTH - 60) as f64;

    // 先在 scratch surface 上排版，决定胶囊是否向下扩高
    let measure = (|| -> Option<(Vec<String>, f64, f64)> {
        let surf = ImageSurface::create(cairo::Format::ARgb32, 1, 1).ok()?;
        let cr = Context::new(&surf).ok()?;
        set_font(&cr, true, sf(22.0));
        Some(layout_text(&cr, &st.text, text_max_w))
    })();
    let (lines, _, line_h) = measure.unwrap_or((vec![st.text.clone()], 0.0, sf(30.0)));
    let n = if st.text.is_empty() { 0 } else { lines.len() };
    let multiline = n > 1;
    let th = n as f64 * line_h;
    let h = if multiline {
        ((sf(76.0) + th + sf(16.0)).ceil() as i32).max(base_h)
    } else {
        base_h
    };

    // 窗口尺寸变化：顶部固定，向下生长
    if h != xs.cur_h {
        let x = ((xs.sw - w) / 2).max(0) as i16;
        let y = ((xs.sh - base_h) / 2).max(0) as i16;
        let _ = xs.conn.configure_window(
            xs.window,
            &x11rb::protocol::xproto::ConfigureWindowAux::default()
                .x(x as i32)
                .y(y as i32)
                .width(w as u32)
                .height(h as u32),
        );
        let _ = xs.conn.flush();
        xs.cur_h = h;
    }

    // 绘制到内存 ARGB surface
    let mut surface = match ImageSurface::create(cairo::Format::ARgb32, w, h) {
        Ok(s) => s,
        Err(e) => {
            log_once(&format!("cairo surface 创建失败: {:?}", e));
            return;
        }
    };
    // Context 必须在 surface.data() 之前 drop（cairo-rs 的 data() 要求独占借用）
    {
    let cr = match Context::new(&surface) {
        Ok(c) => c,
        Err(e) => {
            log_once(&format!("cairo context 创建失败: {:?}", e));
            return;
        }
    };
    cr.set_antialias(cairo::Antialias::Default); // 等价 SmoothingModeAntiAlias

    // 胶囊底 + 白边
    rounded_rect(&cr, 1.0, 1.0, (w - 1) as f64, (h - 1) as f64, sf(CORNER_PX));
    let (r, g, b, a) = argb_to_rgba(CAPSULE_FILL, opacity);
    cr.set_source_rgba(r, g, b, a);
    let _ = cr.fill();
    rounded_rect(&cr, 1.0, 1.0, (w - 1) as f64, (h - 1) as f64, sf(CORNER_PX));
    let (r, g, b, a) = argb_to_rgba(BORDER_COLOR, opacity);
    cr.set_source_rgba(r, g, b, a);
    cr.set_line_width(sf(1.5));
    let _ = cr.stroke();

    // 波形条
    let bar_w = sf(WAVE_BAR_W);
    let bar_gap = sf(WAVE_BAR_GAP);
    let wave_total = WAVE_BARS as f64 * (bar_w + bar_gap) - bar_gap;
    let x0 = (w as f64 - wave_total) / 2.0;
    let max_h = if st.text.is_empty() { sf(WAVE_BAR_MAX_H) } else { sf(18.0) };
    let (r, g, b, a) = argb_to_rgba(WAVE_COLOR, opacity);
    cr.set_source_rgba(r, g, b, a);
    for (i, v) in st.levels.iter().enumerate() {
        let bh = (v * max_h).max(sf(2.0));
        let x = x0 + i as f64 * (bar_w + bar_gap);
        let _ = cr.rectangle(x, sf(WAVE_CY) - bh / 2.0, bar_w, bh);
        let _ = cr.fill();
    }

    // 状态行：固定单行居中
    draw_text_line(&cr, &st.status, st.status_color, opacity, w as f64 / 2.0, sf(STATUS_Y), false, sf(15.0));

    // 正文：单行垂直居中；多行顶对齐（胶囊已扩高）
    if !st.text.is_empty() {
        if multiline {
            set_font(&cr, true, sf(22.0));
            let (r, g, b, a) = argb_to_rgba(TEXT_COLOR, opacity);
            cr.set_source_rgba(r, g, b, a);
            let top = sf(76.0);
            let fe = cr.font_extents().map(|f| (f.ascent(), f.descent())).unwrap_or((sf(18.0), sf(5.0)));
            for (i, line) in lines.iter().enumerate() {
                let ext = cr.text_extents(line).ok();
                let ew = ext.as_ref().map(|e| e.width()).unwrap_or(0.0);
                let xb = ext.as_ref().map(|e| e.x_bearing()).unwrap_or(0.0);
                let x = (w as f64 - text_max_w) / 2.0 + (text_max_w - ew) / 2.0 - xb;
                let y = top + fe.0 + i as f64 * line_h;
                cr.move_to(x, y);
                let _ = cr.show_text(line);
            }
        } else {
            draw_text_line(
                &cr,
                &st.text,
                TEXT_COLOR,
                opacity,
                w as f64 / 2.0,
                sf(TEXT_Y),
                true,
                sf(22.0),
            );
        }
    }
    }
    let _ = surface.flush();

    // PutImage 直传（x86 小端：cairo BGRA = X11 ZPixmap 32bpp）
    let data = match surface.data() {
        Ok(d) => d,
        Err(e) => {
            log_once(&format!("cairo 读取像素失败: {:?}", e));
            return;
        }
    };
    let bytes: &[u8] = &data;
    if let Err(e) = xs
        .conn
        .put_image(ImageFormat::Z_PIXMAP, xs.window, xs.gc, w as u16, h as u16, 0, 0, 0, xs.depth, bytes)
    {
        log_once(&format!("PutImage 失败: {:?}", e));
        return;
    }
    let _ = xs.conn.flush();
}

fn log_once(msg: &str) {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| applog::warn(msg));
}
