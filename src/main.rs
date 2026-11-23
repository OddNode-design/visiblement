// No console window in release builds; errors go to a message box.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::{ptr, thread};

mod app;
mod editor;
mod markers;
mod ndi;
mod stream;
mod single;
mod update;

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
    stream_cb_add_ro: unsafe extern "C" fn(Handle, *const c_char, *mut c_void, ndi::OpenFn) -> c_int,
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
        // Transparent WebM: GPU decoding and FFmpeg's own VP8/VP9 decoders drop the alpha plane;
        // libvpx (software) keeps it.
        m.set("file-local-options/hwdec", "no");
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

/// 360° video: the spatial-media metadata (Google's injector: v1 XML in a uuid box, or the v2 `sv3d` box)
/// in the `moov` of an MP4 / MOV.
fn is_spherical(path: &str) -> bool {
    read_moov(path).is_some_and(|moov| spherical_in(&moov))
}

/// The images of the numbered sequence `image` belongs to (shot_0001.png, shot_0002.png…): same
/// name before the number and same extension, in the order of their numbers (so unpadded 1, 2,
/// 10 sort right, and shot_final.png stays out). Returns the name before the number, the
/// extension and the images with their numbers.
fn sequence_frames(image: &Path) -> Option<(String, String, Vec<(u64, PathBuf)>)> {
    let stem = image.file_stem()?.to_string_lossy().into_owned();
    let ext = image.extension()?.to_string_lossy().into_owned();
    let prefix = stem.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
    if prefix.len() == stem.len() {
        return None; // no frame number
    }
    let mut frames: Vec<(u64, PathBuf)> = std::fs::read_dir(image.parent()?)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let same_ext = path.extension()?.to_string_lossy().eq_ignore_ascii_case(&ext);
            let number = path.file_stem()?.to_string_lossy().strip_prefix(prefix.as_str())?.to_string();
            (same_ext && !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())).then(|| Some((number.parse().ok()?, path)))?
        })
        .collect();
    frames.sort_by_key(|(n, _)| *n);
    Some((prefix, ext, frames))
}

/// The sequence as an mpv address: "mf://@<list>", a text file naming every image in order.
/// The list is named after the sequence, so the converted file is too.
fn sequence_url(image: &Path) -> Option<String> {
    let (prefix, _, frames) = sequence_frames(image)?;
    if frames.is_empty() {
        return None;
    }
    let dir = std::env::temp_dir().join(format!("visiblement-seq-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let name = if prefix.trim_matches(['_', '-', '.', ' ']).is_empty() { "sequence".to_string() } else { prefix };
    let list = dir.join(format!("{name}.txt"));
    let body: String = frames.iter().map(|(_, p)| format!("{}\n", p.to_string_lossy())).collect();
    std::fs::write(&list, body).ok()?;
    Some(format!("mf://@{}", list.to_string_lossy()))
}

/// "shot_####.png · 120 images" for the Convertir dialog, with the missing numbers if any.
fn sequence_label(image: &Path) -> Option<String> {
    let (prefix, ext, frames) = sequence_frames(image)?;
    let stem = image.file_stem()?.to_string_lossy();
    let digits = stem.len() - stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (first, last) = (frames.first()?.0, frames.last()?.0);
    let missing = (last - first + 1) as usize - frames.len();
    let gap = if missing > 0 { format!(" · {missing} manquantes") } else { String::new() };
    Some(format!("{prefix}{}.{} · {} images{gap}", "#".repeat(digits), ext.to_ascii_lowercase(), frames.len()))
}

/// The `moov` box of an MP4 / MOV (where tracks are described), wherever it sits in the file.
fn read_moov(path: &str) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return None };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut pos = 0u64;
    while pos + 8 <= len {
        let mut h = [0u8; 16];
        if f.seek(SeekFrom::Start(pos)).is_err() || f.read_exact(&mut h[..8]).is_err() {
            return None;
        }
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        if size == 1 {
            if f.read_exact(&mut h[8..]).is_err() {
                return None;
            }
            size = u64::from_be_bytes(h[8..].try_into().unwrap());
        } else if size == 0 {
            size = len - pos;
        }
        if size < 8 {
            return None;
        }
        if &h[4..8] == b"moov" {
            let mut moov = vec![0; size.min(64 << 20) as usize];
            return (f.seek(SeekFrom::Start(pos)).is_ok() && f.read_exact(&mut moov).is_ok()).then_some(moov);
        }
        pos += size;
    }
    None
}

