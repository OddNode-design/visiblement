//! Mini editor: export the A-B selection (or the whole file) with a second libmpv instance
//! running in encoding mode, so no external FFmpeg is needed.

use crate::{EVENT_END_FILE, EVENT_SHUTDOWN, Mpv};
use std::ffi::{CStr, c_char};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

/// Uncompressed audio at the source's sample depth.
fn pcm(audio_format: Option<&str>) -> &'static str {
    match audio_format.unwrap_or("") {
        f if f.starts_with("float") || f.starts_with("double") => "pcm_f32le",
        f if f.starts_with("s32") || f.starts_with("s24") => "pcm_s24le",
        _ => "pcm_s16le",
    }
}

/// Encoder settings close to the source: same audio sample format, same video family.
pub fn codecs(ext: &str, video_codec: Option<&str>, audio_format: Option<&str>, alpha: bool) -> Vec<(&'static str, String)> {
    let pcm = pcm(audio_format);
    match ext {
        "wav" | "wave" => vec![("oac", pcm.into()), ("vid", "no".into())],
        "flac" => vec![("oac", "flac".into()), ("vid", "no".into())],
        "mp3" => vec![("oac", "libmp3lame".into()), ("oacopts", "b=320k".into()), ("vid", "no".into())],
        _ => {
            let video = match video_codec.unwrap_or("") {
                c if c.contains("prores") && alpha => {
                    vec![("ovc", "prores_ks".into()), ("ovcopts", "profile=4444,pix_fmt=yuva444p10le".into())]
                }
                c if c.contains("prores") => vec![("ovc", "prores_ks".into()), ("ovcopts", "profile=3".into())],
                c if c.contains("hevc") => vec![("ovc", "libx265".into()), ("ovcopts", "crf=18,preset=medium".into())],
                c if c.contains("vp9") && alpha => {
                    vec![("ovc", "libvpx-vp9".into()), ("ovcopts", "crf=20,b=0,pix_fmt=yuva420p".into())]
                }
                c if c.contains("vp9") || c.contains("vp8") => vec![("ovc", "libvpx-vp9".into()), ("ovcopts", "crf=20,b=0".into())],
                _ => vec![("ovc", "libx264".into()), ("ovcopts", "crf=16,preset=medium".into())],
            };
            let audio = if ext == "mov" && video_codec.is_some_and(|c| c.contains("prores")) {
                vec![("oac", pcm.into())]
            } else if ext == "webm" {
                vec![("oac", "libopus".into()), ("oacopts", "b=192k".into())]
            } else {
                vec![("oac", "aac".into()), ("oacopts", "b=256k".into())]
            };
            [video, audio].concat()
        }
    }
}

/// Every "Convertir" format as "codec:type" (ui/app.js lists the same ids).
#[cfg(test)] // the page holds the same list; the tests walk this one
pub const PRESETS: &[&str] = &[
    "h264:hq", "h264:std", "h264:small", "h265:hq", "h265:std", "h265:small", "av1:hq", "av1:std", "av1:small",
    "prores:proxy", "prores:lt", "prores:422", "prores:hq", "prores:4444", "prores:xq",
    "dnxhr:lb", "dnxhr:sq", "dnxhr:hq", "dnxhr:hqx", "dnxhr:444",
    "qtrle:rgb", "gif:full", "gif:small",
    "mp3:320", "mp3:192", "aac:256", "aac:128", "wav:16", "wav:24", "wav:32f", "flac:src",
];

