fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    // Embeds the icon as resource id 1: used by Explorer and loaded again for the mpv window.
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.compile().expect("icon resource");
}