fn spherical_in(moov: &[u8]) -> bool {
    const UUID: [u8; 16] = [0xff, 0xcc, 0x82, 0x63, 0xf8, 0x55, 0x4a, 0x93, 0x88, 0x14, 0x58, 0x7a, 0x02, 0x52, 0x1f, 0xdd];
    let find = |n: &[u8]| moov.windows(n.len()).position(|w| w == n);
    find(b"sv3d").is_some()
        || find(&UUID).is_some_and(|i| String::from_utf8_lossy(&moov[i..moov.len().min(i + 4096)]).contains("Spherical>true"))
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
            .set_title(crate::tr("Enregistrer la frame", "Save the frame"))
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

/// Newest release tag from the redirect of github.com/<repo>/releases/latest (no API quota).
fn latest_tag_by_redirect() -> Option<String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("curl.exe")
        .args(["-s", "-m", "5", "-o", "NUL", "-w", "%{redirect_url}"])
        .arg(format!("https://github.com/{REPO}/releases/latest"))
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    tag_from_release_url(&String::from_utf8_lossy(&out.stdout))
}

fn tag_from_release_url(url: &str) -> Option<String> {
    let tag = url.trim().rsplit_once("/releases/tag/")?.1;
    (tag.starts_with('v') && tag.len() > 1).then(|| tag.to_string())
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
        // The API allows 60 anonymous calls an hour per IP address. When it refuses (or answers
        // nothing), the github.com "latest release" page still redirects to the newest tag, and the
        // .msi sits at a predictable address.
        let (tag, asset) = match tag_name(&body) {
            Some(tag) => (tag.to_string(), update::msi_asset(&body)),
            None => match latest_tag_by_redirect() {
                Some(tag) => {
                    let url = format!(
                        "https://github.com/{REPO}/releases/download/{tag}/Visiblement-{}-x64.msi",
                        tag.trim_start_matches('v')
                    );
                    (tag, Some((url, 0)))
                }
                None => {
                    if manual {
                        app::toast("Impossible de vérifier les mises à jour (hors ligne ?)");
                    }
                    return;
                }
            },
        };
        let tag = tag.as_str();
        if parse_version(tag) <= parse_version(env!("CARGO_PKG_VERSION")) {
            if manual {
                app::toast(&format!("Visiblement est à jour ({})", env!("CARGO_PKG_VERSION")));
            }
            return;
        }
        update::remember(tag, asset);
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

fn write_settings(settings: serde_json::Map<String, serde_json::Value>) {
    let Some(path) = settings_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, serde_json::Value::Object(settings).to_string());
}

fn set_never_ask_update(never: bool) {
    let mut settings = read_settings();
    settings.insert("never_ask_update".into(), never.into());
    write_settings(settings);
}

fn save_volume(volume: Option<String>, mute: Option<String>) {
    let mut settings = read_settings();
    for (k, v) in [("volume", volume), ("mute", mute)] {
        if let Some(v) = v {
            settings.insert(k.into(), v.into());
        }
    }
    write_settings(settings);
}

/// mpv options restoring the last session's volume / mute (settings.json "volume", "mute").
fn saved_volume() -> Vec<(&'static str, String)> {
    let settings = read_settings();
    ["volume", "mute"]
        .into_iter()
        .filter_map(|k| settings.get(k).and_then(serde_json::Value::as_str).map(|v| (k, v.to_string())))
        .collect()
}

/// Interface language, "fr" or "en" (settings.json "lang"; the page picks one on first launch).
fn set_lang(lang: &str) {
    if lang == "fr" || lang == "en" {
        let mut settings = read_settings();
        settings.insert("lang".into(), lang.into());
        write_settings(settings);
    }
}

/// The French or English text of a native dialog, after the language setting.
/// Toasts are translated by the page; only what Windows draws comes through here
/// (dialogs are rare, so reading the setting each time is fine).
fn tr(fr: &'static str, en: &'static str) -> &'static str {
    if read_settings().get("lang").and_then(serde_json::Value::as_str) == Some("en") { en } else { fr }
}

