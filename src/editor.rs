//! Mini editor: export the A-B selection (or the whole file) with a second libmpv instance
//! running in encoding mode, so no external FFmpeg is needed.

use crate::{EVENT_END_FILE, EVENT_SHUTDOWN, Mpv};
use std::path::Path;
use std::thread;

/// Encoder settings close to the source: same audio sample format, same video family.
pub fn codecs(ext: &str, video_codec: Option<&str>, audio_format: Option<&str>, alpha: bool) -> Vec<(&'static str, String)> {
    let pcm = match audio_format.unwrap_or("") {
        f if f.starts_with("float") || f.starts_with("double") => "pcm_f32le",
        f if f.starts_with("s32") || f.starts_with("s24") => "pcm_s24le",
        _ => "pcm_s16le",
    };
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

fn clock(t: f64) -> String {
    let ms = (t * 1000.0).round() as i64;
    format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

/// Encodes `src` between `a` and `b` into `out` with a separate libmpv instance in encoding mode.
/// Blocks until done; true when the output file was written.
pub fn encode(m: &Mpv, src: &str, out: &Path, a: Option<f64>, b: Option<f64>, settings: &[(&str, String)], progress: impl Fn(f64)) -> bool {
    // SAFETY: a fresh, independent handle; destroyed below on every path.
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
    if let Some(log) = std::env::var_os("VISIBLEMENT_LOG") {
        enc.option("log-file", &format!("{}.export.txt", log.to_string_lossy()));
    }
    for (k, v) in settings {
        enc.option(k, v);
    }
    if unsafe { (m.initialize)(h) } < 0 {
        unsafe { (m.destroy)(h) };
        return false;
    }
    enc.cmd(&["loadfile", src]);
    loop {
        // SAFETY: valid handle; the event lives until the next call.
        let ev = unsafe { &*(m.wait_event)(h, 0.5) };
        match ev.id {
            EVENT_SHUTDOWN | EVENT_END_FILE => break,
            _ => progress(enc.num("percent-pos")),
        }
    }
    // Destroying flushes and closes the output file.
    unsafe { (m.destroy)(h) };
    out.metadata().is_ok_and(|md| md.len() > 0)
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
            .set_title("Enregistrer l'extrait")
            .set_file_name(format!("{stem}_extrait.{ext}"))
            .add_filter(ext.to_uppercase(), &[&ext])
            .save_file()
        else {
            return;
        };
        if out == src_path {
            m.toast("Choisis un autre nom que le fichier d'origine");
            return;
        }
        let out_ext = out.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or(ext);

        let span = match (a, b) {
            (Some(a), Some(b)) => format!("{} → {}", clock(a), clock(b)),
            _ => "fichier entier".into(),
        };
        let settings = codecs(&out_ext, video_codec.as_deref(), audio_format.as_deref(), alpha);
        let ok = encode(&m, &src, &out, a, b, &settings, |pct| {
            m.cmd(&["show-text", &format!("Export ({span})… {pct:.0} %"), "1000"]);
        });
        let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        m.toast(&if ok { format!("Extrait enregistré : {name}") } else { "Échec de l'export".into() });
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
    fn clock_format() {
        assert_eq!(clock(83.5), "1:23.500");
    }
}