/// "Convertir": output extension and encoder settings for a "codec:type" id. `alpha` keeps the
/// transparency where the type can carry it (ProRes 4444 / XQ, QuickTime Animation).
/// The pixel format goes through a filter: mpv ignores `pix_fmt` in ovcopts.
pub fn preset(id: &str, alpha: bool, audio_format: Option<&str>) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    let (codec, kind) = id.split_once(':')?;
    let pcm = pcm(audio_format);
    let mut v: Vec<(&'static str, String)> = Vec::new();
    let mut set = |k: &'static str, val: &str| v.push((k, val.to_string()));
    // Odd sizes (a 1780x2287 PNG sequence) make 4:2:0 / 4:2:2 encoders refuse to open: one empty
    // row or column brings them to even. A no-op on even sizes.
    let fmt = |pix: &str| format!("lavfi=[pad=ceil(iw/2)*2:ceil(ih/2)*2:color=black@0,format={pix}]");
    let ext = match codec {
        "h264" | "h265" => {
            let crf = match (codec, kind) {
                ("h264", "hq") => 16, ("h264", "std") => 20, ("h264", "small") => 26,
                ("h265", "hq") => 18, ("h265", "std") => 22, ("h265", "small") => 28,
                _ => return None,
            };
            if codec == "h264" {
                set("ovc", "libx264");
                set("ovcopts", &format!("crf={crf},preset=medium"));
            } else {
                // No libx265 in libmpv: the graphics card's encoder (NVIDIA, AMD or Intel); see `encode_any`.
                set("ovc", HEVC_ENCODERS[0]);
                set("ovcopts", &format!("cq={crf},qp_i={crf},qp_p={crf},quality={crf}"));
            }
            set("vf", &fmt("yuv420p"));
            set("oac", "aac");
            set("oacopts", "b=256k");
            "mp4"
        }
        "prores" => {
            let profile = ["proxy", "lt", "422", "hq", "4444", "xq"].iter().position(|k| *k == kind)?;
            let pix = match (profile >= 4, alpha) { (false, _) => "yuv422p10le", (true, true) => "yuva444p10le", (true, false) => "yuv444p10le" };
            set("ovc", "prores_ks");
            set("ovcopts", &format!("profile={profile}"));
            set("vf", &fmt(pix));
            set("oac", pcm);
            "mov"
        }
        "dnxhr" => {
            let pix = match kind { "lb" | "sq" | "hq" => "yuv422p", "hqx" => "yuv422p10le", "444" => "yuv444p10le", _ => return None };
            set("ovc", "dnxhd");
            set("ovcopts", &format!("profile=dnxhr_{kind}"));
            set("vf", &fmt(pix));
            set("oac", pcm);
            "mov"
        }
        "qtrle" => {
            set("ovc", "qtrle");
            set("vf", &fmt(if alpha { "argb" } else { "rgb24" }));
            set("oac", pcm);
            "mov"
        }
        "av1" => {
            let crf = match kind { "hq" => 24, "std" => 32, "small" => 40, _ => return None };
            set("ovc", "libsvtav1");
            set("ovcopts", &format!("crf={crf},preset=8"));
            set("vf", &fmt("yuv420p10le"));
            set("oac", "aac");
            set("oacopts", "b=256k");
            "mp4"
        }
        "gif" => {
            // One palette per frame: streams (no second pass) and keeps the colours right.
            let pre = match kind { "full" => "", "small" => "fps=15,scale=640:-2,", _ => return None };
            set("ovc", "gif");
            set("vf", &format!("lavfi=[{pre}split[a][b];[a]palettegen=stats_mode=single[p];[b][p]paletteuse=new=1]"));
            set("aid", "no");
            "gif"
        }
        "mp3" | "aac" => {
            let rate = match (codec, kind) { ("mp3", "320" | "192") | ("aac", "256" | "128") => kind, _ => return None };
            set("oac", if codec == "mp3" { "libmp3lame" } else { "aac" });
            set("oacopts", &format!("b={rate}k"));
            set("vid", "no");
            if codec == "mp3" { "mp3" } else { "m4a" }
        }
        "wav" => {
            set("oac", match kind { "16" => "pcm_s16le", "24" => "pcm_s24le", "32f" => "pcm_f32le", _ => return None });
            set("vid", "no");
            "wav"
        }
        "flac" => {
            set("oac", "flac");
            set("vid", "no");
            "flac"
        }
        _ => return None,
    };
    Some((ext, v))
}