const RECENT_MAX: usize = 10;

/// Recently opened media, newest first (settings.json "recent").
fn recent_media() -> Vec<String> {
    read_settings()
        .get("recent")
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn add_recent_media(path: &str) {
    let mut list = recent_media();
    list.retain(|p| !p.eq_ignore_ascii_case(path));
    list.insert(0, path.to_string());
    list.truncate(RECENT_MAX);
    let mut settings = read_settings();
    settings.insert("recent".into(), list.into());
    write_settings(settings);
}

/// Video capture devices, by the names DirectShow uses: plugged-in webcams and capture cards,
/// plus virtual cameras registered as DirectShow filters (OBS Virtual Camera, NVIDIA Broadcast,
/// NDI Webcam from NDI Tools…).
fn list_cameras() -> Vec<String> {
    use std::os::windows::process::CommandExt;
    let script = r"
        $names = @()
        $names += Get-CimInstance Win32_PnPEntity | Where-Object { $_.PNPClass -in 'Camera','Image' -and $_.Status -eq 'OK' } | ForEach-Object { $_.Name }
        $cat = 'HKLM:\SOFTWARE\Classes\CLSID\{860BB310-5D01-11d0-BD3B-00A0C911CE86}\Instance'
        $names += Get-ChildItem $cat -ErrorAction SilentlyContinue | ForEach-Object { (Get-ItemProperty $_.PSPath).FriendlyName }
        $names | Where-Object { $_ } | Sort-Object -Unique";
    std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(0x0800_0000)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Everything the "Infos" panel shows, gathered from mpv and the file system.
fn media_info(m: &Mpv) -> serde_json::Value {
    let props = [
        "filename", "path", "file-format", "file-size", "duration", "container-fps", "estimated-frame-count",
        "video-codec", "audio-codec", "hwdec-current", "video-params", "audio-params", "metadata", "track-list",
        "video-bitrate", "audio-bitrate", "estimated-vf-fps", "display-fps", "frame-drop-count",
        "decoder-frame-drop-count", "avsync", "demuxer-cache-duration", "speed",
    ];
    let mut out = serde_json::Map::new();
    for p in props {
        out.insert(p.into(), m.get(p).map_or(serde_json::Value::Null, serde_json::Value::String));
    }
    if let Some(meta) = m.get("path").and_then(|p| std::fs::metadata(p).ok()) {
        let ms = |t: std::io::Result<std::time::SystemTime>| {
            t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64)
        };
        out.insert("modified".into(), ms(meta.modified()).into());
        out.insert("created".into(), ms(meta.created()).into());
    }
    serde_json::Value::Object(out)
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
        stream_cb_add_ro: sym!("mpv_stream_cb_add_ro"),
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
        fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
        fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
        fn IsZoomed(hwnd: Hwnd) -> i32;
        fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
        fn MonitorFromWindow(hwnd: Hwnd, flags: u32) -> Hwnd;
        fn RedrawWindow(hwnd: Hwnd, rect: *const c_void, region: Hwnd, flags: u32) -> i32;
        fn GetWindowRect(hwnd: Hwnd, rect: *mut WinRect) -> i32;
        fn GetMonitorInfoW(monitor: Hwnd, info: *mut MonitorInfo) -> i32;
    }

    #[repr(C)]
    #[derive(Default)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct MonitorInfo {
        size: u32,
        monitor: WinRect,
        work: WinRect,
        flags: u32,
    }

    /// (whole monitor, work area without the taskbar) of the monitor holding the window.
    pub type Rect = (i32, i32, i32, i32);

    pub fn monitor_rects(hwnd: isize) -> Option<(Rect, Rect)> {
        let mut info = MonitorInfo { size: std::mem::size_of::<MonitorInfo>() as u32, ..Default::default() };
        // SAFETY: valid window handle; `info` is a correctly sized MONITORINFO. 2 = MONITOR_DEFAULTTONEAREST.
        let ok = unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd as Hwnd, 2), &mut info) } != 0;
        let r = |r: &WinRect| (r.left, r.top, r.right - r.left, r.bottom - r.top);
        ok.then(|| (r(&info.monitor), r(&info.work)))
    }

    /// (x, y, width, height) of a window, as Windows has it right now.
    pub fn window_rect(hwnd: isize) -> Option<Rect> {
        let mut r = WinRect::default();
        // SAFETY: valid window handle and a correctly laid out RECT.
        let ok = unsafe { GetWindowRect(hwnd as Hwnd, &mut r) } != 0;
        ok.then(|| (r.left, r.top, r.right - r.left, r.bottom - r.top))
    }

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(p: *mut Point) -> i32;
        fn BeginDeferWindowPos(n: i32) -> *mut c_void;
        fn DeferWindowPos(info: *mut c_void, hwnd: Hwnd, after: Hwnd, x: i32, y: i32, w: i32, h: i32, flags: u32) -> *mut c_void;
        fn EndDeferWindowPos(info: *mut c_void) -> i32;
    }

    /// The mouse in physical screen pixels, right on any monitor whatever its scale.
    pub fn cursor() -> Option<(f64, f64)> {
        let mut p = Point { x: 0, y: 0 };
        // SAFETY: writes one POINT.
        (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x as f64, p.y as f64))
    }

    /// Moves / resizes the UI window and the video window under it in one go: Windows applies both
    /// together, so neither shows a moved-but-not-resized frame or lags behind the other.
    pub fn move_pair(ui: isize, video: isize, (x, y, w, h): Rect) {
        const FLAGS: u32 = 0x0004 | 0x0010; // SWP_NOZORDER | SWP_NOACTIVATE
        // SAFETY: a deferred batch of two SetWindowPos on our own windows.
        unsafe {
            let mut d = BeginDeferWindowPos(2);
            for hwnd in [ui, video] {
                if !d.is_null() {
                    d = DeferWindowPos(d, hwnd as Hwnd, null_mut(), x, y, w.max(1), h.max(1), FLAGS);
                }
            }
            if !d.is_null() {
                EndDeferWindowPos(d);
            }
        }
    }

    /// Moves and resizes a window without activating it or changing its z-order.
    pub fn set_rect(hwnd: isize, (x, y, w, h): Rect) {
        // SAFETY: plain Win32 call on our own window. SWP_NOZORDER | SWP_NOACTIVATE.
        unsafe {
            SetWindowPos(hwnd as Hwnd, null_mut(), x, y, w.max(1), h.max(1), 0x0004 | 0x0010);
            // Repaint everything: the transparent window would otherwise show stale pixels in the new area.
            // RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW
            RedrawWindow(hwnd as Hwnd, null(), null_mut(), 0x0001 | 0x0004 | 0x0080 | 0x0100);
        }
    }

    #[link(name = "comctl32")]
    unsafe extern "system" {
        fn SetWindowSubclass(hwnd: Hwnd, proc: SubclassProc, id: usize, data: usize) -> i32;
        fn DefSubclassProc(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
    }
    type SubclassProc = unsafe extern "system" fn(Hwnd, u32, usize, isize, usize, usize) -> isize;

    unsafe extern "system" fn no_frame(hwnd: Hwnd, msg: u32, w: usize, l: isize, _id: usize, _data: usize) -> isize {
        match msg {
            0x0085 => 0, // WM_NCPAINT: nothing to paint, the window has no visible frame
            0x0086 => 1, // WM_NCACTIVATE: accept the (de)activation without drawing a caption
            // SAFETY: forwarding to the next window procedure with the original arguments.
            _ => unsafe { DefSubclassProc(hwnd, msg, w, l) },
        }
    }

    /// The UI window is transparent and keeps caption styles (tao re-applies them): on activation,
    /// Windows would paint a classic frame straight into its transparent surface, where it stays
    /// visible as a ghost. This makes the window never paint a non-client frame.
    pub fn never_paint_frame(hwnd: isize) {
        // SAFETY: subclassing our own window on its own (UI) thread.
        unsafe { SetWindowSubclass(hwnd as Hwnd, no_frame, 1, 0) };
    }

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(hwnd: Hwnd, attr: u32, value: *const c_void, size: u32) -> i32;
    }

    /// Windows 11 rounded corners (DWMWCP_ROUND) or square ones (DWMWCP_DONOTROUND); ignored on
    /// Windows 10, which has square windows anyway.
    pub fn round_corners(hwnd: isize, round: bool) {
        let pref: u32 = if round { 2 } else { 1 };
        let no_border: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE: the app draws its own thin edge
        // SAFETY: DWMWA_WINDOW_CORNER_PREFERENCE (33) and DWMWA_BORDER_COLOR (34), u32 values, our own window.
        unsafe {
            DwmSetWindowAttribute(hwnd as Hwnd, 33, &pref as *const u32 as *const c_void, 4);
            DwmSetWindowAttribute(hwnd as Hwnd, 34, &no_border as *const u32 as *const c_void, 4);
        }
    }

    /// Removes the hidden caption / resize frame styles of a borderless window.
    pub fn strip_frame(hwnd: isize) {
        const FRAME: isize = 0x00C0_0000 | 0x0004_0000 | 0x0001_0000; // WS_CAPTION | WS_THICKFRAME | WS_MAXIMIZEBOX
        // SAFETY: plain Win32 calls on our own window. SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED.
        unsafe {
            let style = GetWindowLongPtrW(hwnd as Hwnd, -16);
            SetWindowLongPtrW(hwnd as Hwnd, -16, style & !FRAME);
            SetWindowPos(hwnd as Hwnd, null_mut(), 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0020);
        }
    }

    /// Puts the video window exactly at (x, y, w, h), frameless and not maximized. Windows can give
    /// it back a caption and resize borders (e.g. after the UI window leaves fullscreen), so the
    /// style is enforced every time rather than trusted.
    pub fn place_frameless(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
        const GWL_STYLE: i32 = -16;
        const FRAME: isize = 0x00C0_0000 | 0x0004_0000 | 0x0001_0000; // WS_CAPTION | WS_THICKFRAME | WS_MAXIMIZEBOX
        // SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED
        const FLAGS: u32 = 0x0004 | 0x0010 | 0x0020;
        let hwnd = hwnd as Hwnd;
        // SAFETY: plain Win32 calls on our own window handle.
        unsafe {
            if IsZoomed(hwnd) != 0 {
                ShowWindow(hwnd, 4); // SW_SHOWNOACTIVATE: restore without stealing focus
            }
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            if style & FRAME != 0 {
                SetWindowLongPtrW(hwnd, GWL_STYLE, style & !FRAME);
            }
            SetWindowPos(hwnd, null_mut(), x, y, w.max(1), h.max(1), FLAGS);
        }
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

/// Media Visiblement opens from Explorer (installer/visiblement.wxs), for Page Up / Page Down.
const MEDIA_EXT: [&str; 14] = ["mp4", "mov", "mkv", "webm", "avi", "m4v", "mxf", "gif", "webp", "apng", "wav", "mp3", "flac", "m4a"];

/// The media before (`step` -1) or after (+1) `current` in its folder, by name. Files a sync client
/// (Dropbox, OneDrive) keeps online only are skipped: opening them would start a download.
fn folder_neighbour(current: &Path, step: i32) -> Option<PathBuf> {
    let local = |e: &std::fs::DirEntry| {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            // OFFLINE, RECALL_ON_OPEN, RECALL_ON_DATA_ACCESS: a cloud placeholder, not on disk.
            e.metadata().is_ok_and(|m| m.is_file() && m.file_attributes() & (0x1000 | 0x40000 | 0x400000) == 0)
        }
        #[cfg(not(windows))]
        e.metadata().is_ok_and(|m| m.is_file())
    };
    let key = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut files: Vec<PathBuf> = std::fs::read_dir(current.parent()?)
        .ok()?
        .flatten()
        .filter(|e| {
            let ext = e.path().extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            MEDIA_EXT.contains(&ext.as_str()) && local(e)
        })
        .map(|e| e.path())
        .collect();
    files.sort_by_key(|p| key(p));
    let here = key(current);
    if step > 0 {
        files.into_iter().find(|p| key(p) > here)
    } else {
        files.into_iter().rev().find(|p| key(p) < here)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spherical_detection() {
        let mut v1 = b"....trak".to_vec();
        v1.extend([0xff, 0xcc, 0x82, 0x63, 0xf8, 0x55, 0x4a, 0x93, 0x88, 0x14, 0x58, 0x7a, 0x02, 0x52, 0x1f, 0xdd]);
        v1.extend(b"<rdf:SphericalVideo><GSpherical:Spherical>true</GSpherical:Spherical>");
        assert!(spherical_in(&v1));
        assert!(spherical_in(b"....sv3d....proj"));
        assert!(!spherical_in(b"....trak....mdia"));
    }

    /// VB_360_YES / VB_360_NO: a video with / without spatial-media metadata.
    #[test]
    #[ignore]
    fn spherical_files() {
        assert!(is_spherical(&std::env::var("VB_360_YES").unwrap()));
        assert!(!is_spherical(&std::env::var("VB_360_NO").unwrap()));
    }

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

    /// The playlist drop: `loadfile <file> insert-at-play <index>` puts the file before that entry.
    #[test]
    #[ignore]
    fn playlist_insert_at() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release");
        let (_lib, api) = load_mpv(&dir).expect("libmpv-2.dll in target/release");
        let h = unsafe { (api.create)() };
        let m = Mpv { h, ..api };
        m.option("vo", "null");
        m.option("ao", "null");
        m.option("idle", "yes");
        m.option("pause", "yes");
        assert!(unsafe { (m.initialize)(h) } >= 0);
        for f in ["a.mp4", "b.mp4", "c.mp4"] {
            m.cmd(&["loadfile", f, "append"]);
        }
        assert!(m.cmd(&["loadfile", "x.mp4", "insert-at-play", "1"]));
        let name = |i: usize| m.get(&format!("playlist/{i}/filename")).unwrap_or_default().rsplit(['/', '\\']).next().unwrap_or("").to_string();
        let order: Vec<String> = (0..4).map(name).collect();
        unsafe { (m.destroy)(h) };
        assert_eq!(order, ["a.mp4", "x.mp4", "b.mp4", "c.mp4"]);
    }

    /// The stop button: a long encode stopped after half a second ends at once and leaves no file.
    #[test]
    #[ignore]
    fn cancel_stops_and_removes_the_file() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release");
        let (_lib, m) = load_mpv(&dir).expect("libmpv-2.dll in target/release");
        let tmp = std::env::temp_dir().join("visiblement-cancel-test");
        let _ = std::fs::create_dir_all(&tmp);
        let src = tmp.join("long.mp4");
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=25:duration=60", "-pix_fmt", "yuv420p", "-preset", "ultrafast"])
            .arg(&src).status().unwrap().success());
        let out = tmp.join("long.mov");
        let (_, settings) = editor::preset("prores:4444", false, None).unwrap();
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(500));
            editor::CANCEL.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let t0 = std::time::Instant::now();
        let ok = editor::encode_any(&m, &src.to_string_lossy(), &out, &settings, |_| {});
        editor::CANCEL.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(!ok, "a stopped encode is not a success");
        assert!(t0.elapsed().as_secs_f64() < 3.0, "stopped quickly: {:?}", t0.elapsed());
        assert!(!out.exists(), "no half file left");
    }

    /// A PNG sequence (with alpha) into ProRes 4444 through mpv's mf:// reader.
    #[test]
    #[ignore]
    fn converts_png_sequence() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release");
        let (_lib, m) = load_mpv(&dir).expect("libmpv-2.dll in target/release");
        let tmp = std::env::temp_dir().join("visiblement-convert-test");
        let seq = tmp.join("seq");
        let _ = std::fs::create_dir_all(&seq);
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=1,format=rgba"])
            .arg(seq.join("shot_%04d.png")).status().unwrap().success());
        let src = sequence_url(&seq.join("shot_0007.png")).expect("sequence");
        assert_eq!(sequence_label(&seq.join("shot_0007.png")).as_deref(), Some("shot_####.png · 25 images"));
        assert!(src.starts_with("mf://@") && src.ends_with("shot_.txt"), "{src}");
        let out = tmp.join("seq.mov");
        let _ = std::fs::remove_file(&out);
        let (_, mut settings) = editor::preset("prores:4444", true, None).unwrap();
        editor::sequence_settings("prores:4444", "23.976", &mut settings);
        assert!(editor::encode_any(&m, &src, &out, &settings, |_| {}));
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-count_frames", "-show_entries", "stream=codec_name,pix_fmt,color_space,color_primaries,color_transfer,r_frame_rate,nb_read_frames", "-of", "csv=p=0"])
            .arg(&out).output().unwrap();
        let info = String::from_utf8_lossy(&probe.stdout);
        assert!(info.contains("prores,yuva444p"), "{info}");
        assert!(info.contains("bt709,bt709,bt709"), "Rec.709 written in the file: {info}");
        assert!(info.contains("24000/1001"), "exact 23.976: {info}");
        assert!(info.trim_end().ends_with(",25"), "every image, the last one too: {info}");
    }

    #[test]
    fn folder_neighbour_steps_by_name() {
        let dir = std::env::temp_dir().join(format!("vb-folder-step-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        for f in ["A.mov", "b.mp4", "c.txt", "d.wav"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let name = |p: Option<PathBuf>| p.map(|p| p.file_name().unwrap().to_string_lossy().into_owned());
        assert_eq!(name(folder_neighbour(&dir.join("b.mp4"), 1)).as_deref(), Some("d.wav"));
        assert_eq!(name(folder_neighbour(&dir.join("b.mp4"), -1)).as_deref(), Some("A.mov"));
        assert_eq!(folder_neighbour(&dir.join("d.wav"), 1), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sequence_order_and_strays() {
        let dir = std::env::temp_dir().join(format!("vb-seq-order-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        for f in ["plan_1.png", "plan_2.png", "plan_10.png", "plan_final.png", "plan_3.jpg", "autre_4.png"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let (_, _, frames) = sequence_frames(&dir.join("plan_2.png")).unwrap();
        let names: Vec<_> = frames.iter().map(|(_, p)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["plan_1.png", "plan_2.png", "plan_10.png"]);
        assert_eq!(sequence_label(&dir.join("plan_2.png")).as_deref(), Some("plan_#.png · 3 images · 7 manquantes"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every "Convertir" format through the bundled libmpv (needs ffmpeg / ffprobe on PATH).
    #[test]
    #[ignore]
    fn converts_to_every_format() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release");
        let (_lib, m) = load_mpv(&dir).expect("libmpv-2.dll in target/release");
        let tmp = std::env::temp_dir().join("visiblement-convert-test");
        let _ = std::fs::create_dir_all(&tmp);
        let src = tmp.join("src.mov");
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=322x240:rate=25:duration=1,format=yuva444p",
                "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:v", "prores_ks", "-profile:v", "4444",
                "-pix_fmt", "yuva444p10le", "-c:a", "pcm_s24le", "-shortest"])
            .arg(&src).status().unwrap().success());
        let mut failed = vec![];
        for id in editor::PRESETS {
            if std::env::var("VB_ONLY").is_ok_and(|only| !id.starts_with(&only)) {
                continue;
            }
            let (ext, settings) = editor::preset(id, true, Some("s32")).unwrap();
            let out = tmp.join(format!("{}.{ext}", id.replace(':', "_")));
            let _ = std::fs::remove_file(&out);
            let ok = editor::encode_any(&m, &src.to_string_lossy(), &out, &settings, |_| {});
            let probe = std::process::Command::new("ffprobe")
                .args(["-v", "error", "-show_entries", "stream=codec_type,codec_name,pix_fmt", "-of", "csv=p=0"])
                .arg(&out).output().unwrap();
            let info = String::from_utf8_lossy(&probe.stdout).replace('\n', " ");
            println!("{id:14} {ok:5} {info}");
            let audio_only = ["mp3", "aac", "wav", "flac"].contains(&id.split(':').next().unwrap());
            if !ok || info.trim().is_empty() || (!audio_only && !info.contains("video")) {
                failed.push(*id);
            }
        }
        assert!(failed.is_empty(), "failed: {failed:?}");
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
        assert_eq!(tag_from_release_url("https://github.com/o/r/releases/tag/v0.1.10\n").as_deref(), Some("v0.1.10"));
        assert_eq!(tag_from_release_url("https://github.com/o/r/releases"), None);
        assert_eq!(tag_from_release_url(""), None);
    }
}
