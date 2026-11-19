//! Network streams: VLC-style addresses made playable by mpv, and yt-dlp fetched on demand for
//! web video pages (YouTube, Vimeo…), which mpv plays through its ytdl hook.

use crate::{app, Mpv};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::thread;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

/// Turns what the user typed into an address mpv understands, or None if it isn't one.
/// - `www.site.com/…` gets https://
/// - VLC's "listen on all interfaces" form `rtp://@:1234` / `udp://@:1234` becomes `…://0.0.0.0:1234`
/// - `mms://` is served by FFmpeg's `mmsh://`
pub fn normalize_url(input: &str) -> Option<String> {
    let s = input.trim();
    let s = if s.to_ascii_lowercase().starts_with("www.") { format!("https://{s}") } else { s.to_string() };
    let (scheme, rest) = s.split_once("://")?;
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-') || rest.is_empty() {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    let rest = match (scheme.as_str(), rest.strip_prefix('@')) {
        ("rtp" | "udp", Some(r)) => format!("0.0.0.0{r}"),
        _ => rest.to_string(),
    };
    let scheme = if scheme == "mms" { "mmsh".to_string() } else { scheme };
    Some(format!("{scheme}://{rest}"))
}

/// A web page rather than a media file: mpv needs yt-dlp to find the video inside.
pub fn needs_ytdl(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return false;
    }
    let path = lower.split(['?', '#']).next().unwrap_or("");
    let file = path.rsplit('/').next().unwrap_or("");
    let media = [
        ".mp4", ".mov", ".mkv", ".webm", ".avi", ".m4v", ".ts", ".m3u8", ".mpd", ".flv", ".asx", ".sdp",
        ".mp3", ".wav", ".flac", ".aac", ".m4a", ".ogg", ".opus", ".gif", ".webp", ".png", ".jpg",
    ];
    !media.iter().any(|ext| file.ends_with(ext))
}

/// Where the app keeps yt-dlp (it is downloaded the first time it is needed).
pub fn ytdlp_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Visiblement").join("yt-dlp.exe"))
}

/// Opens a network address; for web pages, installs yt-dlp first if it is missing.
pub fn open(m: Mpv, input: &str) {
    let Some(url) = normalize_url(input) else {
        app::toast("Adresse invalide. Exemples : https://…, rtsp://…, rtp://@:1234");
        return;
    };
    let have_ytdlp = ytdlp_path().is_some_and(|p| p.is_file());
    if !needs_ytdl(&url) || have_ytdlp {
        m.cmd(&["loadfile", &url, "replace"]);
        return;
    }
    let Some(dest) = ytdlp_path() else { return };
    thread::spawn(move || {
        app::toast("Installation du module de lecture en ligne (yt-dlp)…");
        if let Some(dir) = dest.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let part = dest.with_extension("part");
        let ok = std::process::Command::new("curl.exe")
            .args(["-L", "-s", "-f", "-m", "300", "-o"])
            .arg(&part)
            .arg(YTDLP_URL)
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .is_ok_and(|s| s.success())
            && std::fs::rename(&part, &dest).is_ok();
        if ok {
            m.cmd(&["loadfile", &url, "replace"]);
        } else {
            let _ = std::fs::remove_file(&part);
            app::toast("Impossible d'installer yt-dlp : vérifie la connexion");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vlc_style_addresses() {
        assert_eq!(normalize_url("rtp://@:1234").as_deref(), Some("rtp://0.0.0.0:1234"));
        assert_eq!(normalize_url(" udp://@:5000 ").as_deref(), Some("udp://0.0.0.0:5000"));
        assert_eq!(normalize_url("mms://mms.examples.com/stream.asx").as_deref(), Some("mmsh://mms.examples.com/stream.asx"));
        assert_eq!(normalize_url("rtsp://server.example.org:8080/test.sdp").as_deref(), Some("rtsp://server.example.org:8080/test.sdp"));
        assert_eq!(normalize_url("www.youtube.com/watch?v=gg64x").as_deref(), Some("https://www.youtube.com/watch?v=gg64x"));
        assert_eq!(normalize_url("pas une adresse"), None);
        assert_eq!(normalize_url("://x"), None);
    }

    #[test]
    fn web_pages_need_ytdl_media_files_do_not() {
        assert!(needs_ytdl("https://www.youtube.com/watch?v=gg64x"));
        assert!(needs_ytdl("https://vimeo.com/123456"));
        assert!(!needs_ytdl("http://www.example.com/stream.avi"));
        assert!(!needs_ytdl("https://cdn.example.com/live/index.m3u8?token=1"));
        assert!(!needs_ytdl("rtsp://server.example.org:8080/test.sdp"));
    }
}
