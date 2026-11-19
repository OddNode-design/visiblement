// No console window in release builds; errors go to a message box.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use std::{ptr, thread};

mod editor;
mod markers;

const APP: &str = "Visiblement";

type Handle = *mut c_void;

const EVENT_NONE: c_int = 0;
const EVENT_SHUTDOWN: c_int = 1;
const EVENT_START_FILE: c_int = 6;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const EVENT_CLIENT_MESSAGE: c_int = 16;
const EVENT_PROPERTY_CHANGE: c_int = 22;
const EVENT_HOOK: c_int = 25;

const LOGO_PX: i64 = 320;
// One eye, raw premultiplied BGRA, scaled and squashed (blink) by mpv.
const EYE_FILE: &str = "visiblement-eye.bgra";
// Idle screen and audio files: the cloud alone, eyes drawn live (from assets/cloud.svg), 512-unit layout.
const CLOUD_FILE: &str = "visiblement-cloud.bgra";
const BG_DEFAULT: &str = "#111014";
const BG_LOGO: &str = "#1b1a1e"; // the logo tile colour, as the whole background
const EYE_PX: (i64, i64) = (64, 104);
// Eyes in the 512-unit logo space (assets/icon.svg): centers, radii.
const EYES: [(f64, f64); 2] = [(226.0, 256.0), (286.0, 256.0)];
const EYE_RX: f64 = 21.0;
const EYE_RY: f64 = 34.0;

// Tuned for speed: GPU decode (NVDEC/D3D11VA/...) with safe fallback, modern GPU renderer.
const OPTIONS: &[(&str, &str)] = &[
    ("vo", "gpu-next"),
    ("hwdec", "auto-safe"),
    ("idle", "yes"),         // stay open with an empty window: drop files on it
    ("force-window", "yes"),
    ("keep-open", "yes"),    // keep the last frame instead of closing
    ("image-display-duration", "inf"), // still images (png, static webp) stay on screen
    ("autocreate-playlist", "filter"), // previous / next = the other media in the same folder
    ("directory-filter-types", "video,image"),
    ("input-default-bindings", "yes"),
    ("input-vo-keyboard", "yes"),
    ("osc", "no"),           // uosc (portable_config/scripts) replaces the stock controls
    ("osd-bar", "no"),
    ("border", "no"),        // uosc draws its own title bar
    ("background", "color"),  // transparent videos: plain background; the alpha button switches to a checkerboard
    ("background-color", "#1b1a1e"), // starts on the idle screen (BG_LOGO)
    ("autofit-larger", "90%x90%"), // 8K or vertical videos open at a size that fits the screen
    ("screenshot-high-bit-depth", "no"), // 8-bit frames: what the clipboard and most apps expect
    ("title", "${?filename:${filename} - }Visiblement"),
];

