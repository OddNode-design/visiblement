//! Window layout and the bridge between mpv and the web interface.
//!
//! Two windows: the video window (mpv draws into it via `wid`, it owns the taskbar entry)
//! and, owned by it so it always stays on top, a transparent window holding a WebView with
//! `ui/index.html`. The UI window is the one the user moves and resizes; the video window
//! follows it, sitting just below the dedicated title bar.

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
use tao::platform::windows::{IconExtWindows, WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Fullscreen, Icon, Window, WindowBuilder};
use wry::http::{header::CONTENT_TYPE, Request, Response, StatusCode};
use wry::{DragDropEvent, WebView, WebViewBuilder};

/// Height of the dedicated title bar, in CSS pixels (must match `--titlebar` in ui/index.html).
const TITLE_BAR: f64 = 40.0;
const BG_LOGO: &str = "#1b1a1e"; // idle screen and audio files: the logo tile colour
const BG_VIDEO: &str = "#000000";

/// mpv properties mirrored into the UI (`vb.update({...})`).
const OBSERVED: &[&str] = &[
    "pause", "time-pos", "duration", "volume", "mute", "filename", "path", "playlist-pos", "playlist-count",
    "estimated-frame-number", "estimated-frame-count", "container-fps", "idle-active", "speed", "loop-file",
    "keep-open", "ab-loop-a", "ab-loop-b", "chapter-list", "track-list", "sid", "aid", "background",
    "hwdec-current", "current-tracks/video", "current-tracks/audio", "video-params",
];

