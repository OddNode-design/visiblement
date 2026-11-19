//! NDI sources, received with the NDI runtime that NDI Tools installs (Processing.NDI.Lib.x64.dll),
//! and handed to mpv as raw BGRA video through a custom `ndi://` stream (mpv_stream_cb).
//! Video only for now; frames are kept in a short queue (oldest dropped) so latency never builds up.

use crate::{app, Mpv};
use libloading::Library;
use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[repr(C)]
struct Source {
    name: *const c_char,
    url: *const c_char,
}

#[repr(C)]
struct RecvCreate {
    source: Source,
    color_format: i32, // 0 = BGRX_BGRA
    bandwidth: i32,    // 100 = highest
    allow_video_fields: u8,
    recv_name: *const c_char,
}

#[repr(C)]
struct VideoFrame {
    xres: i32,
    yres: i32,
    fourcc: u32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    aspect: f32,
    format: i32,
    timecode: i64,
    data: *mut u8,
    line_stride: i32,
    metadata: *const c_char,
    timestamp: i64,
}

const FRAME_VIDEO: i32 = 1;
const QUEUE: usize = 3;

struct Api {
    _lib: Library,
    find_create: unsafe extern "C" fn(*const c_void) -> *mut c_void,
    find_wait: unsafe extern "C" fn(*mut c_void, u32) -> bool,
    find_sources: unsafe extern "C" fn(*mut c_void, *mut u32) -> *const Source,
    find_destroy: unsafe extern "C" fn(*mut c_void),
    recv_create: unsafe extern "C" fn(*const RecvCreate) -> *mut c_void,
    recv_capture: unsafe extern "C" fn(*mut c_void, *mut VideoFrame, *mut c_void, *mut c_void, u32) -> i32,
    recv_free_video: unsafe extern "C" fn(*mut c_void, *const VideoFrame),
    recv_destroy: unsafe extern "C" fn(*mut c_void),
}

/// The NDI runtime, loaded once. None when NDI Tools / NDI Runtime is not installed.
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let dirs = ["NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"].iter().filter_map(std::env::var_os).map(PathBuf::from);
        // SAFETY: loading the vendor DLL; its DllMain has no special requirements.
        let lib = dirs
            .map(|d| d.join("Processing.NDI.Lib.x64.dll"))
            .find_map(|p| unsafe { Library::new(p) }.ok())
            .or_else(|| unsafe { Library::new("Processing.NDI.Lib.x64.dll") }.ok())?;
        macro_rules! sym {
            ($n:literal) => {
                // SAFETY: signatures follow Processing.NDI.Lib.h (NDI SDK 5/6).
                *unsafe { lib.get(concat!($n, "\0").as_bytes()) }.ok()?
            };
        }
        let init: unsafe extern "C" fn() -> bool = sym!("NDIlib_initialize");
        // SAFETY: plain call into the runtime.
        if !unsafe { init() } {
            return None;
        }
        Some(Api {
            find_create: sym!("NDIlib_find_create_v2"),
            find_wait: sym!("NDIlib_find_wait_for_sources"),
            find_sources: sym!("NDIlib_find_get_current_sources"),
            find_destroy: sym!("NDIlib_find_destroy"),
            recv_create: sym!("NDIlib_recv_create_v3"),
            recv_capture: sym!("NDIlib_recv_capture_v2"),
            recv_free_video: sym!("NDIlib_recv_free_video_v2"),
            recv_destroy: sym!("NDIlib_recv_destroy"),
            _lib: lib,
        })
    })
    .as_ref()
}

