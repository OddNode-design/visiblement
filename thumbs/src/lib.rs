//! Explorer thumbnails for videos. Windows asks this COM server (IThumbnailProvider) for a picture
//! of a file; libmpv, next to this DLL, decodes one frame of it (a tenth of the way in). Explorer
//! lays the default app's icon in the corner by itself, so the picture stays clean. Any failure
//! returns an error, and Explorer falls back to the file icon.

use std::cell::RefCell;
use std::ffi::{CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::Shell::PropertiesSystem::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage};
use windows::core::*;

/// Registered by the installer under HKCU\Software\Classes\CLSID.
pub const CLSID_THUMBNAILS: GUID = GUID::from_u128(0x3240865e_3ec8_438e_a36e_0c456e012f80);

#[implement(IThumbnailProvider, IInitializeWithStream)]
struct Thumbnails {
    stream: RefCell<Option<IStream>>,
}

/// Live COM objects: at zero, Explorer may unload the DLL (so an update can replace it).
static OBJECTS: AtomicU32 = AtomicU32::new(0);

impl Thumbnails {
    fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self { stream: RefCell::new(None) }
    }
}

impl Drop for Thumbnails {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IInitializeWithStream_Impl for Thumbnails_Impl {
    fn Initialize(&self, stream: Ref<IStream>, _mode: u32) -> Result<()> {
        *self.stream.borrow_mut() = Some(stream.ok()?.clone());
        Ok(())
    }
}

impl IThumbnailProvider_Impl for Thumbnails_Impl {
    fn GetThumbnail(&self, cx: u32, phbmp: *mut HBITMAP, alpha: *mut WTS_ALPHATYPE) -> Result<()> {
        let stream = self.stream.borrow().clone().ok_or(Error::from(E_UNEXPECTED))?;
        let frame = grab_frame(&stream, cx.clamp(16, 2048)).ok_or(Error::from(E_FAIL))?;
        // SAFETY: the out pointers come from the caller of GetThumbnail.
        unsafe {
            *phbmp = to_bitmap(&frame)?;
            *alpha = WTSAT_ARGB;
        }
        Ok(())
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, object: *mut *mut c_void) -> Result<()> {
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let unknown: IUnknown = Thumbnails::new().into();
        // SAFETY: iid and object come from COM.
        unsafe { unknown.query(iid, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// COM entry point: the class factory of CLSID_THUMBNAILS.
#[unsafe(no_mangle)]
extern "system" fn DllGetClassObject(clsid: *const GUID, iid: *const GUID, object: *mut *mut c_void) -> HRESULT {
    // SAFETY: COM passes valid pointers.
    if clsid.is_null() || unsafe { *clsid } != CLSID_THUMBNAILS {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = Factory.into();
    // SAFETY: as above.
    unsafe { factory.query(iid, object) }
}

/// Unloadable once no thumbnail object is alive (libmpv, loaded once, simply stays mapped).
#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(Ordering::SeqCst) == 0 { S_OK } else { S_FALSE }
}

// ---------- One frame through libmpv ----------

type Handle = *mut c_void;

#[repr(C)]
struct Event {
    id: i32,
    error: i32,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct StreamInfo {
    cookie: *mut c_void,
    read_fn: Option<unsafe extern "C" fn(*mut c_void, *mut c_char, u64) -> i64>,
    seek_fn: Option<unsafe extern "C" fn(*mut c_void, i64) -> i64>,
    size_fn: Option<unsafe extern "C" fn(*mut c_void) -> i64>,
    close_fn: Option<unsafe extern "C" fn(*mut c_void)>,
    cancel_fn: Option<unsafe extern "C" fn(*mut c_void)>,
}

type OpenFn = unsafe extern "C" fn(*mut c_void, *mut c_char, *mut StreamInfo) -> i32;

struct Mpv {
    _lib: libloading::Library,
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> i32,
    set_option: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> i32,
    command: unsafe extern "C" fn(Handle, *const *const c_char) -> i32,
    wait_event: unsafe extern "C" fn(Handle, f64) -> *mut Event,
    terminate_destroy: unsafe extern "C" fn(Handle),
    stream_cb_add_ro: unsafe extern "C" fn(Handle, *const c_char, *mut c_void, OpenFn) -> i32,
}

// SAFETY: plain function pointers into a library that stays loaded.
unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

const EVENT_SHUTDOWN: i32 = 1;
const EVENT_END_FILE: i32 = 7;

/// This DLL's folder: libmpv-2.dll is installed next to it.
fn own_dir() -> Option<PathBuf> {
    let mut module = HMODULE::default();
    // SAFETY: the address of a function in this DLL identifies the module.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(own_dir as *const u16),
            &mut module,
        )
        .ok()?;
    }
    let mut buf = [0u16; 1024];
    // SAFETY: buffer of the given size.
    let n = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    PathBuf::from(String::from_utf16_lossy(&buf[..n])).parent().map(Path::to_path_buf)
}

fn mpv() -> Option<&'static Mpv> {
    static MPV: OnceLock<Option<Mpv>> = OnceLock::new();
    MPV.get_or_init(|| {
        // VISIBLEMENT_MPV_DIR: tests run from target/release/deps, libmpv sits in target/release.
        let dir = std::env::var_os("VISIBLEMENT_MPV_DIR").map(PathBuf::from).or_else(own_dir)?;
        let path = dir.join("libmpv-2.dll");
        // SAFETY: loading libmpv; LOAD_WITH_ALTERED_SEARCH_PATH finds its neighbours by its own folder.
        let lib = unsafe { libloading::os::windows::Library::load_with_flags(&path, 0x0000_0008) }.ok()?;
        let lib = libloading::Library::from(lib);
        macro_rules! sym {
            ($name:literal) => {
                // SAFETY: the signatures match libmpv's client API.
                *unsafe { lib.get(concat!($name, "\0").as_bytes()) }.ok()?
            };
        }
        Some(Mpv {
            create: sym!("mpv_create"),
            initialize: sym!("mpv_initialize"),
            set_option: sym!("mpv_set_option_string"),
            command: sym!("mpv_command"),
            wait_event: sym!("mpv_wait_event"),
            terminate_destroy: sym!("mpv_terminate_destroy"),
            stream_cb_add_ro: sym!("mpv_stream_cb_add_ro"),
            _lib: lib,
        })
    })
    .as_ref()
}

/// RGBA pixels, row after row.
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

/// The file behind the shell's stream, when it says (file streams carry their full path).
fn stream_path(stream: &IStream) -> Option<PathBuf> {
    let mut stat = STATSTG::default();
    // SAFETY: STATFLAG_DEFAULT returns a CoTaskMem name, freed below.
    unsafe { stream.Stat(&mut stat, STATFLAG_DEFAULT).ok()? };
    if stat.pwcsName.is_null() {
        return None;
    }
    // SAFETY: a valid wide string from Stat.
    let name = unsafe { stat.pwcsName.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(stat.pwcsName.0 as *const c_void)) };
    name.map(PathBuf::from).filter(|p| p.is_absolute() && p.is_file())
}

/// What mpv's stream callbacks read through: the shell's stream, usable from any thread.
struct Source {
    stream: AgileReference<IStream>,
    closed: AtomicBool,
}

/// Lets COM calls from mpv's threads reach this (single-threaded) one while it waits.
fn pump_messages() {
    let mut msg = MSG::default();
    // SAFETY: a standard message loop on this thread's queue.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// One frame a tenth of the way into the media, fitted in `cx` × `cx`.
pub fn grab_frame(stream: &IStream, cx: u32) -> Option<Frame> {
    let m = mpv()?;
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let png = std::env::temp_dir().join(format!("visiblement-thumb-{}-{}.png", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)));
    // mpv reads the file itself when the stream gives its path (fast, no COM); otherwise through
    // the shell's stream, from its own threads.
    let path = stream_path(stream).filter(|_| std::env::var_os("VISIBLEMENT_THUMB_STREAM").is_none());
    let source = Box::new(Source { stream: AgileReference::new(stream).ok()?, closed: AtomicBool::new(false) });
    // SAFETY: a fresh handle, destroyed below on every path; `source` outlives it.
    unsafe {
        let h = (m.create)();
        if h.is_null() {
            return None;
        }
        let opt = |k: &str, v: &str| {
            let (k, v) = (CString::new(k).unwrap(), CString::new(v).unwrap_or_default());
            (m.set_option)(h, k.as_ptr(), v.as_ptr());
        };
        opt("config", "no");
        opt("terminal", "no");
        if let Some(log) = std::env::var_os("VISIBLEMENT_LOG") {
            opt("log-file", &format!("{}.thumb.txt", log.to_string_lossy()));
        }
        opt("o", &png.to_string_lossy());
        opt("ovc", "png");
        opt("aid", "no");
        opt("sid", "no");
        opt("frames", "1");
        opt("start", "10%");
        opt("hr-seek", "no");
        opt("vf", &format!("lavfi=[scale=w={cx}:h={cx}:force_original_aspect_ratio=decrease,format=rgba]"));
        if (m.initialize)(h) < 0 {
            (m.terminate_destroy)(h);
            return None;
        }
        (m.stream_cb_add_ro)(h, c"vbthumb".as_ptr(), &*source as *const _ as *mut c_void, stream_open);
        let url = CString::new(path.map_or("vbthumb://file".into(), |p| p.to_string_lossy().into_owned())).ok()?;
        let args = [c"loadfile".as_ptr(), url.as_ptr(), std::ptr::null()];
        (m.command)(h, args.as_ptr());
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            pump_messages();
            let ev = &*(m.wait_event)(h, 0.02);
            if ev.id == EVENT_END_FILE || ev.id == EVENT_SHUTDOWN {
                break;
            }
        }
        // From here the callbacks fail fast, so closing never waits on a COM call to this thread.
        source.closed.store(true, Ordering::SeqCst);
        pump_messages();
        (m.terminate_destroy)(h); // flushes and closes the PNG
    }
    drop(source);
    let frame = read_png(&png);
    let _ = std::fs::remove_file(&png);
    frame
}

fn read_png(path: &Path) -> Option<Frame> {
    let mut decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..w * h * 4].to_vec(),
        png::ColorType::Rgb => buf[..w * h * 3].chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..w * h * 2].chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf[..w * h].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some(Frame { w, h, rgba })
}

