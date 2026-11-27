//! Light playback copies of heavy videos (a 16-megapixel ProRes 4444 decodes at ~10 frames/s here).
//!
//! When a local file is too heavy to decode in real time, a lighter copy is made once in the
//! background (HEVC, at most 2560 px and about 4 megapixels, high quality) and kept in a cache folder.
//! From then on mpv's `on_load` hook opens the copy instead (`stream-open-filename`), while its `path`
//! property stays the original: converting, exporting, "show in folder", the folder's next file and
//! the recent list all keep working on the original without knowing. Saving or copying a frame goes
//! back to the original too (see `grab_original`), at full resolution.

use crate::Mpv;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::SystemTime;

/// Codecs whose every frame is a full picture (no motion compensation): the heavy ones to decode.
const INTRA: [&str; 8] = ["prores", "dnxhd", "cfhd", "v210", "r210", "qtrle", "rawvideo", "png"];
/// Above this many decoded pixels per second (an intra codec), or this bit rate, playback stutters.
const HEAVY_PIXELS: f64 = 250e6;
const HEAVY_BITS: f64 = 1.5e9;
/// The cache folder's size limit: the least recently used copies go first.
const BUDGET: u64 = 20 << 30;

static BUILDING: AtomicBool = AtomicBool::new(false);
/// A light copy just got ready for the file being played: where to resume once it reopens on it.
pub static RESUME: Mutex<Option<(f64, bool)>> = Mutex::new(None);

fn dir() -> Option<PathBuf> {
    crate::data_dir(true).map(|d| d.join("proxies"))
}

/// Where the light copy of `path` lives, made or not. Keyed on path, size and date: a re-rendered
/// file gets a new copy.
fn copy_for(path: &str) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let mut h = DefaultHasher::new();
    (path, meta.len(), meta.modified().ok()).hash(&mut h);
    let stem: String = Path::new(path).file_stem()?.to_string_lossy().chars().take(40).collect();
    Some(dir()?.join(format!("{stem}-{:016x}.mp4", h.finish())))
}

/// `on_load` hook: if the file has a light copy, mpv opens that instead (its `path` stays the original).
pub fn redirect(m: &Mpv) {
    let Some(path) = m.get("path").filter(|p| !p.contains("://")) else { return };
    let Some(copy) = copy_for(&path).filter(|c| c.is_file()) else { return };
    // Its date says when it was last used: the cache trims the oldest first.
    if let Ok(f) = std::fs::File::options().write(true).open(&copy) {
        let _ = f.set_modified(SystemTime::now());
    }
    m.set("stream-open-filename", &copy.to_string_lossy());
}

/// The original's description saved with its light copy, when the player shows that copy.
pub fn original_info(m: &Mpv) -> Option<serde_json::Value> {
    if !active(m) {
        return None;
    }
    let copy = PathBuf::from(m.get("stream-open-filename")?);
    serde_json::from_str(&std::fs::read_to_string(copy.with_extension("json")).ok()?).ok()
}

/// True while the player shows a light copy rather than the file itself.
pub fn active(m: &Mpv) -> bool {
    match (m.get("stream-open-filename"), m.get("path")) {
        (Some(open), Some(path)) => open != path,
        _ => false,
    }
}

/// Once a file is loaded: too heavy for real time? Then its light copy gets made in the background.
pub fn check(m: Mpv) {
    if active(&m) || BUILDING.load(Ordering::Relaxed) {
        return;
    }
    let Some(path) = m.get("path").filter(|p| !p.contains("://") && Path::new(p).is_file()) else { return };
    let codec = m.get("current-tracks/video/codec").unwrap_or_default();
    let (w, h) = (m.num("video-params/w"), m.num("video-params/h"));
    let fps = m.num("container-fps");
    let seconds = m.num("duration");
    let bits = std::fs::metadata(&path).map_or(0.0, |md| md.len() as f64 * 8.0) / seconds.max(0.001);
    let heavy = (INTRA.contains(&codec.as_str()) && w * h * fps > HEAVY_PIXELS) || bits > HEAVY_BITS;
    if heavy && w > 0.0 && h > 0.0 && m.get("current-tracks/video/image").as_deref() != Some("yes") {
        build(m, path, w, h);
    }
}