/// NDI sources visible on the network (waits up to ~2.5 s for them to announce themselves).
/// None when the NDI runtime is missing.
pub fn sources() -> Option<Vec<String>> {
    let api = api()?;
    // SAFETY: the finder handle is used only here and destroyed before returning.
    unsafe {
        let finder = (api.find_create)(ptr::null());
        if finder.is_null() {
            return Some(Vec::new());
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(2500) {
            (api.find_wait)(finder, 500);
        }
        let mut n = 0u32;
        let list = (api.find_sources)(finder, &mut n);
        let names = (0..n as usize)
            .filter_map(|i| {
                let s = &*list.add(i);
                (!s.name.is_null()).then(|| CStr::from_ptr(s.name).to_string_lossy().into_owned())
            })
            .collect();
        (api.find_destroy)(finder);
        Some(names)
    }
}

/// Frames waiting for mpv, shared between the receiver thread and mpv's stream callbacks.
struct Feed {
    frames: Mutex<VecDeque<Vec<u8>>>,
    ready: Condvar,
    stop: AtomicBool,
    /// Bytes of the frame mpv is currently reading, and how far it got.
    current: Mutex<(Vec<u8>, usize)>,
}

impl Feed {
    fn push(&self, frame: Vec<u8>) {
        let mut q = self.frames.lock().unwrap();
        while q.len() >= QUEUE {
            q.pop_front(); // mpv is behind: drop the oldest frame rather than add latency
        }
        q.push_back(frame);
        self.ready.notify_one();
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.ready.notify_all();
    }
}

/// The feed of the NDI source being played (one at a time).
static ACTIVE: Mutex<Option<Arc<Feed>>> = Mutex::new(None);

/// Copies a BGRA frame into a contiguous buffer (the source may pad its lines).
fn copy_frame(f: &VideoFrame) -> Vec<u8> {
    let (w, h) = (f.xres as usize, f.yres as usize);
    let row = w * 4;
    let stride = if f.line_stride > 0 { f.line_stride as usize } else { row };
    let mut out = vec![0u8; row * h];
    for y in 0..h {
        // SAFETY: the runtime guarantees `yres` lines of `line_stride` bytes at `data`.
        let src = unsafe { std::slice::from_raw_parts(f.data.add(y * stride), row) };
        out[y * row..(y + 1) * row].copy_from_slice(src);
    }
    out
}

/// Connects to an NDI source and plays it: waits for its first frame (to learn the size and frame
/// rate), then loads `ndi://<name>` in mpv as raw video and keeps feeding it.
pub fn open(m: Mpv, name: String) {
    let Some(api) = api() else {
        app::toast("Runtime NDI introuvable : installez NDI Tools");
        return;
    };
    if let Some(old) = ACTIVE.lock().unwrap().take() {
        old.stop();
    }
    thread::spawn(move || {
        app::toast(&format!("Connexion à {name}…"));
        let c_name = CString::new(name.clone()).unwrap_or_default();
        let c_recv = c"Visiblement";
        let create = RecvCreate {
            source: Source { name: c_name.as_ptr(), url: ptr::null() },
            color_format: 0,
            bandwidth: 100,
            allow_video_fields: 0,
            recv_name: c_recv.as_ptr(),
        };
        // SAFETY: `create` and the strings it points to outlive the call.
        let recv = unsafe { (api.recv_create)(&create) };
        if recv.is_null() {
            app::toast("Impossible de se connecter à la source NDI");
            return;
        }
        let feed = Arc::new(Feed {
            frames: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            stop: AtomicBool::new(false),
            current: Mutex::new((Vec::new(), 0)),
        });
        *ACTIVE.lock().unwrap() = Some(feed.clone());

        let mut size: Option<(i32, i32)> = None;
        let start = Instant::now();
        while !feed.stop.load(Ordering::Relaxed) {
            // SAFETY: zeroed frame filled by the runtime; freed below when it holds video.
            let mut frame: VideoFrame = unsafe { std::mem::zeroed() };
            let kind = unsafe { (api.recv_capture)(recv, &mut frame, ptr::null_mut(), ptr::null_mut(), 500) };
            if kind != FRAME_VIDEO {
                if size.is_none() && start.elapsed() > Duration::from_secs(6) {
                    app::toast("La source NDI n'envoie pas d'image");
                    break;
                }
                continue;
            }
            let this = (frame.xres, frame.yres);
            let data = copy_frame(&frame);
            let fps = if frame.frame_rate_d > 0 { frame.frame_rate_n as f64 / frame.frame_rate_d as f64 } else { 30.0 };
            unsafe { (api.recv_free_video)(recv, &frame) };
            match size {
                None => {
                    size = Some(this);
                    feed.push(data);
                    let opts = format!(
                        "demuxer=rawvideo,demuxer-rawvideo-w={},demuxer-rawvideo-h={},demuxer-rawvideo-fps={fps:.3},\
                         demuxer-rawvideo-mp-format=bgra,untimed=yes,cache=no,demuxer-readahead-secs=0,force-media-title={name}",
                        this.0, this.1
                    );
                    m.cmd(&["loadfile", &format!("ndi://{name}"), "replace", "-1", &opts]);
                }
                // Same size: feed it. A new size needs a reload with the new dimensions.
                Some(s) if s == this => feed.push(data),
                Some(_) => {
                    feed.stop();
                    let (m2, n2) = (m, name.clone());
                    thread::spawn(move || open(m2, n2));
                    break;
                }
            }
        }
        // SAFETY: the receiver is not used after this.
        unsafe { (api.recv_destroy)(recv) };
    });
}

// ---- mpv stream callbacks (registered once with mpv_stream_cb_add_ro for "ndi://") ----

#[repr(C)]
pub struct StreamInfo {
    cookie: *mut c_void,
    read_fn: Option<unsafe extern "C" fn(*mut c_void, *mut c_char, u64) -> i64>,
    seek_fn: Option<unsafe extern "C" fn(*mut c_void, i64) -> i64>,
    size_fn: Option<unsafe extern "C" fn(*mut c_void) -> i64>,
    close_fn: Option<unsafe extern "C" fn(*mut c_void)>,
    cancel_fn: Option<unsafe extern "C" fn(*mut c_void)>,
}

pub type OpenFn = unsafe extern "C" fn(*mut c_void, *mut c_char, *mut StreamInfo) -> i32;

/// mpv opens `ndi://…`: hand it the active feed.
pub unsafe extern "C" fn stream_open(_user: *mut c_void, _uri: *mut c_char, info: *mut StreamInfo) -> i32 {
    let Some(feed) = ACTIVE.lock().ok().and_then(|a| a.clone()) else {
        return -13; // MPV_ERROR_LOADING_FAILED
    };
    // SAFETY: mpv passes a valid info struct to fill.
    let info = unsafe { &mut *info };
    info.cookie = Arc::into_raw(feed) as *mut c_void;
    info.read_fn = Some(stream_read);
    info.seek_fn = None;
    info.size_fn = None;
    info.close_fn = Some(stream_close);
    info.cancel_fn = Some(stream_cancel);
    0
}

unsafe extern "C" fn stream_read(cookie: *mut c_void, buf: *mut c_char, nbytes: u64) -> i64 {
    // SAFETY: cookie is the Arc<Feed> given in stream_open, alive until stream_close.
    let feed = unsafe { &*(cookie as *const Feed) };
    let mut cur = feed.current.lock().unwrap();
    if cur.1 >= cur.0.len() {
        let mut q = feed.frames.lock().unwrap();
        loop {
            if feed.stop.load(Ordering::Relaxed) {
                return 0; // end of stream
            }
            if let Some(f) = q.pop_front() {
                *cur = (f, 0);
                break;
            }
            q = feed.ready.wait_timeout(q, Duration::from_millis(200)).unwrap().0;
        }
    }
    let (frame, pos) = &mut *cur;
    let n = (frame.len() - *pos).min(nbytes as usize);
    // SAFETY: mpv gives a buffer of at least `nbytes` bytes.
    unsafe { ptr::copy_nonoverlapping(frame.as_ptr().add(*pos), buf as *mut u8, n) };
    *pos += n;
    n as i64
}

unsafe extern "C" fn stream_close(cookie: *mut c_void) {
    // SAFETY: matches Arc::into_raw in stream_open.
    let feed = unsafe { Arc::from_raw(cookie as *const Feed) };
    feed.stop();
}

unsafe extern "C" fn stream_cancel(cookie: *mut c_void) {
    // SAFETY: still owned by mpv until stream_close.
    unsafe { &*(cookie as *const Feed) }.stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_struct_matches_the_sdk_layout() {
        // NDIlib_video_frame_v2_t is 72 bytes on x64, p_data at offset 40.
        assert_eq!(std::mem::size_of::<VideoFrame>(), 72);
        assert_eq!(std::mem::offset_of!(VideoFrame, data), 40);
        assert_eq!(std::mem::size_of::<RecvCreate>(), 40);
    }

    #[test]
    fn padded_lines_are_compacted() {
        let mut src = vec![1u8; 4 * 2 + 8]; // 2 px wide + 8 bytes of padding
        src.extend(vec![2u8; 4 * 2 + 8]);
        let f = VideoFrame {
            xres: 2, yres: 2, fourcc: 0, frame_rate_n: 30, frame_rate_d: 1, aspect: 1.0, format: 0, timecode: 0,
            data: src.as_mut_ptr(), line_stride: 16, metadata: ptr::null(), timestamp: 0,
        };
        assert_eq!(copy_frame(&f), [vec![1u8; 8], vec![2u8; 8]].concat());
    }
}