// mpv's stream callbacks: the cookie is the Source boxed in grab_frame.
fn with_stream<R>(cookie: *mut c_void, f: impl FnOnce(&IStream) -> R) -> Option<R> {
    // SAFETY: the cookie is the boxed Source, alive for the whole grab.
    let source = unsafe { &*(cookie as *const Source) };
    if source.closed.load(Ordering::SeqCst) {
        return None;
    }
    // SAFETY: mpv's threads are not COM threads yet; joining the MTA twice is harmless.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    source.stream.resolve().ok().map(|s| f(&s))
}

unsafe extern "C" fn stream_open(user: *mut c_void, _uri: *mut c_char, info: *mut StreamInfo) -> i32 {
    // SAFETY: mpv passes a valid info struct to fill.
    let info = unsafe { &mut *info };
    info.cookie = user;
    info.read_fn = Some(stream_read);
    info.seek_fn = Some(stream_seek);
    info.size_fn = Some(stream_size);
    info.close_fn = Some(stream_close); // required by mpv
    info.cancel_fn = None;
    with_stream(user, |s| {
        // SAFETY: rewinding the stream to the start.
        unsafe { s.Seek(0, STREAM_SEEK_SET, None) }.is_ok()
    })
    .filter(|ok| *ok)
    .map_or(-13, |_| 0) // MPV_ERROR_LOADING_FAILED
}