const EVENT_LOG_MESSAGE: i32 = 2;
/// The last encode's error lines from mpv / FFmpeg (empty when it went fine).
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

/// H.265 encoders, tried in this order until one works on this PC.
pub const HEVC_ENCODERS: &[&str] = &["hevc_nvenc", "hevc_amf", "hevc_qsv"];

/// `encode`, retrying H.265 with the next graphics card encoder when one is missing. An encoder
/// that fails to open leaves an audio-only file: a MOV / MP4 without a video track counts as failed.
pub fn encode_any(m: &Mpv, src: &str, out: &Path, settings: &[(&str, String)], progress: impl Fn(f64)) -> bool {
    let video = settings.iter().any(|(k, _)| *k == "ovc");
    let ovc = settings.iter().find(|(k, _)| *k == "ovc").map(|(_, v)| v.as_str());
    let encoders: Vec<&str> = if ovc == Some(HEVC_ENCODERS[0]) { HEVC_ENCODERS.to_vec() } else { vec![ovc.unwrap_or("")] };
    encoders.iter().any(|enc| {
        let _ = std::fs::remove_file(out);
        let s: Vec<_> = settings.iter().map(|(k, v)| (*k, if *k == "ovc" { enc.to_string() } else { v.clone() })).collect();
        !CANCEL.load(Ordering::Relaxed) && encode(m, src, out, None, None, &s, &progress) && (!video || has_video(out))
    })
}

/// MOV / MP4: a track with a video handler ("vide") in the moov; other containers pass.
fn has_video(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext != "mov" && ext != "mp4" {
        return true;
    }
    crate::read_moov(&path.to_string_lossy()).is_some_and(|moov| moov.windows(4).any(|w| w == b"vide"))
}

fn clock(t: f64) -> String {
    let ms = (t * 1000.0).round() as i64;
    format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

/// Encodes `src` between `a` and `b` into `out` with a separate libmpv instance in encoding mode.
/// Blocks until done; true when the output file was written.
pub fn encode(m: &Mpv, src: &str, out: &Path, a: Option<f64>, b: Option<f64>, settings: &[(&str, String)], progress: impl Fn(f64)) -> bool {
    // SAFETY: a fresh, independent handle; destroyed below on every path.
    LAST_ERROR.lock().unwrap().clear();
    let h = unsafe { (m.create)() };
    if h.is_null() {
        return false;
    }
    let enc = Mpv { h, ..*m };
    enc.option("config", "no");
    enc.option("terminal", "no");
    enc.option("o", &out.to_string_lossy());
    enc.option("start", &a.map_or("0".into(), |v| v.to_string()));
    if let Some(b) = b {
        enc.option("end", &b.to_string());
    }
    enc.option("hr-seek", "yes");
    // Errors from mpv / FFmpeg, kept to tell the user why a conversion failed.
    unsafe { (m.request_log)(h, c"error".as_ptr()) };
    let mut errors: Vec<String> = Vec::new();
    if let Some(log) = std::env::var_os("VISIBLEMENT_LOG") {
        enc.option("log-file", &format!("{}.export.txt", log.to_string_lossy()));
    }
    // MOV / MP4 video: no edit list. mpv cannot tell the muxer how long the last frame lasts, so the
    // edit list it wrote ended one frame early and players dropped the last frame (24 of 25).
    let ext = out.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if matches!(ext.as_str(), "mov" | "mp4" | "m4v") && settings.iter().any(|(k, _)| *k == "ovc") {
        enc.option("ofopts", "use_editlist=0");
    }
    // Every encoder on all cores: through the library, FFmpeg leaves them on one thread unless asked
    // (ProRes from a PNG sequence went from 36 s to 3 s).
    let mut settings = settings.to_vec();
    let video = settings.iter().any(|(k, _)| *k == "ovc");
    match settings.iter_mut().find(|(k, _)| *k == "ovcopts") {
        Some((_, v)) => v.push_str(",threads=0"),
        None if video => settings.push(("ovcopts", "threads=0".into())),
        None => {}
    }
    for (k, v) in &settings {
        enc.option(k, v);
    }
    if unsafe { (m.initialize)(h) } < 0 {
        unsafe { (m.destroy)(h) };
        return false;
    }
    enc.cmd(&["loadfile", src]);
    loop {
        // SAFETY: valid handle; the event lives until the next call.
        let ev = unsafe { &*(m.wait_event)(h, 0.25) };
        match ev.id {
            EVENT_SHUTDOWN | EVENT_END_FILE => break,
            _ if CANCEL.load(Ordering::Relaxed) => break,
            EVENT_LOG_MESSAGE => {
                // SAFETY: data points to an mpv_event_log_message { prefix, level, text, log_level }.
                let msg = unsafe { &*(ev.data as *const [*const c_char; 3]) };
                let text = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_string_lossy().trim().to_string();
                let line = format!("{}: {}", text(msg[0]), text(msg[2]));
                if !errors.contains(&line) {
                    errors.push(line);
                }
            }
            _ => progress(enc.num("percent-pos")),
        }
    }
    // Destroying flushes and closes the output file.
    unsafe { (m.destroy)(h) };
    *LAST_ERROR.lock().unwrap() = errors.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join("\n");
    if CANCEL.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(out); // a stopped encode leaves no half file behind
        return false;
    }
    out.metadata().is_ok_and(|md| md.len() > 0)
}

