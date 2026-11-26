fn main() {
    // The icon resource only exists for Windows targets (winresource is a Windows-only build dependency).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        // Embeds the icon as resource id 1: used by Explorer and loaded again for the mpv window.
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().expect("icon resource");
    }
}