#[repr(C)]
struct Event {
    id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct ClientMessage {
    num_args: c_int,
    args: *const *const c_char,
}

#[derive(Clone, Copy)]
struct Mpv {
    h: Handle,
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    wait_event: unsafe extern "C" fn(Handle, f64) -> *const Event,
    destroy: unsafe extern "C" fn(Handle),
    hook_add: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    hook_continue: unsafe extern "C" fn(Handle, u64) -> c_int,
    set_option: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    set_property: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    set_property_raw: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    get_property: unsafe extern "C" fn(Handle, *const c_char) -> *mut c_char,
    observe: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    command: unsafe extern "C" fn(Handle, *const *const c_char) -> c_int,
    free: unsafe extern "C" fn(*mut c_void),
}

// SAFETY: the libmpv client API is thread-safe, and the handle is only destroyed
// after every worker thread has been joined.
unsafe impl Send for Mpv {}

fn cstr(s: &str) -> CString {
    // Interior NULs cannot come from mpv or the OS; drop them rather than fail.
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl Mpv {
    fn option(&self, k: &str, v: &str) {
        unsafe { (self.set_option)(self.h, cstr(k).as_ptr(), cstr(v).as_ptr()) };
    }
    fn set(&self, k: &str, v: &str) {
        unsafe { (self.set_property)(self.h, cstr(k).as_ptr(), cstr(v).as_ptr()) };
    }
    /// Boolean property (MPV_FORMAT_FLAG), e.g. for uosc button conditions.
    fn set_flag(&self, k: &str, v: bool) {
        let mut flag = c_int::from(v);
        unsafe { (self.set_property_raw)(self.h, cstr(k).as_ptr(), 3, (&raw mut flag).cast()) };
    }
    fn get(&self, k: &str) -> Option<String> {
        unsafe {
            let p = (self.get_property)(self.h, cstr(k).as_ptr());
            if p.is_null() {
                return None;
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            (self.free)(p.cast());
            Some(s)
        }
    }
    fn num(&self, k: &str) -> f64 {
        self.get(k).and_then(|v| v.parse().ok()).unwrap_or(0.0)
    }
    fn observe(&self, k: &str) {
        unsafe { (self.observe)(self.h, 0, cstr(k).as_ptr(), 0) }; // 0 = MPV_FORMAT_NONE
    }
    fn cmd(&self, args: &[&str]) -> bool {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
        ptrs.push(ptr::null());
        unsafe { (self.command)(self.h, ptrs.as_ptr()) >= 0 }
    }
    fn toast(&self, msg: &str) {
        self.cmd(&["show-text", msg, "1500"]);
    }
}

/// WAV markers -> temporary FFmetadata file -> chapters for this file only (shown on the timeline).
fn load_markers(m: &Mpv) {
    let Some(path) = m.get("path") else { return };
    if !markers::is_wav(&path) {
        return;
    }
    let found = markers::read_wav_markers(Path::new(&path));
    if found.is_empty() {
        return;
    }
    let file = std::env::temp_dir().join(format!("visiblement-markers-{}.ffmeta", std::process::id()));
    if std::fs::write(&file, markers::to_ffmetadata(&found)).is_ok() {
        m.set("file-local-options/chapters-file", &file.to_string_lossy());
    }
}

/// True when the current video carries transparency (ProRes 4444, PNG, QuickTime Animation…).
fn has_alpha(m: &Mpv) -> bool {
    let alpha = m.get("video-params/alpha").unwrap_or_default();
    alpha == "straight" || alpha == "premul" || m.get("video-params/pixelformat").is_some_and(|f| is_alpha_format(&f))
}

/// FFmpeg pixel formats with an alpha plane: yuva420p, yuva444p10le, rgba, bgra, argb, gbrap, ya8, pal8 (GIF)…
fn is_alpha_format(f: &str) -> bool {
    let f = f.to_ascii_lowercase();
    f.starts_with("yuva") || f.starts_with("gbra") || f.starts_with("ya") || f == "pal8"
        || ["rgba", "bgra", "argb", "abgr"].iter().any(|p| f.starts_with(p))
}

// Animated images loop forever, like in a browser.
fn is_animated_image(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [".gif", ".webp", ".apng"].iter().any(|e| p.ends_with(e))
}

/// Idle screen: the logo, centered, with eyes that follow the mouse and blink.
/// Everything is drawn with overlay-add bitmaps, which mpv renders above the OSD.
struct Idle {
    cloud: String,
    eye: String,
    shown: bool,
    /// Background currently switched to the logo colour.
    logo_bg: bool,
    x: f64,
    y: f64,
    scale: f64,
    blink_until: Instant,
    next_blink: Instant,
    last_eyes: Vec<i64>,
    // Gaze: a spring (pos, vel) chases the target, in logo units from the eyes' rest position.
    gaze: (f64, f64),
    pos: (f64, f64),
    vel: (f64, f64),
    mouse: (f64, f64),
    last_move: Instant,
    next_glance: Instant,
    last_step: Instant,
}

impl Idle {
    fn new(dir: &Path) -> Self {
        let now = Instant::now();
        let file = |n: &str| dir.join(n).to_string_lossy().into_owned();
        Idle {
            cloud: file(CLOUD_FILE),
            eye: file(EYE_FILE),
            shown: false,
            logo_bg: false,
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            blink_until: now,
            next_blink: now + Duration::from_secs(2),
            last_eyes: Vec::new(),
            gaze: (256.0, 256.0),
            pos: (0.0, 0.0),
            vel: (0.0, 0.0),
            mouse: (-1.0, -1.0),
            last_move: now,
            next_glance: now,
            last_step: now,
        }
    }

    /// Show, move or hide the logo after a window resize, an idle change or a new file.
    /// Idle: the logo tile + hint. Audio-only file: the cloud alone on the tile colour.
    fn layout(&mut self, m: &Mpv) {
        let (w, h) = (m.num("osd-width"), m.num("osd-height"));
        let idle = m.get("idle-active").as_deref() == Some("yes");
        let audio = !idle && m.get("current-tracks/video/id").is_none() && m.get("current-tracks/audio/id").is_some();
        if (idle || audio) != self.logo_bg {
            self.logo_bg = idle || audio;
            m.set("background-color", if self.logo_bg { BG_LOGO } else { BG_DEFAULT });
        }
        if !(idle || audio) || w <= 0.0 || h <= 0.0 {
            if self.shown {
                for id in ["0", "1", "2"] {
                    m.cmd(&["overlay-remove", id]);
                }
                m.cmd(&["osd-overlay", "3", "none", ""]);
                self.shown = false;
                self.last_eyes.clear();
            }
            return;
        }
        let d = (w.min(h) * 0.5).min(LOGO_PX as f64).round();
        self.scale = d / 512.0;
        (self.x, self.y) = (((w - d) / 2.0).round(), ((h - d) / 2.0).round());
        let (px, ds) = (LOGO_PX.to_string(), (d as i64).to_string());
        m.cmd(&[
            "overlay-add", "0", &(self.x as i64).to_string(), &(self.y as i64).to_string(), &self.cloud,
            "0", "bgra", &px, &px, &(LOGO_PX * 4).to_string(), &ds, &ds,
        ]);
        if audio {
            m.cmd(&["osd-overlay", "3", "none", ""]);
            self.shown = true;
            self.last_eyes.clear();
            self.draw_eyes(m);
            return;
        }
        let hint = format!(
            r"{{\an8\pos({},{})\fs{}\bord0\shad0\1c&HFAF2F4&\alpha&H66&}}Glisse une vidéo ici · clic droit pour le menu",
            (w / 2.0).round(),
            (self.y + d + d * 0.08).round(),
            (d * 0.075).max(14.0).round()
        );
        m.cmd(&["osd-overlay", "3", "ass-events", &hint, &w.to_string(), &h.to_string()]);
        self.shown = true;
        self.last_eyes.clear();
        self.draw_eyes(m);
    }

    /// Move the eye bitmaps if the mouse moved or a blink started / ended.
    fn draw_eyes(&mut self, m: &Mpv) {
        if !self.shown {
            return;
        }
        let now = Instant::now();
        if now >= self.next_blink {
            self.blink_until = now + Duration::from_millis(130);
            self.next_blink = now + Duration::from_millis(2500 + jitter_ms(3500));
        }
        let ry = if now < self.blink_until { 4.0 } else { EYE_RY };
        // Follow the mouse while it moves; once it rests, glance around on its own.
        if m.get("mouse-pos/hover").as_deref() == Some("yes") {
            let mouse = (m.num("mouse-pos/x"), m.num("mouse-pos/y"));
            if mouse != self.mouse {
                self.mouse = mouse;
                self.gaze = ((mouse.0 - self.x) / self.scale, (mouse.1 - self.y) / self.scale);
                self.last_move = now;
            }
        }
        if now - self.last_move > Duration::from_millis(2500) && now >= self.next_glance {
            let angle = jitter_ms(6283) as f64 / 1000.0;
            let dist = 120.0 + jitter_ms(400) as f64;
            self.gaze = if jitter_ms(4) == 0 {
                (256.0, 256.0)
            } else {
                (256.0 + angle.cos() * dist, 256.0 + angle.sin() * dist * 0.7)
            };
            self.next_glance = now + Duration::from_millis(900 + jitter_ms(2200));
        }
        // Spring step at ~60 Hz: slight overshoot reads as alive.
        if now - self.last_step >= Duration::from_millis(14) {
            self.last_step = now;
            let target = gaze_offset(self.gaze.0 - 256.0, self.gaze.1 - 256.0);
            self.vel.0 = (self.vel.0 + (target.0 - self.pos.0) * 0.14) * 0.74;
            self.vel.1 = (self.vel.1 + (target.1 - self.pos.1) * 0.14) * 0.74;
            self.pos.0 += self.vel.0;
            self.pos.1 += self.vel.1;
        }
        let (s, (ox, oy)) = (self.scale, self.pos);
        let rects: Vec<i64> = EYES
            .iter()
            .flat_map(|&(cx, cy)| {
                let (dw, dh) = ((2.0 * EYE_RX * s).max(1.0), (2.0 * ry * s).max(1.0));
                let (ex, ey) = (self.x + (cx + ox) * s - dw / 2.0, self.y + (cy + oy) * s - dh / 2.0);
                [ex, ey, dw, dh].map(|v| v.round() as i64)
            })
            .collect();
        if rects == self.last_eyes {
            return;
        }
        let (ew, eh) = (EYE_PX.0.to_string(), EYE_PX.1.to_string());
        for (i, r) in rects.as_chunks::<4>().0.iter().enumerate() {
            let [x, y, dw, dh] = r.map(|v| v.to_string());
            m.cmd(&[
                "overlay-add", &(i + 1).to_string(), &x, &y, &self.eye,
                "0", "bgra", &ew, &eh, &(EYE_PX.0 * 4).to_string(), &dw, &dh,
            ]);
        }
        self.last_eyes = rects;
    }
}

/// transitions.dev-style "Matrix dot loader": a 5×5 dot grid pulsing through four patterns,
/// drawn while a file opens (shown after a short delay so fast opens never flash it).
struct Loader {
    since: Option<Instant>,
    shown: bool,
}

impl Loader {
    const DELAY: Duration = Duration::from_millis(180);

    fn start(&mut self) {
        self.since = Some(Instant::now());
    }

    fn stop(&mut self, m: &Mpv) {
        self.since = None;
        if self.shown {
            m.cmd(&["osd-overlay", "4", "none", ""]);
            self.shown = false;
        }
    }

    fn active(&self) -> bool {
        self.since.is_some()
    }

    fn tick(&mut self, m: &Mpv) {
        let Some(since) = self.since else { return };
        let t = since.elapsed();
        if t < Self::DELAY {
            return;
        }
        let (w, h) = (m.num("osd-width"), m.num("osd-height"));
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let dot = (w.min(h) / 160.0).clamp(3.0, 8.0).round();
        let step = dot * 2.0;
        let (x0, y0) = ((w - step * 4.0 - dot) / 2.0, (h - step * 4.0 - dot) / 2.0);
        let mut ass = String::new();
        for j in 0..5 {
            for i in 0..5 {
                let a = (255.0 * (1.0 - dot_level(t.as_secs_f64(), i, j))).round() as u8;
                let (x, y) = (x0 + i as f64 * step, y0 + j as f64 * step);
                ass.push_str(&format!(
                    r"{{\an7\pos(0,0)\bord0\shad0\1c&HFAF2F4&\1a&H{a:02X}&\p1}}m {x:.0} {y:.0} l {:.0} {y:.0} {:.0} {:.0} {x:.0} {:.0}",
                    x + dot, x + dot, y + dot, y + dot
                ));
                ass.push('\n');
            }
        }
        m.cmd(&["osd-overlay", "4", "ass-events", &ass, &w.to_string(), &h.to_string()]);
        self.shown = true;
    }
}

/// Brightness (0.12–1) of dot (i, j) at time t: a wave travelling outward, its shape changing
/// every 0.9 s between rings, a cross, a diagonal sweep and a centre pulse.
fn dot_level(t: f64, i: i32, j: i32) -> f64 {
    let (dx, dy) = ((i - 2).abs() as f64, (j - 2).abs() as f64);
    let d = match (t / 0.9) as u64 % 4 {
        0 => dx.max(dy),         // rings
        1 => dx + dy,            // cross / diamond
        2 => (i + j) as f64 / 2.0, // diagonal sweep
        _ => (dx * dx + dy * dy).sqrt(), // round pulse
    };
    let wave = ((t * 7.0 - d * 1.6).sin() + 1.0) / 2.0;
    0.12 + 0.88 * wave * wave
}

/// How far the eyes move toward a point (dx, dy) away from them: up to 44 logo units.
fn gaze_offset(dx: f64, dy: f64) -> (f64, f64) {
    let d = dx.hypot(dy).max(1.0);
    let k = (d / 3.0).min(44.0) / d;
    (dx * k, dy * k)
}

// shortcut: clock-based jitter, not a real RNG; fine for blink timing.
fn jitter_ms(max: u64) -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos() as u64 % max)
}

/// Pause on the current frame, ask where to save it, write it at full resolution.
fn save_frame(m: Mpv) -> thread::JoinHandle<()> {
    let was_paused = m.get("pause").as_deref() == Some("yes");
    m.set("pause", "yes");
    let name = format!(
        "{}_frame{}.png",
        m.get("filename/no-ext").unwrap_or_else(|| APP.into()),
        m.get("estimated-frame-number").unwrap_or_default()
    );
    thread::spawn(move || {
        let target = rfd::FileDialog::new()
            .set_title("Enregistrer la frame")
            .set_file_name(&name)
            .add_filter("PNG", &["png"])
            .add_filter("JPEG", &["jpg", "jpeg"])
            .add_filter("WebP", &["webp"])
            .save_file();
        if let Some(p) = target {
            // mpv picks the image format from the extension.
            let ok = m.cmd(&["screenshot-to-file", &p.to_string_lossy(), "video"]);
            m.toast(if ok { "Frame enregistrée" } else { "Échec de l'enregistrement" });
        }
        if !was_paused {
            m.set("pause", "no");
        }
    })
}

/// Grab the current frame and put it on the clipboard as an image.
fn copy_frame(m: Mpv) -> thread::JoinHandle<()> {
    let tmp = std::env::temp_dir().join(format!("visiblement-frame-{}.png", std::process::id()));
    let grabbed = m.cmd(&["screenshot-to-file", &tmp.to_string_lossy(), "video"]);
    thread::spawn(move || {
        let ok = grabbed && png_to_clipboard(&tmp).is_ok();
        let _ = std::fs::remove_file(&tmp);
        m.toast(if ok { "Frame copiée" } else { "Échec de la copie" });
    })
}

fn png_to_clipboard(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut dec = png::Decoder::new(file);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image trop grande")?];
    let info = reader.next_frame(&mut buf)?;
    let rgba = to_rgba(&buf[..info.buffer_size()], info.color_type).ok_or("format non géré")?;
    arboard::Clipboard::new()?.set_image(arboard::ImageData {
        width: info.width as usize,
        height: info.height as usize,
        bytes: rgba.into(),
    })?;
    Ok(())
}

fn to_rgba(px: &[u8], color: png::ColorType) -> Option<Vec<u8>> {
    match color {
        png::ColorType::Rgba => Some(px.to_vec()),
        png::ColorType::Rgb => Some(px.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect()),
        _ => None,
    }
}

const REPO: &str = "OddNode-design/visiblement";

/// "v1.2.3" -> [1, 2, 3]; anything unparsable counts as 0.
fn parse_version(v: &str) -> Vec<u32> {
    v.trim().trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Extracts "tag_name" from the GitHub releases API answer without a JSON dependency.
fn tag_name(json: &str) -> Option<&str> {
    let rest = &json[json.find("\"tag_name\"")? + 10..];
    let rest = &rest[rest.find('"')? + 1..];
    Some(&rest[..rest.find('"')?])
}

/// Background check at startup: if GitHub has a newer release, offer to open its page.
fn check_for_update() {
    use std::os::windows::process::CommandExt;
    thread::spawn(|| {
        // curl.exe ships with Windows 10+; CREATE_NO_WINDOW keeps it invisible. Offline = silent.
        let Ok(out) = std::process::Command::new("curl.exe")
            .args(["-s", "-m", "5", "-H", "Accept: application/vnd.github+json"])
            .arg(format!("https://api.github.com/repos/{REPO}/releases/latest"))
            .creation_flags(0x0800_0000)
            .output()
        else {
            return;
        };
        let body = String::from_utf8_lossy(&out.stdout);
        let Some(tag) = tag_name(&body) else { return };
        if parse_version(tag) <= parse_version(env!("CARGO_PKG_VERSION")) {
            return;
        }
        let msg = format!(
            "Une nouvelle version de {APP} est disponible : {tag}\n(version installée : {})\n\nOuvrir la page de téléchargement ?",
            env!("CARGO_PKG_VERSION")
        );
        if win::ask(&msg, APP) {
            win::open_url(&format!("https://github.com/{REPO}/releases/latest"));
        }
    });
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// Loads libmpv-2.dll (next to the exe, else from PATH) and resolves the client API.
/// The returned `Mpv` has no handle yet (`h` is null); keep the `Library` alive while it is used.
fn load_mpv(dir: &Path) -> Result<(Library, Mpv), String> {
    // SAFETY: loading libmpv runs its DllMain, which has no unusual requirements.
    let lib = unsafe { Library::new(dir.join("libmpv-2.dll")) }
        .or_else(|_| unsafe { Library::new("libmpv-2.dll") })
        .map_err(|e| format!("libmpv-2.dll introuvable à côté de visiblement.exe.

{e}"))?;
    macro_rules! sym {
        ($name:literal) => {
            // SAFETY: each target type matches the declaration in mpv/client.h (client API v2).
            *unsafe { lib.get(concat!($name, " ").as_bytes()) }
                .map_err(|e| format!("Fonction {} absente de libmpv : {e}", $name))?
        };
    }
    let m = Mpv {
        h: ptr::null_mut(),
        create: sym!("mpv_create"),
        initialize: sym!("mpv_initialize"),
        wait_event: sym!("mpv_wait_event"),
        destroy: sym!("mpv_terminate_destroy"),
        hook_add: sym!("mpv_hook_add"),
        hook_continue: sym!("mpv_hook_continue"),
        set_option: sym!("mpv_set_option_string"),
        set_property: sym!("mpv_set_property_string"),
        set_property_raw: sym!("mpv_set_property"),
        get_property: sym!("mpv_get_property_string"),
        observe: sym!("mpv_observe_property"),
        command: sym!("mpv_command"),
        free: sym!("mpv_free"),
    };
    Ok((lib, m))
}

fn run() -> Result<(), String> {
    let dir = exe_dir();
    let (_lib, api) = load_mpv(&dir)?;
    let (initialize, wait_event, destroy) = (api.initialize, api.wait_event, api.destroy);
    let h = unsafe { (api.create)() };
    if h.is_null() {
        return Err("Impossible de créer le lecteur mpv.".into());
    }
    let m = Mpv { h, ..api };

    // Skin (uosc), menu (input.conf) and logo ship in portable_config next to the exe.
    let config = dir.join("portable_config");
    m.option("config", "yes");
    m.option("config-dir", &config.to_string_lossy());
    for (k, v) in OPTIONS {
        m.option(k, v);
    }
    // Troubleshooting: VISIBLEMENT_LOG=C:\path\log.txt writes mpv's log there.
    if let Some(log) = std::env::var_os("VISIBLEMENT_LOG") {
        m.option("log-file", &log.to_string_lossy());
    }
    if unsafe { initialize(h) } < 0 {
        unsafe { destroy(h) };
        return Err("Impossible d'initialiser mpv.".into());
    }
    check_for_update();
    // Before each file opens: turn WAV markers (cue points, After Effects XMP) into chapters.
    unsafe { (m.hook_add)(h, 0, c"on_load".as_ptr(), 0) };
    let mut idle = Idle::new(&config);
    for p in ["idle-active", "osd-width", "osd-height", "mouse-pos", "video-params", "current-tracks/video", "current-tracks/audio"] {
        m.observe(p);
    }

    // Every argument is a file (or URL) to play, in order: also covers drag & drop onto the .exe.
    // The first file replaces the (empty) playlist so autocreate-playlist starts on it, not on its neighbour.
    for (i, arg) in std::env::args_os().skip(1).enumerate() {
        m.cmd(&["loadfile", &arg.to_string_lossy(), if i == 0 { "replace" } else { "append" }]);
    }

    let mut workers = Vec::new();
    let mut icon_set = false;
    let mut loader = Loader { since: None, shown: false };
    loop {
        // While the logo is on screen, wake up at ~60 Hz to animate the eyes.
        let timeout = if idle.shown || loader.active() { 0.016 } else { -1.0 };
        // SAFETY: mpv_wait_event never returns NULL; the event lives until the next call.
        let ev = unsafe { &*wait_event(h, timeout) };
        if !icon_set {
            icon_set = win::set_window_icon();
        }
        match ev.id {
            EVENT_SHUTDOWN => break,
            EVENT_NONE => {
                idle.draw_eyes(&m);
                loader.tick(&m);
            }
            EVENT_FILE_LOADED | EVENT_END_FILE => loader.stop(&m),
            EVENT_START_FILE => {
                loader.start();
                let anim = m.get("path").is_some_and(|p| is_animated_image(&p));
                m.set("loop-file", if anim { "inf" } else { "no" });
            }
            EVENT_PROPERTY_CHANGE => {
                // SAFETY: data points to an mpv_event_property whose first field is the name.
                let name = unsafe { CStr::from_ptr(*(ev.data as *const *const c_char)) };
                if name.to_bytes() == b"video-params" {
                    // Shows the checkerboard button in the bar (portable_config/script-opts/uosc.conf).
                    m.set_flag("user-data/visiblement/alpha", has_alpha(&m));
                    idle.layout(&m);
                } else if name.to_bytes() == b"mouse-pos" {
                    idle.draw_eyes(&m);
                } else {
                    idle.layout(&m);
                }
            }
            EVENT_HOOK => {
                // SAFETY: data points to an mpv_event_hook { name, id } for this event id.
                let id = unsafe { *(ev.data as *const u8).add(std::mem::size_of::<*const c_char>()).cast::<u64>() };
                load_markers(&m);
                unsafe { (m.hook_continue)(h, id) };
            }
            EVENT_CLIENT_MESSAGE => {
                // SAFETY: data points to an mpv_event_client_message for this event id.
                let msg = unsafe { &*(ev.data as *const ClientMessage) };
                if msg.num_args < 1 {
                    continue;
                }
                let name = unsafe { CStr::from_ptr(*msg.args) }.to_string_lossy();
                match name.as_ref() {
                    "visiblement-save-frame" => workers.push(save_frame(m)),
                    "visiblement-copy-frame" => workers.push(copy_frame(m)),
                    "visiblement-export" => workers.push(editor::export(m)),
                    _ => {}
                }
            }
            _ => {}
        }
        workers.retain(|w: &thread::JoinHandle<()>| !w.is_finished());
    }
    // A save dialog may still be open: the handle must outlive it.
    for w in workers {
        let _ = w.join();
    }
    unsafe { destroy(h) };
    Ok(())
}

mod win {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    type Hwnd = *mut c_void;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowExW(parent: Hwnd, after: Hwnd, class: *const u16, title: *const u16) -> Hwnd;
        fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
        fn LoadImageW(inst: *mut c_void, name: *const u16, kind: u32, cx: i32, cy: i32, flags: u32) -> *mut c_void;
        fn SendMessageW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
        fn GetSystemMetrics(index: i32) -> i32;
        pub fn MessageBoxW(hwnd: Hwnd, text: *const u16, caption: *const u16, flags: u32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(hwnd: Hwnd, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> *mut c_void;
    }

    /// Yes / No question in front of the player. True on Yes.
    pub fn ask(text: &str, caption: &str) -> bool {
        // MB_YESNO | MB_ICONINFORMATION | MB_SETFOREGROUND | MB_TOPMOST; IDYES = 6.
        let flags = 0x4 | 0x40 | 0x1_0000 | 0x4_0000;
        // SAFETY: NUL-terminated UTF-16 buffers that live for the call.
        unsafe { MessageBoxW(null_mut(), wide(text).as_ptr(), wide(caption).as_ptr(), flags) == 6 }
    }

    /// Opens a URL in the default browser.
    pub fn open_url(url: &str) {
        // SAFETY: NUL-terminated UTF-16 buffers that live for the call. 1 = SW_SHOWNORMAL.
        unsafe { ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(url).as_ptr(), null(), null(), 1) };
    }

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// Give the mpv window our icon (resource id 1, embedded by build.rs). False until the window exists.
    pub fn set_window_icon() -> bool {
        let class = wide("mpv");
        // SAFETY: plain Win32 calls with valid NUL-terminated strings; MAKEINTRESOURCE(1) is the pointer value 1.
        unsafe {
            let mut hwnd = null_mut();
            loop {
                hwnd = FindWindowExW(null_mut(), hwnd, class.as_ptr(), null());
                if hwnd.is_null() {
                    return false;
                }
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid == std::process::id() {
                    break;
                }
            }
            let inst = GetModuleHandleW(null());
            // (ICON_BIG, SM_CXICON) and (ICON_SMALL, SM_CXSMICON)
            for (which, metric) in [(1usize, 11), (0, 49)] {
                let size = GetSystemMetrics(metric);
                let icon = LoadImageW(inst, std::ptr::without_provenance(1), 1, size, size, 0); // IMAGE_ICON
                if !icon.is_null() {
                    SendMessageW(hwnd, 0x80, which, icon as isize); // WM_SETICON
                }
            }
            true
        }
    }
}

fn alert(msg: &str) {
    // SAFETY: both buffers are NUL-terminated UTF-16 and live for the call. 0x10 = MB_ICONERROR.
    unsafe { win::MessageBoxW(ptr::null_mut(), win::wide(msg).as_ptr(), win::wide(APP).as_ptr(), 0x10) };
}

fn main() {
    if let Err(e) = run() {
        alert(&e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animated_detection() {
        assert!(is_animated_image(r"C:\a\B.GIF"));
        assert!(is_animated_image("x.webp"));
        assert!(!is_animated_image("x.mp4"));
        assert!(!is_animated_image("gif.mov"));
    }

    #[test]
    fn loader_dots_stay_visible_and_move() {
        for k in 0..200 {
            let t = k as f64 * 0.05;
            for (i, j) in [(0, 0), (2, 2), (4, 1)] {
                let v = dot_level(t, i, j);
                assert!((0.12..=1.0).contains(&v), "{v}");
            }
        }
        assert!((dot_level(0.1, 2, 2) - dot_level(0.1, 0, 0)).abs() > 0.01);
    }

    /// Real encode through libmpv (needs target/release/libmpv-2.dll and ffmpeg on PATH):
    /// cargo test --release -- --ignored
    #[test]
    #[ignore]
    fn exports_prores_alpha_and_wav_excerpts() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release");
        let (_lib, m) = load_mpv(&dir).expect("libmpv-2.dll in target/release");
        let tmp = std::env::temp_dir().join("visiblement-export-test");
        let _ = std::fs::create_dir_all(&tmp);
        let ff = |args: &[&str]| assert!(std::process::Command::new("ffmpeg").args(args).status().unwrap().success());
        let src_mov = tmp.join("src.mov");
        ff(&["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=4,format=yuva444p",
            "-c:v", "prores_ks", "-profile:v", "4444", "-pix_fmt", "yuva444p10le", &src_mov.to_string_lossy()]);
        let out = tmp.join("cut.mov");
        let _ = std::fs::remove_file(&out);
        let settings = editor::codecs("mov", Some("prores"), None, true);
        assert!(editor::encode(&m, &src_mov.to_string_lossy(), &out, Some(1.0), Some(3.0), &settings, |_| {}));
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "stream=codec_name,pix_fmt:format=duration", "-of", "csv=p=0"])
            .arg(&out).output().unwrap();
        let info = String::from_utf8_lossy(&probe.stdout);
        assert!(info.contains("prores") && info.contains("yuva444p"), "{info}");
        let secs: f64 = info.lines().last().unwrap().trim().parse().unwrap();
        assert!((secs - 2.0).abs() < 0.1, "{secs}");
    }

    #[test]
    fn alpha_pixel_formats() {
        for f in ["yuva444p10le", "rgba", "bgra", "gbrap", "ya8", "pal8", "argb"] {
            assert!(is_alpha_format(f), "{f}");
        }
        for f in ["yuv420p", "yuv422p10le", "nv12", "d3d11", "p010", "rgb24", "gbrp"] {
            assert!(!is_alpha_format(f), "{f}");
        }
    }

    #[test]
    fn rgb_expands_to_opaque_rgba() {
        assert_eq!(to_rgba(&[1, 2, 3, 4, 5, 6], png::ColorType::Rgb), Some(vec![1, 2, 3, 255, 4, 5, 6, 255]));
        assert_eq!(to_rgba(&[0], png::ColorType::Grayscale), None);
    }

    #[test]
    fn eyes_look_toward_the_mouse_but_stay_in_place() {
        let (dx, dy) = gaze_offset(1000.0, 0.0);
        assert!((dx - 44.0).abs() < 1e-9 && dy.abs() < 1e-9);
        let (dx, _) = gaze_offset(9.0, 0.0);
        assert!((dx - 3.0).abs() < 1e-9);
        assert_eq!(gaze_offset(0.0, 0.0), (0.0, 0.0));
    }

    #[test]
    fn update_versions_compare_numerically() {
        assert!(parse_version("v0.10.0") > parse_version("0.9.3"));
        assert!(parse_version("v0.1.0") <= parse_version("0.1.0"));
        assert_eq!(tag_name(r#"{"url":"x","tag_name": "v1.2.0","name":"y"}"#), Some("v1.2.0"));
        assert_eq!(tag_name(r#"{"message":"Not Found"}"#), None);
    }
}