/// Set by the page's stop button: the running export / conversion ends and its file is removed.
pub static CANCEL: AtomicBool = AtomicBool::new(false);

/// The progress pill of the page (label, percentage), then its end.
fn job(label: &str, pct: f64) {
    crate::app::eval_js(&format!("window.vb && vb.job({}, {})", serde_json::json!(label), pct.round()));
}

/// The result of an export / conversion: the pill goes, a toast says how it ended.
fn job_done(m: &Mpv, ok: bool, done: String, failed: &str, cancelled: &str) {
    crate::app::eval_js("window.vb && vb.jobDone()");
    let why = LAST_ERROR.lock().unwrap().clone();
    let failed = if why.is_empty() { failed.to_string() } else { format!("{failed}\n{why}") };
    m.toast(if CANCEL.swap(false, Ordering::Relaxed) { cancelled } else if ok { &done } else { &failed });
}

/// Asks where to save, then encodes in the background with a progress toast in the player.
pub fn export(m: Mpv) -> thread::JoinHandle<()> {
    let src = m.get("path").unwrap_or_default();
    let a = m.get("ab-loop-a").and_then(|v| v.parse::<f64>().ok());
    let b = m.get("ab-loop-b").and_then(|v| v.parse::<f64>().ok());
    let video_codec = m.get("current-tracks/video/codec");
    let audio_format = m.get("audio-params/format");
    let alpha = crate::has_alpha(&m);
    thread::spawn(move || {
        if src.is_empty() {
            m.toast("Aucun fichier ouvert");
            return;
        }
        let src_path = Path::new(&src);
        let ext = src_path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_else(|| "mp4".into());
        let stem = src_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(out) = rfd::FileDialog::new()
            .set_title(crate::tr("Enregistrer l'extrait", "Save the clip"))
            .set_file_name(format!("{stem}_extrait.{ext}"))
            .add_filter(ext.to_uppercase(), &[&ext])
            .save_file()
        else {
            return;
        };
        if out == src_path {
            m.toast("Choisissez un autre nom que le fichier d'origine");
            return;
        }
        let out_ext = out.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or(ext);

        let span = match (a, b) {
            (Some(a), Some(b)) => format!("{} → {}", clock(a), clock(b)),
            _ => "fichier entier".into(),
        };
        let settings = codecs(&out_ext, video_codec.as_deref(), audio_format.as_deref(), alpha);
        crate::app::EXPORTING.store(true, Ordering::Relaxed);
        CANCEL.store(false, Ordering::Relaxed);
        let label = format!("Export ({span})");
        let ok = encode(&m, &src, &out, a, b, &settings, |pct| job(&label, pct));
        crate::app::EXPORTING.store(false, Ordering::Relaxed);
        let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        job_done(&m, ok, format!("Extrait enregistré : {name}"), "Échec de l'export", "Export annulé");
    })
}