/// The Source is owned by grab_frame, which frees it after mpv is gone.
unsafe extern "C" fn stream_close(_cookie: *mut c_void) {}

unsafe extern "C" fn stream_read(cookie: *mut c_void, buf: *mut c_char, n: u64) -> i64 {
    with_stream(cookie, |s| {
        let mut read = 0u32;
        // SAFETY: mpv's buffer holds n bytes.
        let hr = unsafe { s.Read(buf as *mut c_void, n.min(u32::MAX as u64) as u32, Some(&mut read)) };
        if hr.is_err() { -1 } else { read as i64 }
    })
    .unwrap_or(-1)
}

unsafe extern "C" fn stream_seek(cookie: *mut c_void, offset: i64) -> i64 {
    with_stream(cookie, |s| {
        let mut pos = 0u64;
        // SAFETY: absolute seek.
        match unsafe { s.Seek(offset, STREAM_SEEK_SET, Some(&mut pos)) } {
            Ok(()) => pos as i64,
            Err(_) => -2, // MPV_ERROR_GENERIC
        }
    })
    .unwrap_or(-2)
}

unsafe extern "C" fn stream_size(cookie: *mut c_void) -> i64 {
    with_stream(cookie, |s| {
        let mut stat = STATSTG::default();
        // SAFETY: STATFLAG_NONAME leaves no name to free.
        match unsafe { s.Stat(&mut stat, STATFLAG_NONAME) } {
            Ok(()) => stat.cbSize as i64,
            Err(_) => -4, // MPV_ERROR_UNSUPPORTED
        }
    })
    .unwrap_or(-4)
}

// ---------- Bitmap ----------

