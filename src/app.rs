//! Window layout and the bridge between mpv and the web interface.
//!
//! Two windows: the video window (mpv draws into it via `wid`, it owns the taskbar entry)
//! and, owned by it so it always stays on top, a transparent window holding a WebView with
//! `ui/index.html`. The UI window is the one the user moves and resizes; the video window
//! follows it, sitting just below the dedicated title bar.
//!
//! macOS: same two windows, but mpv has no `wid` there, so it draws through the libmpv render API
//! into an OpenGL view of the video window (mac.rs), and the video window is a child of the UI
//! window (the one that moves) so that the two travel together.

use crate::{
    copy_frame, exe_dir, has_alpha, is_animated_image, load_markers, load_mpv, save_frame, win, Event as MpvEvent, Mpv,
    APP, EVENT_END_FILE, EVENT_FILE_LOADED, EVENT_HOOK, EVENT_PROPERTY_CHANGE, EVENT_SHUTDOWN, EVENT_START_FILE, OPTIONS,
};
use serde_json::{json, Map, Value};
use std::borrow::Cow;
use std::ffi::{c_char, CStr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
#[cfg(target_os = "macos")]
use tao::platform::macos::{WindowBuilderExtMacOS, WindowExtMacOS};
#[cfg(windows)]
use tao::platform::windows::{IconExtWindows, WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Icon, Window, WindowBuilder};
use wry::http::{header::CONTENT_TYPE, Request, Response, StatusCode};
use wry::{DragDropEvent, WebView, WebViewBuilder};

/// The native handle the `win` helpers work on: the HWND (Windows) or the NSWindow pointer (macOS).
fn hnd(w: &Window) -> isize {
    #[cfg(windows)]
    return w.hwnd();
    #[cfg(target_os = "macos")]
    return w.ns_window() as isize;
}

/// Page address: WebView2 serves custom protocols over http://<name>.localhost, WKWebView as <name>://localhost.
const START_URL: &str = if cfg!(windows) { "http://vb.localhost/index.html" } else { "vb://localhost/index.html" };

/// The `ui` folder: next to the exe, on macOS inside the bundle (Contents/Resources/ui) when there is one.
fn ui_folder(exe_dir: &Path) -> PathBuf {
    let bundled = exe_dir.join("../Resources/ui");
    if cfg!(target_os = "macos") && bundled.is_dir() { bundled } else { exe_dir.join("ui") }
}

/// Minimizes / restores the app. macOS: the UI window is the parent, its child (video) follows.
fn minimize(ui: &Window, video: &Window, on: bool) {
    if cfg!(target_os = "macos") { ui.set_minimized(on) } else { video.set_minimized(on) }
}

/// Height of the dedicated title bar, in CSS pixels (must match `--titlebar` in ui/index.html).
const TITLE_BAR: f64 = 40.0;
const BG_LOGO: &str = "#1b1a1e"; // idle screen and audio files: the logo tile colour
const BG_VIDEO: &str = "#000000";

/// mpv properties mirrored into the UI (`vb.update({...})`).
const OBSERVED: &[&str] = &[
    "pause", "time-pos", "duration", "volume", "mute", "filename", "path", "playlist-pos", "playlist-count",
    "estimated-frame-number", "estimated-frame-count", "container-fps", "idle-active", "speed", "loop-file",
    "keep-open", "ab-loop-a", "ab-loop-b", "chapter-list", "track-list", "sid", "aid", "background",
    "hwdec-current", "current-tracks/video", "current-tracks/audio", "video-params", "osd-dimensions", "media-title",
    "eof-reached", "playlist",
];

pub enum UserEvent {
    Prop(String, Option<String>),
    Loading(bool),
    VideoSize(f64, f64),
    Toast(String),
    Ipc(String),
    Drag(&'static str, Vec<PathBuf>, (i32, i32)),
    /// A newer release exists (tag); `prompt` opens the dialog, otherwise only the button lights up.
    Update(String, bool),
    /// JavaScript to run in the page, from a worker thread.
    Eval(String),
    /// Files opened while the app runs (a second launch, see single.rs).
    Open(Vec<PathBuf>),
    /// Close the app (e.g. to let the installer replace it: Windows only).
    #[cfg_attr(not(windows), allow(dead_code))]
    Close,
    Quit,
}

static PROXY: OnceLock<Mutex<EventLoopProxy<UserEvent>>> = OnceLock::new();
/// Set while an export runs: closing the window then would lose the file.
pub static EXPORTING: AtomicBool = AtomicBool::new(false);

fn send(ev: UserEvent) {
    if let Some(p) = PROXY.get()
        && let Ok(p) = p.lock()
    {
        let _ = p.send_event(ev);
    }
}

/// Shows the update prompt in the UI (now, or as soon as the page is ready).
/// Runs JavaScript in the page, from any thread.
pub fn eval_js(js: &str) {
    send(UserEvent::Eval(js.to_string()));
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn request_close() {
    send(UserEvent::Close);
}

pub fn offer_update(tag: &str, prompt: bool) {
    send(UserEvent::Update(tag.to_string(), prompt));
}

/// Toast in the web UI, from any thread.
pub fn toast(msg: &str) {
    send(UserEvent::Toast(msg.to_string()));
}

/// Files of the `ui` folder next to the exe, served at http://vb.localhost/.
/// Image chosen with "Importer un masque", served to the page at /mask.
static MASK: Mutex<Option<PathBuf>> = Mutex::new(None);

fn serve(dir: &Path, req: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let rel = req.uri().path().trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let not_found = || Response::builder().status(StatusCode::NOT_FOUND).body(Cow::Borrowed(&[][..])).unwrap();
    if rel == "mask" {
        let path = MASK.lock().ok().and_then(|m| m.clone());
        return match path.and_then(|p| std::fs::read(&p).ok().map(|b| (p, b))) {
            Some((p, bytes)) => {
                let mime = match p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref() {
                    Some("png") => "image/png",
                    Some("jpg" | "jpeg") => "image/jpeg",
                    Some("webp") => "image/webp",
                    Some("bmp") => "image/bmp",
                    Some("gif") => "image/gif",
                    Some("svg") => "image/svg+xml",
                    _ => "application/octet-stream",
                };
                Response::builder().header(CONTENT_TYPE, mime).body(Cow::Owned(bytes)).unwrap()
            }
            None => not_found(),
        };
    }
    // Only plain relative paths inside the ui folder.
    if rel.split('/').any(|part| part == ".." || part.contains(':') || part.contains('\\')) {
        return not_found();
    }
    let Ok(bytes) = std::fs::read(dir.join(rel)) else { return not_found() };
    let mime = match rel.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    };
    Response::builder().header(CONTENT_TYPE, mime).body(Cow::Owned(bytes)).unwrap()
}

/// Keeps the video window exactly under the UI window's content area (below the title bar).
/// The video window covers the whole window, under the title bar too, so Windows can round all four
/// of its corners like the title bar's (only in a plain window: maximized and fullscreen stay
/// square). mpv keeps the picture below the bar with a top margin.
fn sync_video(ui: &Window, video: &Window, m: Option<&Mpv>, mode: &Mode) {
    let Ok(pos) = ui.inner_position() else { return };
    let size = ui.inner_size();
    let fullscreen = matches!(mode, Mode::Fullscreen { .. });
    let bar = if fullscreen { 0.0 } else { (TITLE_BAR * ui.scale_factor()).round() };
    let rect = (pos.x, pos.y, size.width as i32, size.height as i32);
    // macOS: tao reports a size the app just set only later; AppKit's frame is current.
    #[cfg(target_os = "macos")]
    let (rect, size) = match win::window_rect(hnd(ui)) {
        Some(r) => (r, PhysicalSize::new(r.2.max(1) as u32, r.3.max(1) as u32)),
        None => (rect, size),
    };
    // Called on every move / resize step: only what changed is sent to Windows and mpv.
    if win::window_rect(hnd(video)) != Some(rect) {
        win::place_frameless(hnd(video), rect.0, rect.1, rect.2, rect.3);
    }
    #[cfg(target_os = "macos")]
    {
        if std::env::var_os("VISIBLEMENT_LOG").is_some() {
            eprintln!("sync_video: ui {rect:?} -> video {:?}", win::window_rect(hnd(video)));
        }
        win::set_render_size(size.width, size.height);
        win::presentation(fullscreen);
    }
    static LAST: Mutex<(Option<bool>, String)> = Mutex::new((None, String::new()));
    let Ok(mut last) = LAST.lock() else { return };
    let round = matches!(mode, Mode::Normal);
    if last.0 != Some(round) {
        // Both windows get the same Windows-drawn corners: the page draws none, so their shapes match exactly.
        win::round_corners(hnd(video), round);
        win::round_corners(hnd(ui), round);
        last.0 = Some(round);
    }
    if let Some(m) = m {
        let margin = format!("{:.4}", bar / f64::from(size.height.max(1)));
        if last.1 != margin {
            m.set("video-margin-ratio-top", &margin);
            last.1 = margin;
        }
    }
}

/// Smallest window (logical px, title bar included): below it the bar and Convertir get cramped.
const MIN_W: f64 = 640.0;
const MIN_H: f64 = 420.0;

/// New file: fit the window to the picture (90% of the screen at most), keeping its centre.
fn fit_to_video(ui: &Window, mode: &Mode, w: f64, h: f64) {
    if !matches!(mode, Mode::Normal) || w <= 0.0 || h <= 0.0 {
        return;
    }
    let Some(mon) = ui.current_monitor() else { return };
    let scale = ui.scale_factor();
    let (mw, mh) = (mon.size().width as f64 * 0.9, mon.size().height as f64 * 0.9);
    let bar = TITLE_BAR * scale;
    let k = (mw / w).min((mh - bar) / h).min(1.0);
    let (cw, ch) = ((w * k).round().max(MIN_W * scale), (h * k + bar).round().max(MIN_H * scale));
    let (Ok(pos), size) = (ui.outer_position(), ui.outer_size()) else { return };
    let (cx, cy) = (pos.x as f64 + size.width as f64 / 2.0, pos.y as f64 + size.height as f64 / 2.0);
    let (mx, my) = (mon.position().x as f64, mon.position().y as f64);
    let x = (cx - cw / 2.0).clamp(mx, mx + mon.size().width as f64 - cw);
    let y = (cy - ch / 2.0).clamp(my, my + mon.size().height as f64 - ch);
    #[cfg(not(target_os = "macos"))]
    {
        ui.set_inner_size(PhysicalSize::new(cw, ch));
        ui.set_outer_position(PhysicalPosition::new(x, y));
    }
    // Straight through AppKit: tao applies it later, after the video window has been placed.
    #[cfg(target_os = "macos")]
    win::set_rect(hnd(ui), (x as i32, y as i32, cw as i32, ch as i32));
}

/// mpv's event loop, on its own thread: forwards state to the UI thread.
fn mpv_events(m: Mpv) {
    loop {
        // SAFETY: mpv_wait_event never returns NULL; the event lives until the next call.
        let ev: &MpvEvent = unsafe { &*(m.wait_event)(m.h, -1.0) };
        match ev.id {
            EVENT_SHUTDOWN => {
                send(UserEvent::Quit);
                return;
            }
            EVENT_START_FILE => {
                send(UserEvent::Loading(true));
                let anim = m.get("path").is_some_and(|p| is_animated_image(&p));
                m.set("loop-file", if anim { "inf" } else { "no" });
            }
            EVENT_FILE_LOADED => {
                send(UserEvent::Loading(false));
                // Recent media: local files only (not streams, cameras or folders).
                if let Some(path) = m.get("path").filter(|p| Path::new(p).is_file()) {
                    crate::add_recent_media(&path);
                    eval_js(&format!("window.vb && vb.recent({})", json!(crate::recent_media())));
                }
                send(UserEvent::VideoSize(m.num("video-params/dw"), m.num("video-params/dh")));
                // Back on a file whose light copy just got ready: where it was. Otherwise: too heavy?
                let resume = crate::proxy::RESUME.lock().ok().and_then(|mut r| r.take());
                match resume {
                    Some((t, paused)) => {
                        m.cmd(&["seek", &t.to_string(), "absolute+exact"]);
                        m.set("pause", if paused { "yes" } else { "no" });
                    }
                    None => crate::proxy::check(m),
                }
            }
            EVENT_END_FILE => send(UserEvent::Loading(false)),
            EVENT_HOOK => {
                // SAFETY: data points to an mpv_event_hook { name, id }.
                let id = unsafe { *(ev.data as *const u8).add(std::mem::size_of::<*const c_char>()).cast::<u64>() };
                crate::proxy::redirect(&m); // a heavy file with a light copy plays that copy
                load_markers(&m);
                unsafe { (m.hook_continue)(m.h, id) };
            }
            EVENT_PROPERTY_CHANGE => {
                // SAFETY: data points to an mpv_event_property whose first field is the name.
                let name = unsafe { CStr::from_ptr(*(ev.data as *const *const c_char)) }.to_string_lossy().into_owned();
                if name == "video-params" {
                    send(UserEvent::Prop("alpha".into(), Some(has_alpha(&m).to_string())));
                }
                if matches!(name.as_str(), "idle-active" | "current-tracks/video" | "current-tracks/audio") {
                    let idle = m.get("idle-active").as_deref() == Some("yes");
                    let audio = !idle && m.get("current-tracks/video/id").is_none() && m.get("current-tracks/audio/id").is_some();
                    m.set("background-color", if idle || audio { BG_LOGO } else { BG_VIDEO });
                }
                // mpv does not notify estimated-frame-number on its own: refresh it with the position.
                if name == "time-pos" {
                    send(UserEvent::Prop("estimated-frame-number".into(), m.get("estimated-frame-number")));
                }
                let value = m.get(&name);
                send(UserEvent::Prop(name, value));
            }
            _ => {}
        }
    }
}

fn eval(webview: &WebView, js: &str) {
    let _ = webview.evaluate_script(js);
}

/// Window state, handled here rather than with tao's maximize / fullscreen: those toggle the
/// hidden caption and frame styles of the transparent UI window, and after a fullscreen round
/// trip Windows ends up drawing a classic frame where the window used to be.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    /// Fills the monitor's work area; `prev` is the rectangle to restore.
    Maximized { prev: Rect },
    /// Covers the whole monitor; `prev` / `was_max` restore what was there before.
    Fullscreen { prev: Rect, was_max: Option<Rect> },
}

type Rect = (i32, i32, i32, i32);

/// The window's real rectangle, asked to Windows: tao's cached values can lag behind our own
/// SetWindowPos calls (maximize / fullscreen / drag-out are done here, not through tao).
fn rect_of(ui: &Window) -> Rect {
    win::window_rect(hnd(ui)).unwrap_or_else(|| {
        let p = ui.outer_position().unwrap_or_default();
        let s = ui.outer_size();
        (p.x, p.y, s.width as i32, s.height as i32)
    })
}

/// A window move (empty `dir`) or resize from an edge ("n", "se"…), started at `cursor` (screen px).
struct Grab {
    dir: String,
    cursor: (f64, f64),
    pos: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
}

impl Grab {
    /// New outer position and size for the cursor at (x, y), never smaller than `min`.
    fn apply(&self, x: f64, y: f64, min: (f64, f64)) -> (f64, f64, f64, f64) {
        let (dx, dy) = (x - self.cursor.0, y - self.cursor.1);
        let (mut px, mut py) = (self.pos.x as f64, self.pos.y as f64);
        let (mut w, mut h) = (self.size.width as f64, self.size.height as f64);
        if self.dir.is_empty() {
            return (px + dx, py + dy, w, h);
        }
        if self.dir.contains('e') {
            w = (w + dx).max(min.0);
        }
        if self.dir.contains('s') {
            h = (h + dy).max(min.1);
        }
        if self.dir.contains('w') {
            let nw = (w - dx).max(min.0);
            px += w - nw;
            w = nw;
        }
        if self.dir.contains('n') {
            let nh = (h - dy).max(min.1);
            py += h - nh;
            h = nh;
        }
        (px.round(), py.round(), w.round(), h.round())
    }
}

/// Closing: the windows disappear at once and mpv is asked to quit. mpv can be busy for a while
/// (opening a huge file, a slow disk), so the process leaves anyway after a short grace period.
/// Refused while an export runs, which would lose the file.
fn close(ui: &Window, video: &Window, m: &Mpv) {
    if EXPORTING.load(Ordering::Relaxed) {
        toast("Export en cours, patiente avant de fermer");
        return;
    }
    ui.set_visible(false);
    video.set_visible(false);
    crate::save_volume(m.get("volume"), m.get("mute"));
    m.cmd(&["quit"]);
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(1500));
        std::process::exit(0);
    });
}