/// An image sequence on top of a format's settings: its frame rate, and RGB images into YUV with
/// the HD matrix (Rec.709), said so in the file as editing software reads it (FFmpeg's default
/// is the SD one: colours shifted slightly).
pub fn sequence_settings(id: &str, fps: &str, settings: &mut Vec<(&'static str, String)>) {
    // NTSC rates as exact as a float gets (24000/1001, 30000/1001): no drift over long sequences.
    let fps = match fps {
        "23.976" => "23.976023976023978",
        "29.97" => "29.97002997002997",
        other => other,
    };
    settings.push(("mf-fps", fps.to_string()));
    if ["prores", "dnxhr", "h264", "h265", "av1"].iter().any(|c| id.starts_with(c)) {
        for (k, v) in settings.iter_mut() {
            match *k {
                "vf" => *v = v.replacen("lavfi=[", "lavfi=[scale=out_color_matrix=bt709:out_range=tv,", 1),
                "ovcopts" => v.push_str(",colorspace=bt709,color_primaries=bt709,color_trc=bt709"),
                _ => {}
            }
        }
    }
}

/// "Convertir": the whole open file, or an image sequence (`seq`: its mf:// address and frame
/// rate), into another format (see `preset`), in the background.
pub fn convert(m: Mpv, id: String, alpha: bool, seq: Option<(String, String)>) -> thread::JoinHandle<()> {
    let open = m.get("path").unwrap_or_default();
    let audio_format = m.get("audio-params/format");
    thread::spawn(move || {
        let Some((ext, mut settings)) = preset(&id, alpha, audio_format.as_deref()) else { return };
        let (src, src_path) = if let Some((url, fps)) = seq {
            sequence_settings(&id, &fps, &mut settings);
            let path = std::path::PathBuf::from(url.trim_start_matches("mf://"));
            (url, path)
        } else {
            if open.is_empty() || open.contains("://") {
                m.toast("Ouvrez d'abord un fichier à convertir");
                return;
            }
            (open.clone(), std::path::PathBuf::from(&open))
        };
        let mut stem = src_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if src.starts_with("mf://") {
            stem = stem.trim_end_matches(|c: char| c.is_ascii_digit() || matches!(c, '*' | '_' | '.' | '-')).to_string(); // shot_* → shot
        }
        let Some(out) = rfd::FileDialog::new()
            .set_title(crate::tr("Convertir", "Convert"))
            .set_file_name(format!("{stem}_{}.{ext}", id.replace(':', "_")))
            .add_filter(ext.to_uppercase(), &[ext])
            .save_file()
        else {
            return;
        };
        if out == src_path {
            m.toast("Choisissez un autre nom que le fichier d'origine");
            return;
        }
        crate::app::EXPORTING.store(true, Ordering::Relaxed);
        CANCEL.store(false, Ordering::Relaxed);
        let ok = encode_any(&m, &src, &out, &settings, |pct| job("Conversion", pct));
        crate::app::EXPORTING.store(false, Ordering::Relaxed);
        let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        job_done(&m, ok, format!("Fichier converti : {name}"), "Échec de la conversion", "Conversion annulée");
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_keeps_its_sample_format() {
        assert!(codecs("wav", None, Some("s24"), false).contains(&("oac", "pcm_s24le".into())));
        assert!(codecs("wav", None, Some("floatp"), false).contains(&("oac", "pcm_f32le".into())));
        assert!(codecs("wav", None, Some("s16"), false).contains(&("oac", "pcm_s16le".into())));
    }

    #[test]
    fn prores_alpha_stays_prores_4444() {
        let c = codecs("mov", Some("prores"), Some("s16"), true);
        assert!(c.contains(&("ovc", "prores_ks".into())));
        assert!(c.iter().any(|(k, v)| *k == "ovcopts" && v.contains("yuva444p10le")));
        assert!(c.contains(&("oac", "pcm_s16le".into())));
    }

    #[test]
    fn h264_mp4_reencodes_with_x264_and_aac() {
        let c = codecs("mp4", Some("h264"), Some("floatp"), false);
        assert!(c.contains(&("ovc", "libx264".into())) && c.contains(&("oac", "aac".into())));
    }

    #[test]
    fn convert_presets() {
        for id in PRESETS {
            assert!(preset(id, true, None).is_some(), "{id}");
        }
        let (ext, c) = preset("prores:4444", true, Some("s24")).unwrap();
        assert_eq!(ext, "mov");
        assert!(c.contains(&("ovcopts", "profile=4".into())) && c.contains(&("vf", "lavfi=[pad=ceil(iw/2)*2:ceil(ih/2)*2:color=black@0,format=yuva444p10le]".into())));
        assert!(c.contains(&("oac", "pcm_s24le".into())));
        assert!(preset("prores:hq", true, None).unwrap().1.contains(&("vf", "lavfi=[pad=ceil(iw/2)*2:ceil(ih/2)*2:color=black@0,format=yuv422p10le]".into())));
        assert!(preset("prores:xq", false, None).unwrap().1.contains(&("vf", "lavfi=[pad=ceil(iw/2)*2:ceil(ih/2)*2:color=black@0,format=yuv444p10le]".into())));
        assert!(preset("dnxhr:hqx", false, None).unwrap().1.contains(&("ovcopts", "profile=dnxhr_hqx".into())));
        assert_eq!(preset("wav:32f", false, None).unwrap().1, vec![("oac", "pcm_f32le".to_string()), ("vid", "no".to_string())]);
        assert!(preset("prores:nope", false, None).is_none());
        assert!(preset("nope", false, None).is_none());
    }

    #[test]
    fn clock_format() {
        assert_eq!(clock(83.5), "1:23.500");
    }

    /// Odd-sized frames (1780x2287) used to fail in H.264; a broken encoder now says why.
    /// Needs target/release/libmpv-2.dll: cargo test --release -- --ignored odd_size
    #[test]
    #[ignore]
    fn odd_size_sequence_encodes_and_failures_say_why() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release");
        let (_lib, m) = crate::load_mpv(&dir).expect("libmpv-2.dll in target/release");
        let tmp = std::env::temp_dir().join("visiblement-odd-test");
        let _ = std::fs::create_dir_all(&tmp);
        let png = tmp.join("odd_0001.png");
        let enc = png::Encoder::new(std::fs::File::create(&png).unwrap(), 31, 17); // odd both ways
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[128u8; 31 * 17]).unwrap();
        drop(w);
        let src = format!("mf://{}", tmp.join("odd_*.png").to_string_lossy());
        let out = tmp.join("odd.mp4");
        let (_, mut settings) = preset("h264:std", false, None).unwrap();
        sequence_settings("h264:std", "25", &mut settings);
        assert!(encode_any(&m, &src, &out, &settings, |_| {}), "{}", LAST_ERROR.lock().unwrap());
        let bad: Vec<(&str, String)> = vec![("ovc", "no_such_encoder".into())];
        assert!(!encode(&m, &src, &tmp.join("bad.mp4"), None, None, &bad, |_| {}));
        let why = LAST_ERROR.lock().unwrap().clone();
        assert!(!why.is_empty(), "no reason captured");
        println!("reason: {why}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
