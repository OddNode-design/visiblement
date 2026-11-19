//! In-app update: downloads the latest release's .msi with a progress bar in the UI, then
//! hands over to Windows Installer (which upgrades in place) and relaunches the app.

use crate::app;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The newer release's installer: (tag, download URL, size in bytes).
static ASSET: Mutex<Option<(String, String, u64)>> = Mutex::new(None);

/// The .msi asset of a GitHub "latest release" answer: (download URL, size).
pub fn msi_asset(release_json: &str) -> Option<(String, u64)> {
    let v: serde_json::Value = serde_json::from_str(release_json).ok()?;
    v["assets"].as_array()?.iter().find_map(|a| {
        let name = a["name"].as_str()?;
        name.to_ascii_lowercase().ends_with(".msi").then(|| {
            (a["browser_download_url"].as_str().unwrap_or_default().to_string(), a["size"].as_u64().unwrap_or(0))
        })
    }).filter(|(url, _)| url.starts_with("https://"))
}

pub fn remember(tag: &str, asset: Option<(String, u64)>) {
    if let Ok(mut slot) = ASSET.lock() {
        *slot = asset.map(|(url, size)| (tag.to_string(), url, size));
    }
}

/// True when the update can be installed from inside the app (an .msi is attached to the release).
pub fn installable() -> bool {
    ASSET.lock().map(|a| a.is_some()).unwrap_or(false)
}

fn ui(js: &str) {
    app::eval_js(&format!("window.vb && {js}"));
}

/// Where the .msi installs the app (per-user install, see installer/visiblement.wxs).
fn installed_exe() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join("Visiblement").join("visiblement.exe"))
}

/// Downloads the installer (progress → `vb.updateProgress(pct)`), then runs it and closes the app.
pub fn install() {
    let Some((tag, url, size)) = ASSET.lock().ok().and_then(|a| a.clone()) else {
        ui("vb.updateFailed()");
        return;
    };
    thread::spawn(move || {
        let dest = std::env::temp_dir().join(format!("Visiblement-{tag}-x64.msi"));
        let _ = std::fs::remove_file(&dest);
        let child = std::process::Command::new("curl.exe")
            .args(["-L", "-s", "-f", "-m", "600", "-o"])
            .arg(&dest)
            .arg(&url)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        let Ok(mut child) = child else {
            ui("vb.updateFailed()");
            return;
        };
        let ok = loop {
            if let Ok(Some(status)) = child.try_wait() {
                break status.success();
            }
            let got = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
            if let Some(pct) = (got * 100).checked_div(size) {
                ui(&format!("vb.updateProgress({})", pct.min(99)));
            }
            thread::sleep(Duration::from_millis(150));
        };
        let got = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        if !ok || got == 0 || (size > 0 && got != size) {
            let _ = std::fs::remove_file(&dest);
            ui("vb.updateFailed()");
            return;
        }
        ui("vb.updateProgress(\"install\")");

        // Windows Installer can only replace the files once this process has exited: a hidden
        // cmd waits ~2 s, runs the installer with its small progress window, then starts the
        // freshly installed app.
        let exe = installed_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let script = format!(
            r#"/s /c "ping -n 3 127.0.0.1 >nul & msiexec /i "{}" /passive /norestart & start "" "{}""#,
            dest.to_string_lossy(),
            exe
        );
        let started = std::process::Command::new("cmd.exe").raw_arg(script).creation_flags(CREATE_NO_WINDOW).spawn();
        if started.is_err() {
            ui("vb.updateFailed()");
            return;
        }
        thread::sleep(Duration::from_millis(600)); // let the "Installation…" state show
        app::request_close();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_msi_asset() {
        let json = r#"{"tag_name":"v0.2.0","assets":[
            {"name":"Visiblement-windows-x64.zip","browser_download_url":"https://github.com/x/y.zip","size":10},
            {"name":"Visiblement-0.2.0-x64.msi","browser_download_url":"https://github.com/x/y.msi","size":42}]}"#;
        assert_eq!(msi_asset(json), Some(("https://github.com/x/y.msi".into(), 42)));
        assert_eq!(msi_asset(r#"{"assets":[]}"#), None);
        assert_eq!(msi_asset("not json"), None);
    }
}