pub enum UserEvent {
    Prop(String, Option<String>),
    Loading(bool),
    VideoSize(f64, f64),
    Toast(String),
    Ipc(String),
    Drag(&'static str, Vec<PathBuf>),
    /// A newer release exists (tag); `prompt` opens the dialog, otherwise only the button lights up.
    Update(String, bool),
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
pub fn offer_update(tag: &str, prompt: bool) {
    send(UserEvent::Update(tag.to_string(), prompt));
}

/// Toast in the web UI, from any thread.
pub fn toast(msg: &str) {
    send(UserEvent::Toast(msg.to_string()));
}

/// Files of the `ui` folder next to the exe, served at http://vb.localhost/.
fn serve(dir: &Path, req: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let rel = req.uri().path().trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let not_found = || Response::builder().status(StatusCode::NOT_FOUND).body(Cow::Borrowed(&[][..])).unwrap();
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
fn sync_video(ui: &Window, video: &Window, fullscreen: bool) {
    let Ok(pos) = ui.inner_position() else { return };
    let size = ui.inner_size();
    let bar = if fullscreen { 0 } else { (TITLE_BAR * ui.scale_factor()).round() as u32 };
    video.set_outer_position(PhysicalPosition::new(pos.x, pos.y + bar as i32));
    video.set_inner_size(PhysicalSize::new(size.width.max(1), size.height.saturating_sub(bar).max(1)));
}

/// New file: fit the window to the picture (90% of the screen at most), keeping its centre.
fn fit_to_video(ui: &Window, w: f64, h: f64) {
    if ui.is_maximized() || ui.fullscreen().is_some() || w <= 0.0 || h <= 0.0 {
        return;
    }
    let Some(mon) = ui.current_monitor() else { return };
    let scale = ui.scale_factor();
    let (mw, mh) = (mon.size().width as f64 * 0.9, mon.size().height as f64 * 0.9);
    let bar = TITLE_BAR * scale;
    let k = (mw / w).min((mh - bar) / h).min(1.0);
    let (cw, ch) = ((w * k).round().max(480.0), (h * k + bar).round().max(300.0));
    let (Ok(pos), size) = (ui.outer_position(), ui.outer_size()) else { return };
    let (cx, cy) = (pos.x as f64 + size.width as f64 / 2.0, pos.y as f64 + size.height as f64 / 2.0);
    let (mx, my) = (mon.position().x as f64, mon.position().y as f64);
    let x = (cx - cw / 2.0).clamp(mx, mx + mon.size().width as f64 - cw);
    let y = (cy - ch / 2.0).clamp(my, my + mon.size().height as f64 - ch);
    ui.set_inner_size(PhysicalSize::new(cw, ch));
    ui.set_outer_position(PhysicalPosition::new(x, y));
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
                send(UserEvent::VideoSize(m.num("video-params/dw"), m.num("video-params/dh")));
            }
            EVENT_END_FILE => send(UserEvent::Loading(false)),
            EVENT_HOOK => {
                // SAFETY: data points to an mpv_event_hook { name, id }.
                let id = unsafe { *(ev.data as *const u8).add(std::mem::size_of::<*const c_char>()).cast::<u64>() };
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

/// Loads dropped or chosen files: the first replaces the playlist (its folder becomes the playlist).
fn load_paths(m: &Mpv, paths: &[PathBuf]) {
    for (i, p) in paths.iter().enumerate() {
        m.cmd(&["loadfile", &p.to_string_lossy(), if i == 0 { "replace" } else { "append" }]);
    }
}

pub fn run() -> Result<(), String> {
    let dir = exe_dir();
    let ui_dir = dir.join("ui");
    let (_lib, api) = load_mpv(&dir)?;

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let _ = PROXY.set(Mutex::new(event_loop.create_proxy()));
    let icon = Icon::from_resource(1, None).ok();

    // Video window: taskbar entry and owner of the UI window.
    let video = WindowBuilder::new()
        .with_title(APP)
        .with_decorations(false)
        .with_undecorated_shadow(false)
        .with_window_icon(icon.clone())
        .with_inner_size(LogicalSize::new(1280.0, 720.0))
        .build(&event_loop)
        .map_err(|e| e.to_string())?;
    // UI window: transparent, on top of the video window, follows nothing (it is the one moved).
    let ui = WindowBuilder::new()
        .with_title(APP)
        .with_decorations(false)
        .with_transparent(true)
        .with_undecorated_shadow(false)
        .with_window_icon(icon)
        .with_owner_window(video.hwnd() as _)
        .with_skip_taskbar(true)
        .with_inner_size(LogicalSize::new(1280.0, 720.0 + TITLE_BAR))
        .with_min_inner_size(LogicalSize::new(480.0, 300.0))
        .build(&event_loop)
        .map_err(|e| e.to_string())?;
    if let Some(mon) = ui.current_monitor() {
        let s = ui.outer_size();
        let (mx, my) = (mon.position().x, mon.position().y);
        ui.set_outer_position(PhysicalPosition::new(
            mx + (mon.size().width as i32 - s.width as i32) / 2,
            my + (mon.size().height as i32 - s.height as i32) / 2,
        ));
    }
    sync_video(&ui, &video, false);

    // mpv, drawing into the video window.
    let h = unsafe { (api.create)() };
    if h.is_null() {
        return Err("Impossible de créer le lecteur mpv.".into());
    }
    let m = Mpv { h, ..api };
    m.option("config", "no");
    m.option("wid", &video.hwnd().to_string());
    for (k, v) in OPTIONS {
        m.option(k, v);
    }
    // Troubleshooting: VISIBLEMENT_LOG=C:\path\log.txt writes mpv's log there.
    if let Some(log) = std::env::var_os("VISIBLEMENT_LOG") {
        m.option("log-file", &log.to_string_lossy());
    }
    if unsafe { (m.initialize)(h) } < 0 {
        unsafe { (m.destroy)(h) };
        return Err("Impossible d'initialiser mpv.".into());
    }
    // Before each file opens: turn WAV markers (cue points, After Effects XMP) into chapters.
    unsafe { (m.hook_add)(h, 0, c"on_load".as_ptr(), 0) };
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
        .with_url("http://vb.localhost/index.html")
        .with_ipc_handler(move |req: Request<String>| {
            let _ = proxy_ipc.send_event(UserEvent::Ipc(req.body().clone()));
        })
        .with_drag_drop_handler(move |ev| {
            let _ = proxy_drop.send_event(match ev {
                DragDropEvent::Enter { paths, .. } => UserEvent::Drag("enter", paths),
                DragDropEvent::Drop { paths, .. } => UserEvent::Drag("drop", paths),
                DragDropEvent::Leave => UserEvent::Drag("leave", Vec::new()),
                _ => return true,
            });
            true // handled here: the page never navigates to the file
        })
        .build(&ui)
        .map_err(|e| format!("Interface impossible à charger (WebView2) : {e}"))?;

    crate::check_for_update(false);
    // Every argument is a file (or URL) to play: also covers "Open with" and drag & drop onto the .exe.
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    load_paths(&m, &args);

    let mut pending = Map::new();
    let mut flush_at: Option<Instant> = None;
    let mut events = Some(events);
    let mut workers: Vec<thread::JoinHandle<()>> = Vec::new();
    let mut grab: Option<Grab> = None;
    let (mut ui_ready, mut update_offer): (bool, Option<(String, bool)>) = (false, None);
    let show_update = |webview: &WebView, tag: &str, prompt: bool| {
        let info = json!({ "latest": tag, "current": env!("CARGO_PKG_VERSION"), "prompt": prompt });
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
                    pending.insert(k, v.map_or(Value::Null, Value::String));
                    // ~30 updates per second at most: time-pos changes every frame.
                    if flush_at.is_none() {
                        flush_at = Some(Instant::now() + Duration::from_millis(33));
                        *flow = ControlFlow::WaitUntil(flush_at.unwrap());
                    }
                }
                UserEvent::Loading(on) => eval(&webview, &format!("window.vb && vb.loading({on})")),
                UserEvent::VideoSize(w, h) => fit_to_video(&ui, w, h),
                UserEvent::Toast(msg) => eval(&webview, &format!("window.vb && vb.toast({})", json!(msg))),
                UserEvent::Update(tag, prompt) => {
                    if ui_ready {
                        show_update(&webview, &tag, prompt);
                    }
                    update_offer = Some((tag, prompt));
                }
                UserEvent::Drag(kind, paths) => {
                    eval(&webview, &format!("window.vb && vb.drag({})", json!(kind == "enter")));
                    if kind == "drop" && !paths.is_empty() {
                        load_paths(&m, &paths);
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
                            let mut all = Map::new();
                            for p in OBSERVED {
                                all.insert(p.to_string(), m.get(p).map_or(Value::Null, Value::String));
                            }
                            all.insert("alpha".into(), Value::String(has_alpha(&m).to_string()));
                            eval(&webview, &format!("window.vb && vb.update({})", Value::Object(all)));
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
                        "win" => match s("a").as_str() {
                            // Moving / resizing is driven by the page (pointer capture + screen coordinates):
                            // Windows' own drag loop can't start while WebView2 holds the mouse capture.
                            "grab" => {
                                if let (Ok(pos), Some(x), Some(y)) = (ui.outer_position(), msg["x"].as_f64(), msg["y"].as_f64()) {
                                    grab = Some(Grab { dir: s("dir"), cursor: (x, y), pos, size: ui.outer_size() });
                                }
                            }
                            "grab-move" => {
                                if let (Some(g), Some(x), Some(y)) = (&grab, msg["x"].as_f64(), msg["y"].as_f64()) {
                                    let min = (480.0 * ui.scale_factor(), (300.0 + TITLE_BAR) * ui.scale_factor());
                                    let (px, py, w, h) = g.apply(x, y, min);
                                    if g.dir.is_empty() {
                                        ui.set_outer_position(PhysicalPosition::new(px, py));
                                    } else {
                                        ui.set_outer_position(PhysicalPosition::new(px, py));
                                        ui.set_inner_size(PhysicalSize::new(w, h));
                                    }
                                }
                            }
                            "grab-end" => grab = None,
                            "min" => video.set_minimized(true),
                            "max" => ui.set_maximized(!ui.is_maximized()),
                            "fs" => {
                                let fullscreen = ui.fullscreen().is_none();
                                ui.set_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
                                eval(&webview, &format!("window.vb && vb.fullscreen({fullscreen})"));
                                sync_video(&ui, &video, fullscreen);
                            }
                            "close" => {
                                if EXPORTING.load(Ordering::Relaxed) {
                                    crate::app::toast("Export en cours, patiente avant de fermer");
                                } else {
                                    m.cmd(&["quit"]);
                                }
                            }
                            _ => {}
                        },
                        "check-update" => crate::check_for_update(true),
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
                        "open" => {
                            if let Some(files) = rfd::FileDialog::new().set_title("Ouvrir").pick_files() {
                                load_paths(&m, &files);
                            }
                        }
                        "open-subs" => {
                            if let Some(f) = rfd::FileDialog::new()
                                .set_title("Charger des sous-titres")
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
                    unsafe { (m.destroy)(m.h) };
                    std::process::exit(0);
                }
            },
            Event::WindowEvent { window_id, event, .. } => match event {
                WindowEvent::Moved(_) | WindowEvent::Resized(_) if window_id == ui.id() => {
                    sync_video(&ui, &video, ui.fullscreen().is_some());
                }
                // Clicking the taskbar entry activates the video window: hand focus to the UI.
                WindowEvent::Focused(true) if window_id == video.id() => ui.set_focus(),
                WindowEvent::CloseRequested => {
                    if EXPORTING.load(Ordering::Relaxed) {
                        toast("Export en cours, patiente avant de fermer");
                    } else {
                        m.cmd(&["quit"]);
                    }
                }
                _ => {}
            },
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
