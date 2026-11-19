// No console window in release builds; errors go to a message box.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::{ptr, thread};

mod app;
mod editor;
mod markers;

use app::run;

const APP: &str = "Visiblement";

type Handle = *mut c_void;

const EVENT_SHUTDOWN: c_int = 1;
const EVENT_START_FILE: c_int = 6;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const EVENT_PROPERTY_CHANGE: c_int = 22;
const EVENT_HOOK: c_int = 25;


// Tuned for speed: GPU decode (NVDEC/D3D11VA/...) with safe fallback, modern GPU renderer.
// mpv only draws the picture; the interface is ui/index.html in a WebView on top (see app.rs).
const OPTIONS: &[(&str, &str)] = &[
    ("vo", "gpu-next"),
    ("hwdec", "auto-safe"),
    ("idle", "yes"),
    ("force-window", "yes"),
    ("keep-open", "yes"),    // keep the last frame instead of closing
    ("image-display-duration", "inf"), // still images (png, static webp) stay on screen
    ("autocreate-playlist", "filter"), // previous / next = the other media in the same folder
    ("directory-filter-types", "video,image"),
    ("input-default-bindings", "no"), // keys and mouse are handled by the web UI
    ("input-vo-keyboard", "no"),
    ("input-cursor", "no"),
    ("osc", "no"),
    ("osd-level", "0"),
    ("background", "color"),  // transparent videos: plain background; the alpha button switches to a checkerboard
    ("background-color", "#1b1a1e"),
    ("screenshot-high-bit-depth", "no"), // 8-bit frames: what the clipboard and most apps expect
];

#[repr(C)]
struct Event {
    id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
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
        app::toast(msg);
    }
}

/// WAV markers -> temporary FFmetadata file -> chapters for this file only (shown on the timeline).
fn load_markers(m: &Mpv) {
    let Some(path) = m.get("path") else { return };
    let lower = path.to_ascii_lowercase();
    if (lower.ends_with(".webm") || lower.ends_with(".mkv")) && webm_has_alpha(Path::new(&path)) {
        // Transparent WebM: FFmpeg's own VP8/VP9 decoders drop the alpha plane, libvpx keeps it.
        m.set("file-local-options/vd", "libvpx-vp9,libvpx,");
    }
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

/// Matroska/WebM track with AlphaMode = 1 (element 0x53C0), found in the file's header.
fn webm_has_alpha(path: &Path) -> bool {
    use std::io::Read;
    let mut head = vec![0u8; 1 << 20];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    head[..n].windows(4).any(|w| w == [0x53, 0xC0, 0x81, 0x01])
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
/// Asks GitHub for the latest release, in the background.
/// At startup (`manual` false): a newer version opens the prompt, unless "Ne plus demander" was
/// ticked, in which case only the discreet update button lights up.
/// From that button (`manual` true): always answers, with the prompt or a toast.
fn check_for_update(manual: bool) {
    use std::os::windows::process::CommandExt;
    thread::spawn(move || {
        // curl.exe ships with Windows 10+; CREATE_NO_WINDOW keeps it invisible. Offline = silent.
        let Ok(out) = std::process::Command::new("curl.exe")
            .args(["-s", "-m", "5", "-H", "Accept: application/vnd.github+json"])
            .arg(format!("https://api.github.com/repos/{REPO}/releases/latest"))
            .creation_flags(0x0800_0000)
            .output()
        else {
            if manual {
                app::toast("Impossible de vérifier les mises à jour");
            }
            return;
        };
        let body = String::from_utf8_lossy(&out.stdout);
        let Some(tag) = tag_name(&body) else {
            if manual {
                app::toast("Impossible de vérifier les mises à jour (hors ligne ?)");
            }
            return;
        };
        if parse_version(tag) <= parse_version(env!("CARGO_PKG_VERSION")) {
            if manual {
                app::toast(&format!("Visiblement est à jour ({})", env!("CARGO_PKG_VERSION")));
            }
            return;
        }
        app::offer_update(tag, manual || !never_ask_update());
    });
}

/// Settings kept across launches: %APPDATA%\Visiblement\settings.json.
fn settings_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join(APP).join("settings.json"))
}

fn read_settings() -> serde_json::Map<String, serde_json::Value> {
    settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn never_ask_update() -> bool {
    read_settings().get("never_ask_update").and_then(serde_json::Value::as_bool).unwrap_or(false)
}

fn set_never_ask_update(never: bool) {
    let Some(path) = settings_path() else { return };
    let mut settings = read_settings();
    settings.insert("never_ask_update".into(), never.into());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, serde_json::Value::Object(settings).to_string());
}

fn open_release_page() {
    win::open_url(&format!("https://github.com/{REPO}/releases/latest"));
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
        get_property: sym!("mpv_get_property_string"),
        observe: sym!("mpv_observe_property"),
        command: sym!("mpv_command"),
        free: sym!("mpv_free"),
    };
    Ok((lib, m))
}

mod win {
    /// Opens Explorer with the file selected.
    pub fn show_in_folder(path: &str) {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.replace('/', "\\"))).spawn();
    }
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    type Hwnd = *mut c_void;

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn MessageBoxW(hwnd: Hwnd, text: *const u16, caption: *const u16, flags: u32) -> i32;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(hwnd: Hwnd, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> *mut c_void;
    }


    /// Opens a URL in the default browser.
    pub fn open_url(url: &str) {
        // SAFETY: NUL-terminated UTF-16 buffers that live for the call. 1 = SW_SHOWNORMAL.
        unsafe { ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(url).as_ptr(), null(), null(), 1) };
    }

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
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

    /// Needs target/release/libmpv-2.dll: cargo test --release -- --ignored
    #[test]
    #[ignore]
    fn checkerboard_toggles_in_mpv() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release");
        let (_lib, api) = load_mpv(&dir).expect("libmpv-2.dll in target/release");
        let h = unsafe { (api.create)() };
        let m = Mpv { h, ..api };
        m.option("vo", "null");
        m.option("background", "color");
        assert!(unsafe { (m.initialize)(h) } >= 0);
        m.set("background", "tiles");
        assert_eq!(m.get("background").as_deref(), Some("tiles"));
        m.set("background", "color");
        assert_eq!(m.get("background").as_deref(), Some("color"));
        unsafe { (m.destroy)(h) };
    }

    #[test]
    fn webm_alpha_flag_detection() {
        let tmp = std::env::temp_dir().join("visiblement-alpha-test.webm");
        std::fs::write(&tmp, [0x1A, 0x45, 0xDF, 0xA3, 0x00, 0x53, 0xC0, 0x81, 0x01, 0x00]).unwrap();
        assert!(webm_has_alpha(&tmp));
        std::fs::write(&tmp, [0x1A, 0x45, 0xDF, 0xA3, 0x00, 0x53, 0xC0, 0x81, 0x00]).unwrap();
        assert!(!webm_has_alpha(&tmp));
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
    fn update_versions_compare_numerically() {
        assert!(parse_version("v0.10.0") > parse_version("0.9.3"));
        assert!(parse_version("v0.1.0") <= parse_version("0.1.0"));
        assert_eq!(tag_name(r#"{"url":"x","tag_name": "v1.2.0","name":"y"}"#), Some("v1.2.0"));
        assert_eq!(tag_name(r#"{"message":"Not Found"}"#), None);
    }
}