/// A top-down 32-bit DIB with premultiplied alpha, as WTSAT_ARGB expects.
fn to_bitmap(frame: &Frame) -> Result<HBITMAP> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: frame.w as i32,
            biHeight: -(frame.h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: a fresh DIB section; `bits` points to w * h * 4 bytes.
    let bmp = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)? };
    let out = unsafe { std::slice::from_raw_parts_mut(bits as *mut u8, frame.w * frame.h * 4) };
    for (o, p) in out.chunks_mut(4).zip(frame.rgba.chunks(4)) {
        let a = p[3] as u32;
        o.copy_from_slice(&[(p[2] as u32 * a / 255) as u8, (p[1] as u32 * a / 255) as u8, (p[0] as u32 * a / 255) as u8, p[3]]);
    }
    Ok(bmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole Windows path: the handler registered in HKCU (installer, or reg add for a dev build),
    /// asked through the shell like Explorer does. Windows cannot decode ProRes itself: a white
    /// picture back means this handler drew it.
    #[test]
    #[ignore]
    fn shell_uses_the_registered_handler() {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap() };
        let tmp = std::env::temp_dir().join("visiblement-thumb-test");
        let _ = std::fs::create_dir_all(&tmp);
        let video = tmp.join(format!("white_{}.mov", std::process::id())); // a new name: nothing cached
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=white:size=640x360:rate=25:duration=2", "-c:v", "prores_ks"])
            .arg(&video).status().unwrap().success());
        let probe = std::env::var_os("VB_SHELL_FILE").map(PathBuf::from).unwrap_or(video.clone());
        let item: IShellItemImageFactory = unsafe { SHCreateItemFromParsingName(&HSTRING::from(probe.as_os_str()), None) }.unwrap();
        let bmp = unsafe { item.GetImage(SIZE { cx: 256, cy: 256 }, SIIGBF_THUMBNAILONLY) }.unwrap();
        let mut b = BITMAP::default();
        unsafe { GetObjectW(bmp.into(), size_of::<BITMAP>() as i32, Some(&mut b as *mut _ as *mut c_void)) };
        let (w, h) = (b.bmWidth as usize, b.bmHeight.unsigned_abs() as usize);
        let px = unsafe { std::slice::from_raw_parts(b.bmBits as *const u8, w * h * 4) };
        // The shell's copy holds its rows top-down whatever the sign of bmHeight (checked against
        // Windows' own PNG thumbnails).
        let top_down = true;
        let at = |x: usize, y: usize| { let row = if top_down { y } else { h - 1 - y }; px[(row * w + x) * 4] };
        let _ = std::fs::remove_file(&video);
        // What the shell returned, to look at: <temp>isiblement-thumb-test\shell.png
        let rgba: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| {
            let row = if top_down { y } else { h - 1 - y };
            let p = &px[(row * w + x) * 4..][..4];
            [p[2], p[1], p[0], 255]
        }).collect();
        let mut enc = png::Encoder::new(std::fs::File::create(tmp.join("shell.png")).unwrap(), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.write_header().unwrap().write_image_data(&rgba).unwrap();
        assert!(at(w / 10, h / 10) > 200, "white video");
        assert!(at(w - w / 8, h - h / 4) > 200, "no logo drawn in the picture ({w}x{h})");
    }

    /// Needs ffmpeg on PATH and VISIBLEMENT_MPV_DIR = target\release (where libmpv-2.dll is);
    /// VISIBLEMENT_THUMB_STREAM=1 forces the stream path instead of the file path.
    #[test]
    #[ignore]
    fn thumbnail_of_a_video_file() {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap() };
        let tmp = std::env::temp_dir().join("visiblement-thumb-test");
        let _ = std::fs::create_dir_all(&tmp);
        let video = tmp.join("clip.mp4");
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=25:duration=2", "-pix_fmt", "yuv420p"])
            .arg(&video).status().unwrap().success());
        let path = HSTRING::from(video.as_os_str());
        let stream = unsafe { SHCreateStreamOnFileEx(&path, STGM_READ.0, 0, false, None) }.unwrap();
        let unknown: IUnknown = Thumbnails::new().into();
        let init: IInitializeWithStream = unknown.cast().unwrap();
        unsafe { init.Initialize(&stream, STGM_READ.0) }.unwrap();
        let provider: IThumbnailProvider = unknown.cast().unwrap();
        let mut bmp = HBITMAP::default();
        let mut alpha = WTSAT_UNKNOWN;
        unsafe { provider.GetThumbnail(256, &mut bmp, &mut alpha) }.unwrap();
        let mut b = BITMAP::default();
        unsafe { GetObjectW(bmp.into(), size_of::<BITMAP>() as i32, Some(&mut b as *mut _ as *mut c_void)) };
        assert_eq!((b.bmWidth, b.bmHeight), (256, 144));
        // A copy to look at: <temp>isiblement-thumb-test	humb.png
        let f = grab_frame(&stream, 256).unwrap();
        let file = std::fs::File::create(tmp.join("thumb.png")).unwrap();
        let mut enc = png::Encoder::new(file, f.w as u32, f.h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.write_header().unwrap().write_image_data(&f.rgba).unwrap();
    }
}