/// Loads dropped or chosen files: the first replaces the playlist (its folder becomes the playlist).
/// `append`: added at the end of the playlist (playing at once if nothing plays).
fn load_paths(m: &Mpv, paths: &[PathBuf], append: bool) {
    for (i, p) in paths.iter().enumerate() {
        m.cmd(&["loadfile", &p.to_string_lossy(), if i == 0 && !append { "replace" } else { "append-play" }]);
    }
}

pub fn run() -> Result<(), String> {
    // Every argument is a file (or URL) to play: also covers "Open with" and drag & drop onto the .exe.
    // Already running? That app opens them and this launch ends here.
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let pipe = crate::single::forward(&args);
    let dir = exe_dir();
    let ui_dir = ui_folder(&dir);
    let (_lib, api) = load_mpv(&dir)?;

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    // First size: 1280 x 720 picture, but never more than 90% of a small screen (it would spill off it).
    let (start_w, start_h) = event_loop.primary_monitor().map_or((1280.0, 720.0), |mon| {
        let s = mon.size().to_logical::<f64>(mon.scale_factor());
        let k = (s.width * 0.9 / 1280.0).min((s.height * 0.9 - TITLE_BAR) / 720.0).min(1.0);
        ((1280.0 * k).round().max(MIN_W), (720.0 * k).round().max(MIN_H - TITLE_BAR))
    });
    let _ = PROXY.set(Mutex::new(event_loop.create_proxy()));
    if let Some(pipe) = pipe {
        crate::single::serve(pipe, |paths| send(UserEvent::Open(paths)));
    }
    // macOS takes the Dock icon from the app bundle.
    #[cfg(windows)]
    let icon = Icon::from_resource(1, None).ok();
    #[cfg(not(windows))]
    let icon: Option<Icon> = None;

    // Video window: taskbar entry and owner of the UI window.
    let video = WindowBuilder::new()
        .with_title(APP)
        .with_decorations(false)
        .with_window_icon(icon.clone())
        .with_inner_size(LogicalSize::new(start_w, start_h));
    #[cfg(windows)]
    let video = video.with_undecorated_shadow(false);
    let video = video.build(&event_loop).map_err(|e| e.to_string())?;
    // UI window: transparent, on top of the video window, follows nothing (it is the one moved).
    let ui = WindowBuilder::new()
        .with_title(APP)
        .with_decorations(false)
        .with_transparent(true)
        .with_window_icon(icon)
        .with_visible(false) // shown once its hidden frame is gone, so it never paints one
        .with_inner_size(LogicalSize::new(start_w, start_h + TITLE_BAR))
        .with_min_inner_size(LogicalSize::new(MIN_W, MIN_H));
    #[cfg(windows)]
    let ui = ui.with_undecorated_shadow(false).with_owner_window(video.hwnd() as _).with_skip_taskbar(true);
    // macOS: no shadow around the transparent UI window (the video window carries the shadow).
    #[cfg(target_os = "macos")]
    let ui = ui.with_has_shadow(false);
    let ui = ui.build(&event_loop).map_err(|e| e.to_string())?;
    if let Some(mon) = ui.current_monitor() {
        let s = ui.outer_size();
        let (mx, my) = (mon.position().x, mon.position().y);
        ui.set_outer_position(PhysicalPosition::new(
            mx + (mon.size().width as i32 - s.width as i32) / 2,
            my + (mon.size().height as i32 - s.height as i32) / 2,
        ));
    }
    // Our own move / resize / maximize: the UI window needs no hidden caption or frame at all.
    win::strip_frame(hnd(&ui));
    win::never_paint_frame(hnd(&ui));
    ui.set_visible(true);
    // macOS: the video window is a child of the UI window, so it follows every move of it, and sits
    // below it. It never takes clicks: the UI window (transparent over the picture) gets them.
    #[cfg(target_os = "macos")]
    {
        win::attach_child(hnd(&ui), hnd(&video));
        let _ = video.set_ignore_cursor_events(true);
    }
    sync_video(&ui, &video, None, &Mode::Normal);
    let mut mode = Mode::Normal;

    // mpv, drawing into the video window.
    let h = unsafe { (api.create)() };
    if h.is_null() {
        return Err(crate::tr("Impossible de créer le lecteur mpv.", "Could not create the mpv player.").into());
    }
    let m = Mpv { h, ..api };
    m.option("config", "no");
    #[cfg(windows)]
    m.option("wid", &video.hwnd().to_string());
    for (k, v) in OPTIONS {
        m.option(k, v);
    }
    // macOS: no `wid`; the frames are pulled through the render API (src/mac.rs). No forced window:
    // the libmpv video output only exists once the render context does.
    #[cfg(target_os = "macos")]
    {
        m.option("vo", "libmpv");
        m.option("force-window", "no");
    }
    // Volume and mute as they were when the app last closed.
    for (k, v) in crate::saved_volume() {
        m.option(k, &v);
    }
    // Web video pages (YouTube…): mpv's ytdl hook uses the yt-dlp the app downloads on demand.
    if let Some(ytdlp) = crate::stream::ytdlp_path() {
        m.option("script-opts", &format!("ytdl_hook-ytdl_path={}", ytdlp.to_string_lossy()));
    }
    // Troubleshooting: VISIBLEMENT_LOG=C:\path\log.txt writes mpv's log there.
    if let Some(log) = std::env::var_os("VISIBLEMENT_LOG") {
        m.option("log-file", &log.to_string_lossy());
    }
    if unsafe { (m.initialize)(h) } < 0 {
        unsafe { (m.destroy)(h) };
        return Err("Impossible d'initialiser mpv.".into());
    }
    #[cfg(target_os = "macos")]
    let mut renderer = match win::start_render(&m, video.ns_view()) {
        Ok(r) => r,
        Err(e) => {
            unsafe { (m.destroy)(h) };
            return Err(e);
        }
    };
    sync_video(&ui, &video, Some(&m), &Mode::Normal); // picture below the title bar from the start
    // Frosted glass over the video under the page's menus (CSS blur can't reach mpv's window).
    for (name, source) in [("blur", include_str!("blur.glsl")), ("blur2", include_str!("blur2.glsl"))] {
        let path = std::env::temp_dir().join(format!("visiblement-{name}.glsl"));
        // macOS draws through the render API's older renderer, which knows no DYNAMIC parameters.
        #[cfg(target_os = "macos")]
        let source = source.replace("//!TYPE DYNAMIC float", "//!TYPE float");
        if std::fs::write(&path, source).is_ok() {
            m.cmd(&["change-list", "glsl-shaders", "append", &path.to_string_lossy()]);
        }
    }
    // Before each file opens: turn WAV markers (cue points, After Effects XMP) into chapters.
    unsafe { (m.hook_add)(h, 0, c"on_load".as_ptr(), 0) };
    // NDI sources play as ndi://<name>: the app feeds mpv the frames it receives (src/ndi.rs).
    unsafe { (m.stream_cb_add_ro)(h, c"ndi".as_ptr(), std::ptr::null_mut(), crate::ndi::stream_open) };
    for p in OBSERVED {
        m.observe(p);
    }
    let events = thread::spawn(move || mpv_events(m));

    let proxy_ipc = PROXY.get().map(|p| p.lock().unwrap().clone()).expect("proxy");
    let proxy_drop = proxy_ipc.clone();
    let webview = WebViewBuilder::new()
        .with_transparent(true)
        .with_background_color((0, 0, 0, 0))
        .with_devtools(cfg!(debug_assertions))
        .with_custom_protocol("vb".into(), move |_id, req| serve(&ui_dir, &req))
        .with_url(START_URL)
        .with_ipc_handler(move |req: Request<String>| {
            let _ = proxy_ipc.send_event(UserEvent::Ipc(req.body().clone()));
        })
        .with_drag_drop_handler(move |ev| {
            let _ = proxy_drop.send_event(match ev {
                DragDropEvent::Enter { paths, position } => UserEvent::Drag("enter", paths, position),
                DragDropEvent::Over { position } => UserEvent::Drag("over", Vec::new(), position),
                DragDropEvent::Drop { paths, position } => UserEvent::Drag("drop", paths, position),
                DragDropEvent::Leave => UserEvent::Drag("leave", Vec::new(), (0, 0)),
                _ => return true,
            });
            true // handled here: the page never navigates to the file
        })
        .build(&ui)
        .map_err(|e| format!("Interface impossible à charger (WebView2) : {e}"))?;

    crate::check_for_update(false);
    load_paths(&m, &args, false);

    let mut pending = Map::new();
    let mut flush_at: Option<Instant> = None;
    // Controls hidden (page says so): the playback clock stops reaching the page until they show.
    let mut ui_quiet = false;
    let mut events = Some(events);
    let mut workers: Vec<thread::JoinHandle<()>> = Vec::new();
    let mut grab: Option<Grab> = None;
    let mut last_cursor = (0.0f64, f64::MAX);
    let (mut ui_ready, mut update_offer): (bool, Option<(String, bool)>) = (false, None);
    // A file opened from Explorer starts loading before the page is up: "ready" replays the loader.
    let mut loading = false;
    let show_update = |webview: &WebView, tag: &str, prompt: bool| {
        let info = json!({ "latest": tag, "current": env!("CARGO_PKG_VERSION"), "prompt": prompt, "installable": crate::update::installable() });
        eval(webview, &format!("window.vb && vb.updateAvailable({info})"));
    };

    event_loop.run(move |event, _, flow| {
        *flow = flush_at.map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if !pending.is_empty() {
                    eval(&webview, &format!("window.vb && vb.update({})", Value::Object(std::mem::take(&mut pending))));
                }
                flush_at = None;
                *flow = ControlFlow::Wait;
            }
            Event::UserEvent(ev) => match ev {
                UserEvent::Prop(k, v) => {
                    let clock = matches!(k.as_str(), "time-pos" | "estimated-frame-number");
                    pending.insert(k, v.map_or(Value::Null, Value::String));
                    // ~30 updates per second at most: time-pos changes every frame. With the controls
                    // hidden nothing shows it, so it just waits in `pending` (the latest value wins).
                    if flush_at.is_none() && !(ui_quiet && clock) {
                        flush_at = Some(Instant::now() + Duration::from_millis(33));
                        *flow = ControlFlow::WaitUntil(flush_at.unwrap());
                    }
                }
                UserEvent::Loading(on) => {
                    loading = on;
                    eval(&webview, &format!("window.vb && vb.loading({on})"));
                }
                UserEvent::VideoSize(w, h) => {
                    fit_to_video(&ui, &mode, w, h);
                    // macOS sends no Resized for a size the app sets itself: the video window would stay behind.
                    sync_video(&ui, &video, Some(&m), &mode);
                }
                UserEvent::Toast(msg) => eval(&webview, &format!("window.vb && vb.toast({})", json!(msg))),
                UserEvent::Eval(js) => eval(&webview, &js),
                // Opened from Explorer while running: replaces what plays (the playlist is built
                // only by dropping files on it), and the window comes to the front.
                UserEvent::Open(paths) => {
                    minimize(&ui, &video, false);
                    ui.set_focus();
                    if !paths.is_empty() {
                        load_paths(&m, &paths, false);
                    }
                }
                UserEvent::Close => close(&ui, &video, &m),
                UserEvent::Update(tag, prompt) => {
                    if ui_ready {
                        show_update(&webview, &tag, prompt);
                    }
                    update_offer = Some((tag, prompt));
                }
                // Positions are physical pixels from the page's top-left corner; the page decides where
                // a drop goes (into the playlist when it is open, otherwise in place of what plays).
                UserEvent::Drag(kind, paths, (x, y)) => {
                    let over = kind == "enter" || kind == "over";
                    eval(&webview, &format!("window.vb && vb.drag({over}, {x}, {y})"));
                    if kind == "drop" && !paths.is_empty() {
                        eval(&webview, &format!("window.vb && vb.dropped({}, {x}, {y})", json!(paths)));
                    }
                }
                UserEvent::Ipc(body) => {
                    let Ok(msg) = serde_json::from_str::<Value>(&body) else { return };
                    let s = |k: &str| msg.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    match s("t").as_str() {
                        "ready" => {
                            ui_ready = true;
                            if let Some((tag, prompt)) = &update_offer {
                                show_update(&webview, tag, *prompt);
                            }
                            eval(&webview, &format!("window.vb && vb.version({})", json!(env!("CARGO_PKG_VERSION"))));
                            eval(&webview, &format!("window.vb && vb.lang({})", json!(crate::read_settings().get("lang"))));
                            eval(&webview, &format!("window.vb && vb.recent({})", json!(crate::recent_media())));
                            let mut all = Map::new();
                            for p in OBSERVED {
                                all.insert(p.to_string(), m.get(p).map_or(Value::Null, Value::String));
                            }
                            all.insert("alpha".into(), Value::String(has_alpha(&m).to_string()));
                            eval(&webview, &format!("window.vb && vb.update({})", Value::Object(all)));
                            if loading {
                                eval(&webview, "window.vb && vb.loading(true)");
                            }
                        }
                        "cmd" => {
                            let args: Vec<String> = msg
                                .get("a")
                                .and_then(Value::as_array)
                                .map(|a| a.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string)).collect())
                                .unwrap_or_default();
                            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                            if !refs.is_empty() {
                                m.cmd(&refs);
                            }
                        }
                        "set" => m.set(&s("k"), &s("v")),
                        // 360° view: the shader renders at window size, so mpv must not letterbox it.
                        "probe360" => {
                            let path = s("path");
                            thread::spawn(move || {
                                let on = crate::is_spherical(&path);
                                eval_js(&format!("window.vb && vb.spherical({}, {on})", json!(path)));
                            });
                        }
                        "view360" => {
                            let on = msg["on"].as_bool() == Some(true);
                            let shader = std::env::temp_dir().join("visiblement-360.glsl");
                            if on {
                                let _ = std::fs::write(&shader, include_str!("360.glsl"));
                            }
                            m.cmd(&["change-list", "glsl-shaders", if on { "append" } else { "remove" }, shader.to_str().unwrap_or("")]);
                            m.set("keepaspect", if on { "no" } else { "yes" });
                        }
                        "win" => match s("a").as_str() {
                            // Moving / resizing is driven by the page (pointer capture + screen coordinates):
                            // Windows' own drag loop can't start while WebView2 holds the mouse capture.
                            // Dragging the title bar of a maximized / fullscreen window: like Windows, it goes back
                            // to its previous size under the cursor (same relative spot on the bar), then moves.
                            "grab" if mode != Mode::Normal && s("dir").is_empty() => {
                                let Some((x, y)) = win::cursor() else { return };
                                let prev = match mode {
                                    Mode::Maximized { prev } => prev,
                                    Mode::Fullscreen { prev, was_max } => was_max.unwrap_or(prev),
                                    Mode::Normal => return,
                                };
                                let now = rect_of(&ui);
                                let frac = ((x - now.0 as f64) / now.2.max(1) as f64).clamp(0.05, 0.95);
                                let bar = TITLE_BAR * ui.scale_factor();
                                let nx = (x - frac * prev.2 as f64).round() as i32;
                                let ny = (y - bar / 2.0).round() as i32;
                                if matches!(mode, Mode::Fullscreen { .. }) {
                                    eval(&webview, "window.vb && vb.fullscreen(false)");
                                }
                                eval(&webview, "window.vb && vb.maximized(false)");
                                mode = Mode::Normal;
                                win::set_rect(hnd(&ui), (nx, ny, prev.2, prev.3));
                                sync_video(&ui, &video, Some(&m), &mode);
                                grab = Some(Grab {
                                    dir: String::new(),
                                    cursor: (x, y),
                                    pos: PhysicalPosition::new(nx, ny),
                                    size: PhysicalSize::new(prev.2 as u32, prev.3 as u32),
                                });
                            }
                            "grab" if mode != Mode::Normal => {} // no edge resizing while maximized / fullscreen
                            // The cursor comes from Windows (physical pixels, right on every monitor), not from
                            // the page, whose screen coordinates go wrong across monitors of different scales.
                            "grab" => {
                                if let (Ok(pos), Some(cursor)) = (ui.outer_position(), win::cursor()) {
                                    grab = Some(Grab { dir: s("dir"), cursor, pos, size: ui.outer_size() });
                                }
                            }
                            "grab-move" => {
                                let Some((x, y)) = win::cursor() else { return };
                                last_cursor = (x, y);
                                // macOS: the page may never hear the button go up; a move without it ends the grab.
                                #[cfg(target_os = "macos")]
                                if !win::mouse_down() {
                                    grab = None;
                                }
                                // Only a plain window moves or resizes: a late move must not undo fullscreen.
                                if mode != Mode::Normal {
                                    grab = None;
                                }
                                if let Some(g) = &grab {
                                    let min = (MIN_W * ui.scale_factor(), MIN_H * ui.scale_factor());
                                    let (px, py, w, h) = g.apply(x, y, min);
                                    let r = |v: f64| v.round() as i32;
                                    win::move_pair(hnd(&ui), hnd(&video), (r(px), r(py), r(w), r(h)));
                                }
                            }
                            // Released at the top edge of the screen: maximize, like Windows' snap.
                            "grab-end" => {
                                if let Some(g) = grab.take()
                                    && g.dir.is_empty()
                                    && let Some((monitor, work)) = win::monitor_rects(hnd(&ui))
                                    && last_cursor.1 <= (monitor.1 + 3) as f64
                                {
                                    let prev = (g.pos.x, g.pos.y, g.size.width as i32, g.size.height as i32);
                                    win::set_rect(hnd(&ui), work);
                                    mode = Mode::Maximized { prev };
                                    eval(&webview, "window.vb && vb.maximized(true)");
                                    sync_video(&ui, &video, Some(&m), &mode);
                                }
                            }
                            "min" => minimize(&ui, &video, true),
                            "max" => {
                                grab = None;
                                mode = match mode {
                                    Mode::Normal => match win::monitor_rects(hnd(&ui)) {
                                        Some((_, work)) => {
                                            let prev = rect_of(&ui);
                                            win::set_rect(hnd(&ui), work);
                                            Mode::Maximized { prev }
                                        }
                                        None => Mode::Normal,
                                    },
                                    Mode::Maximized { prev } => {
                                        win::set_rect(hnd(&ui), prev);
                                        Mode::Normal
                                    }
                                    fs => fs,
                                };
                                eval(&webview, &format!("window.vb && vb.maximized({})", matches!(mode, Mode::Maximized { .. })));
                                sync_video(&ui, &video, Some(&m), &mode);
                            }
                            "fs" => {
                                grab = None;
                                // macOS: menu bar and Dock first. Hiding them after the window took the whole
                                // screen sent it back to its previous frame.
                                #[cfg(target_os = "macos")]
                                win::presentation(!matches!(mode, Mode::Fullscreen { .. }));
                                mode = match mode {
                                    Mode::Fullscreen { prev, was_max } => {
                                        win::set_rect(hnd(&ui), was_max.map_or(prev, |_| prev));
                                        match was_max {
                                            Some(before_max) => Mode::Maximized { prev: before_max },
                                            None => Mode::Normal,
                                        }
                                    }
                                    other => match win::monitor_rects(hnd(&ui)) {
                                        Some((monitor, _)) => {
                                            let prev = rect_of(&ui);
                                            let was_max = match other { Mode::Maximized { prev } => Some(prev), _ => None };
                                            win::set_rect(hnd(&ui), monitor);
                                            Mode::Fullscreen { prev, was_max }
                                        }
                                        None => other,
                                    },
                                };
                                let fullscreen = matches!(mode, Mode::Fullscreen { .. });
                                if std::env::var_os("VISIBLEMENT_LOG").is_some() {
                                    eprintln!("fs {fullscreen}: monitor {:?} -> ui {:?}", win::monitor_rects(hnd(&ui)), win::window_rect(hnd(&ui)));
                                }
                                eval(&webview, &format!("window.vb && vb.fullscreen({fullscreen})"));
                                sync_video(&ui, &video, Some(&m), &mode);
                            }
                            "close" => close(&ui, &video, &m),
                            _ => {}
                        },
                        "check-update" => crate::check_for_update(true),
                        "install-update" => {
                            if msg["never"].as_bool() == Some(true) {
                                crate::set_never_ask_update(true);
                            }
                            crate::update::install();
                        }
                        "update" => {
                            update_offer = None;
                            if msg["never"].as_bool() == Some(true) {
                                crate::set_never_ask_update(true);
                            }
                            if msg["download"].as_bool() == Some(true) {
                                crate::open_release_page();
                            }
                        }
                        "save-frame" => workers.push(save_frame(m)),
                        "copy-frame" => workers.push(copy_frame(m)),
                        "export" => workers.push(crate::editor::export(m)),
                        "cancel-encode" => crate::editor::CANCEL.store(true, std::sync::atomic::Ordering::Relaxed),
                        "quiet" => {
                            ui_quiet = msg["on"].as_bool() == Some(true);
                            if !ui_quiet && !pending.is_empty() {
                                flush_at = Some(Instant::now());
                                *flow = ControlFlow::WaitUntil(flush_at.unwrap());
                            }
                        }
                        "convert" => {
                            let seq = msg["seq"].as_str().zip(msg["fps"].as_str()).map(|(u, f)| (u.to_string(), f.to_string()));
                            workers.push(crate::editor::convert(m, s("preset"), msg["alpha"].as_bool() == Some(true), seq));
                        }
                        // Convertir › Séquence d'images › Choisir…: one image of it; the page shows the sequence found.
                        "pick-seq" => {
                            if let Some(image) = rfd::FileDialog::new()
                                .set_title(crate::tr("Choisir une image de la séquence", "Pick an image of the sequence"))
                                .add_filter("Images", &["png", "jpg", "jpeg", "tif", "tiff", "exr", "dpx", "tga", "bmp", "webp"])
                                .pick_file()
                            {
                                match crate::sequence_url(&image).zip(crate::sequence_label(&image)) {
                                    Some((url, label)) => eval(&webview, &format!("window.vb && vb.seqPicked({}, {})", json!(url), json!(label))),
                                    None => toast("Cette image n'est pas numérotée (ex. plan_0001.png)"),
                                }
                            }
                        }
                        "lang" => crate::set_lang(&s("v")),
                        "open" => {
                            if let Some(file) = rfd::FileDialog::new().set_title(crate::tr("Ouvrir un fichier", "Open a file")).pick_file() {
                                load_paths(&m, &[file], false);
                            }
                        }
                        "open-files" => {
                            if let Some(files) = rfd::FileDialog::new().set_title(crate::tr("Ouvrir plusieurs fichiers", "Open files")).pick_files() {
                                load_paths(&m, &files, msg["append"].as_bool() == Some(true));
                            }
                        }
                        // mpv plays a folder as a playlist of the media it contains.
                        "open-folder" => {
                            if let Some(dir) = rfd::FileDialog::new().set_title(crate::tr("Ouvrir un dossier", "Open a folder")).pick_folder() {
                                load_paths(&m, &[dir], false);
                            }
                        }
                        "open-path" => {
                            let path = s("path");
                            if Path::new(&path).exists() {
                                load_paths(&m, &[PathBuf::from(path)], false);
                            } else {
                                toast("Ce fichier n'existe plus");
                            }
                        }
                        "open-url" => crate::stream::open(m, &s("url")),
                        // Page Up / Page Down: the previous / next media of the playing file's folder.
                        "folder-step" => {
                            let step = msg["step"].as_i64().unwrap_or(1) as i32;
                            match m.get("path").and_then(|p| crate::folder_neighbour(Path::new(&p), step)) {
                                Some(p) => load_paths(&m, &[p], false),
                                None => toast(if step > 0 { "Dernière vidéo du dossier" } else { "Première vidéo du dossier" }),
                            }
                        }
                        "load-paths" => {
                            let paths: Vec<PathBuf> = msg["paths"].as_array().into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from).collect();
                            match msg["at"].as_u64() {
                                // Into the playlist before entry `at` (playing at once if nothing plays).
                                Some(at) => {
                                    for (i, p) in paths.iter().enumerate() {
                                        m.cmd(&["loadfile", &p.to_string_lossy(), "insert-at-play", &(at + i as u64).to_string()]);
                                    }
                                }
                                None => load_paths(&m, &paths, msg["append"].as_bool() == Some(true)),
                            }
                        }
                        "list-cameras" => {
                            thread::spawn(|| eval_js(&format!("window.vb && vb.cameras({})", json!(crate::list_cameras()))));
                        }
                        "list-ndi" => {
                            thread::spawn(|| {
                                let list = crate::ndi::sources();
                                eval_js(&format!("window.vb && vb.ndi({})", json!(list)));
                            });
                        }
                        "open-ndi" => crate::ndi::open(m, s("name")),
                        "open-camera" => {
                            m.cmd(&["loadfile", &format!("av://dshow:video={}", s("name")), "replace"]);
                        }
                        "import-mask" => {
                            if let Some(file) = rfd::FileDialog::new()
                                .set_title(crate::tr("Importer un masque", "Import a mask"))
                                .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp", "gif", "svg"])
                                .pick_file()
                            {
                                let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                if let Ok(mut slot) = MASK.lock() {
                                    *slot = Some(file);
                                }
                                // The timestamp makes the page reload the image even if the path is the same.
                                let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
                                eval(&webview, &format!("window.vb && vb.mask({}, {})", json!(format!("mask?{stamp}")), json!(name)));
                            }
                        }
                        "info" => eval(&webview, &format!("window.vb && vb.info({})", crate::media_info(&m))),
                        "open-subs" => {
                            if let Some(f) = rfd::FileDialog::new()
                                .set_title(crate::tr("Charger des sous-titres", "Load subtitles"))
                                .add_filter("Sous-titres", &["srt", "ass", "ssa", "vtt", "sub"])
                                .pick_file()
                            {
                                m.cmd(&["sub-add", &f.to_string_lossy(), "select"]);
                            }
                        }
                        "show-in-folder" => {
                            if let Some(p) = m.get("path") {
                                win::show_in_folder(&p);
                            }
                        }
                        _ => {}
                    }
                }
                UserEvent::Quit => {
                    // mpv has shut down: its event thread is done; free it, then leave.
                    if let Some(t) = events.take() {
                        let _ = t.join();
                    }
                    for w in workers.drain(..) {
                        if w.is_finished() {
                            let _ = w.join();
                        }
                    }
                    // The render context must go before the mpv handle (libmpv rule).
                    #[cfg(target_os = "macos")]
                    renderer.stop();
                    unsafe { (m.destroy)(m.h) };
                    std::process::exit(0);
                }
            },
            Event::WindowEvent { window_id, event, .. } => match event {
                WindowEvent::Moved(_) | WindowEvent::Resized(_) if window_id == ui.id() => {
                    sync_video(&ui, &video, Some(&m), &mode);
                }
                // Clicking the taskbar entry activates the video window: hand focus to the UI.
                WindowEvent::Focused(true) if window_id == video.id() => ui.set_focus(),
                WindowEvent::CloseRequested => close(&ui, &video, &m),
                _ => {}
            },
            // macOS delivers files (double click, "Open with", drop on the Dock icon) as Apple Events.
            #[cfg(target_os = "macos")]
            Event::Opened { urls } => {
                let paths: Vec<PathBuf> = urls.iter().filter_map(|u| u.to_file_path().ok()).collect();
                send(UserEvent::Open(paths));
            }
            _ => {}
        }
        workers.retain(|w| !w.is_finished());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grab(dir: &str) -> Grab {
        Grab { dir: dir.into(), cursor: (100.0, 100.0), pos: PhysicalPosition::new(10, 20), size: PhysicalSize::new(800, 600) }
    }

    #[test]
    fn move_follows_the_cursor() {
        assert_eq!(grab("").apply(130.0, 90.0, (480.0, 340.0)), (40.0, 10.0, 800.0, 600.0));
    }

    #[test]
    fn resize_from_the_top_left_keeps_the_bottom_right_corner() {
        assert_eq!(grab("nw").apply(150.0, 130.0, (480.0, 340.0)), (60.0, 50.0, 750.0, 570.0));
        // clamped at the minimum size: the opposite edge does not move
        assert_eq!(grab("nw").apply(900.0, 900.0, (480.0, 340.0)), (330.0, 280.0, 480.0, 340.0));
    }

    #[test]
    fn resize_from_the_bottom_right() {
        assert_eq!(grab("se").apply(120.0, 80.0, (480.0, 340.0)), (10.0, 20.0, 820.0, 580.0));
    }
}