fn build(m: Mpv, path: String, w: f64, h: f64) {
    let Some(copy) = copy_for(&path) else { return };
    if BUILDING.swap(true, Ordering::Relaxed) {
        return;
    }
    let audio_format = m.get("audio-params/format");
    // What the original is, for the Infos panel while the copy plays (mpv then only sees the copy).
    let about = serde_json::json!({
        "w": w, "h": h,
        "codec": m.get("video-codec").or_else(|| m.get("current-tracks/video/codec")),
        "pixfmt": m.get("video-params/pixelformat"),
    });
    thread::spawn(move || {
        let _ = std::fs::create_dir_all(copy.parent().unwrap_or(Path::new(".")));
        // At most 2560 px on the long side and about 4 megapixels: sharp on any screen, light to decode.
        let k = (2560.0 / w.max(h)).min((4.2e6 / (w * h)).sqrt()).min(1.0);
        let (tw, th) = (((w * k / 2.0).round() * 2.0) as i64, ((h * k / 2.0).round() * 2.0) as i64);
        let scale = |pix: &str| format!("lavfi=[scale={tw}:{th}:flags=lanczos,format={pix}]");
        let part = copy.with_extension("part.mp4");
        let progress = |pct: f64| crate::editor::job("Préparation de la lecture fluide", pct);
        let audio = |s: &mut Vec<(&'static str, String)>| s.extend([("oac", "aac".to_string()), ("oacopts", "b=256k".to_string())]);
        // 1. NVIDIA, 10-bit HEVC at a constant quality: measured on a 16-megapixel ProRes 4444, qp 18 keeps
        //    an SSIM of about 0.99 against the original scaled down (the encoder's default rate gave 0.93).
        let mut nvenc = vec![("ovc", "hevc_nvenc".to_string()), ("ovcopts", "rc=constqp,qp=18,preset=p6".to_string()), ("vf", scale("p010le"))];
        audio(&mut nvenc);
        let mut ok = crate::editor::encode(&m, &path, &part, None, None, &nvenc, progress);
        // 2. AMD / Intel encoders (the Convert H.265 settings), then 3. x264 on the processor.
        for id in ["h265:hq", "h264:hq"] {
            if ok || crate::editor::CANCEL.load(Ordering::Relaxed) {
                break;
            }
            let Some((_, mut settings)) = crate::editor::preset(id, false, audio_format.as_deref()) else { continue };
            for (k, v) in settings.iter_mut() {
                if *k == "vf" {
                    *v = scale("yuv420p");
                }
            }
            if id == "h264:hq" {
                settings.push(("ovcopts", "crf=16,preset=veryfast".into()));
            }
            ok = crate::editor::encode_any(&m, &path, &part, &settings, progress);
        }
        ok = ok && std::fs::rename(&part, &copy).is_ok();
        if ok {
            let _ = std::fs::write(copy.with_extension("json"), about.to_string());
        }
        if !ok {
            let _ = std::fs::remove_file(&part);
        }
        crate::editor::job_done(&m, ok, "Lecture fluide prête".into(), "Lecture fluide impossible", "Lecture fluide annulée");
        BUILDING.store(false, Ordering::Relaxed);
        if ok {
            trim(&copy);
            // Still on that file: reopen it, now on the copy, where it was.
            if m.get("path").as_deref() == Some(path.as_str()) {
                let paused = m.get("pause").as_deref() == Some("yes");
                *RESUME.lock().unwrap_or_else(|e| e.into_inner()) = Some((m.num("time-pos"), paused));
                m.cmd(&["playlist-play-index", "current"]);
            }
        }
    });
}

/// Keeps the cache under BUDGET, least recently used first; never the copy just made.
fn trim(keep: &Path) {
    let Some(dir) = dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            md.is_file().then(|| (md.modified().unwrap_or(SystemTime::UNIX_EPOCH), md.len(), e.path()))
        })
        .collect();
    files.sort();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    for (_, len, path) in files {
        if total <= BUDGET {
            break;
        }
        // Half-made copies of an interrupted run go too.
        if path != keep {
            let _ = std::fs::remove_file(&path);
            total = total.saturating_sub(len);
        }
    }
}

/// The frame on screen, taken from the original file at full resolution (the player may show the
/// light copy): one frame encoded to PNG by a separate libmpv instance.
pub fn grab_original(m: &Mpv, out: &Path) -> bool {
    let Some(src) = m.get("path") else { return false };
    let t = m.num("time-pos");
    let settings = [("ovc", "png".to_string()), ("frames", "1".to_string()), ("aid", "no".to_string()), ("sid", "no".to_string())];
    crate::editor::encode(m, &src, out, Some(t), None, &settings, |_| {}) && out.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_name_follows_the_file() {
        let dir = std::env::temp_dir().join(format!("vb-proxy-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("shot.mov");
        std::fs::write(&f, b"a").unwrap();
        let a = copy_for(&f.to_string_lossy()).unwrap();
        assert!(a.file_name().unwrap().to_string_lossy().starts_with("shot-"));
        // A re-rendered file (other size) gets another copy.
        std::fs::write(&f, b"ab").unwrap();
        assert_ne!(copy_for(&f.to_string_lossy()).unwrap(), a);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real thing on a heavy file: `cargo test --release -- --ignored --nocapture light_copy`
    /// with VB_HEAVY=<path to a heavy video> (and target/release/libmpv-2.dll).
    #[test]
    #[ignore]
    fn light_copy_of_a_heavy_file() {
        let Some(file) = std::env::var_os("VB_HEAVY") else { return };
        let file = file.to_string_lossy().into_owned();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release");
        let (_lib, api) = crate::load_mpv(&dir).expect("libmpv");
        let h = unsafe { (api.create)() };
        let m = Mpv { h, ..api };
        for (k, v) in [("config", "no"), ("vo", "null"), ("ao", "null"), ("pause", "yes")] {
            m.option(k, v);
        }
        assert!(unsafe { (m.initialize)(h) } >= 0);
        m.cmd(&["loadfile", &file]);
        let t0 = std::time::Instant::now();
        while m.get("video-params/w").is_none() && t0.elapsed().as_secs() < 20 {
            thread::sleep(std::time::Duration::from_millis(100));
        }
        let copy = copy_for(&file).unwrap();
        let _ = std::fs::remove_file(&copy);
        check(m);
        assert!(BUILDING.load(Ordering::Relaxed), "not detected as heavy");
        let t0 = std::time::Instant::now();
        while BUILDING.load(Ordering::Relaxed) && t0.elapsed().as_secs() < 900 {
            thread::sleep(std::time::Duration::from_millis(500));
        }
        let md = std::fs::metadata(&copy).expect("light copy made");
        println!("light copy: {} ({} MB) in {:.0} s", copy.display(), md.len() >> 20, t0.elapsed().as_secs_f64());
        unsafe { (m.destroy)(h) };
    }
}
