//! macOS port: the window helpers `win::*` of the Windows build (same names, same physical-pixel
//! rectangles with a top-left origin, so app.rs does not care) and the libmpv render loop.
//!
//! Why a render loop: mpv's `wid` option does not exist on macOS, so mpv cannot draw into a window
//! we hand it. Instead the video window gets an `NSOpenGLView`, a dedicated thread owns its GL
//! context, and mpv renders into that context's framebuffer 0 through the libmpv render API
//! (`vo=libmpv`). The thread sleeps until mpv's update callback (a new frame, an OSD change) or a
//! size change wakes it.
//!
//! Coordinates: AppKit uses points with the origin at the bottom left of the primary screen; tao and
//! app.rs use physical pixels from the top left. Conversions go through the scale factor of the
//! window (or of the screen under the mouse for `cursor`), as tao does.
//!
//! Everything here runs on the main thread except `render_loop`.

use crate::Mpv;
use libloading::Library;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationPresentationOptions, NSEvent, NSScreen, NSWindow,
    NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub type Rect = (i32, i32, i32, i32);

// --- Windows: positions and sizes -------------------------------------------------------------

/// SAFETY (all callers): `h` is the NSWindow pointer of a live tao window (`ns_window()`), and only
/// the event-loop (main) thread calls these helpers.
fn window<'a>(h: isize) -> &'a NSWindow {
    unsafe { &*(h as *const NSWindow) }
}

/// Height of the primary screen (the one with the menu bar, first in `screens`), the y-flip reference.
fn primary_height(mtm: MainThreadMarker) -> f64 {
    let screens = NSScreen::screens(mtm);
    if screens.count() == 0 {
        0.0
    } else {
        screens.objectAtIndex(0).frame().size.height
    }
}

/// AppKit frame (points, bottom-left origin) -> (x, y, w, h) in physical pixels from the top left.
fn to_px(f: NSRect, primary_h: f64, scale: f64) -> Rect {
    let r = |v: f64| (v * scale).round() as i32;
    (
        r(f.origin.x),
        r(primary_h - f.origin.y - f.size.height),
        r(f.size.width),
        r(f.size.height),
    )
}

/// The opposite conversion, for `setFrame:`.
fn from_px((x, y, w, h): Rect, primary_h: f64, scale: f64) -> NSRect {
    let (w, h) = (f64::from(w.max(1)) / scale, f64::from(h.max(1)) / scale);
    NSRect::new(
        NSPoint::new(f64::from(x) / scale, primary_h - f64::from(y) / scale - h),
        NSSize::new(w, h),
    )
}

/// (whole screen, visible area without menu bar and Dock) of the screen holding the window.
pub fn monitor_rects(h: isize) -> Option<(Rect, Rect)> {
    let mtm = MainThreadMarker::new()?;
    let screen = window(h).screen()?;
    let (top, scale) = (primary_height(mtm), screen.backingScaleFactor());
    Some((
        to_px(screen.frame(), top, scale),
        to_px(screen.visibleFrame(), top, scale),
    ))
}

/// (x, y, width, height) of a window, as AppKit has it right now.
pub fn window_rect(h: isize) -> Option<Rect> {
    let mtm = MainThreadMarker::new()?;
    let w = window(h);
    Some(to_px(
        w.frame(),
        primary_height(mtm),
        w.backingScaleFactor(),
    ))
}

/// The mouse in physical screen pixels (scale of the screen it is on).
pub fn cursor() -> Option<(f64, f64)> {
    let mtm = MainThreadMarker::new()?;
    let p = NSEvent::mouseLocation();
    let screens = NSScreen::screens(mtm);
    let scale = (0..screens.count())
        .map(|i| screens.objectAtIndex(i))
        .find(|s| {
            let f = s.frame();
            p.x >= f.origin.x
                && p.x < f.origin.x + f.size.width
                && p.y >= f.origin.y
                && p.y < f.origin.y + f.size.height
        })
        .map_or(1.0, |s| s.backingScaleFactor());
    Some((p.x * scale, (primary_height(mtm) - p.y) * scale))
}

fn set_frame(h: isize, rect: Rect) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let w = window(h);
    w.setFrame_display(
        from_px(rect, primary_height(mtm), w.backingScaleFactor()),
        true,
    );
}

/// Moves and resizes a window.
pub fn set_rect(h: isize, rect: Rect) {
    set_frame(h, rect);
}

/// Moves / resizes the UI window and the video window under it. The video window is the UI window's
/// child, so it already follows the move; it still gets the exact rectangle (resizes).
pub fn move_pair(ui: isize, video: isize, rect: Rect) {
    set_frame(ui, rect);
    set_frame(video, rect);
}

pub fn place_frameless(h: isize, x: i32, y: i32, w: i32, hh: i32) {
    set_frame(h, (x, y, w, hh));
}

/// Makes `child` follow `parent` (moves, minimize) and stay right below it.
pub fn attach_child(parent: isize, child: isize) {
    // SAFETY: two live windows of ours, main thread.
    unsafe { window(parent).addChildWindow_ordered(window(child), NSWindowOrderingMode::Below) };
}

/// Fullscreen = a window as big as the screen; this hides the menu bar and the Dock over it.
pub fn presentation(fullscreen: bool) {
    static ON: AtomicBool = AtomicBool::new(false);
    if ON.swap(fullscreen, Ordering::Relaxed) == fullscreen {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let options = if fullscreen {
        NSApplicationPresentationOptions::HideDock | NSApplicationPresentationOptions::HideMenuBar
    } else {
        NSApplicationPresentationOptions::Default
    };
    NSApplication::sharedApplication(mtm).setPresentationOptions(options);
}

// Windows-only chores (frame styles, DWM corners): nothing to do here, the windows are borderless.
pub fn strip_frame(_h: isize) {}
pub fn never_paint_frame(_h: isize) {}
pub fn round_corners(_h: isize, _round: bool) {}

// --- Shell ------------------------------------------------------------------------------------

pub fn open_url(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// Opens Finder with the file selected.
pub fn show_in_folder(path: &str) {
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
}

/// Error box (startup failures), through AppleScript: no AppKit needed before the event loop runs.
pub fn alert(msg: &str) {
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let script = format!(
        "display alert {} message {} as critical",
        quote(crate::APP),
        quote(msg)
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status();
}

// --- libmpv render API ------------------------------------------------------------------------

/// The part of libmpv's render.h / render_gl.h that is used. Layouts follow the C headers.
#[derive(Clone, Copy)]
pub struct RenderApi {
    pub create: unsafe extern "C" fn(*mut *mut c_void, *mut c_void, *mut Param) -> c_int,
    pub set_update_callback:
        unsafe extern "C" fn(*mut c_void, unsafe extern "C" fn(*mut c_void), *mut c_void),
    pub update: unsafe extern "C" fn(*mut c_void) -> u64,
    pub render: unsafe extern "C" fn(*mut c_void, *mut Param) -> c_int,
    pub report_swap: unsafe extern "C" fn(*mut c_void),
    pub free: unsafe extern "C" fn(*mut c_void),
}

/// mpv_render_param { enum mpv_render_param_type type; void *data; }
#[repr(C)]
pub struct Param {
    kind: c_int,
    data: *mut c_void,
}

const PARAM_END: Param = Param {
    kind: 0,
    data: null_mut(),
};
const PARAM_API_TYPE: c_int = 1; // char *, "opengl"
const PARAM_OPENGL_INIT: c_int = 2; // mpv_opengl_init_params *
const PARAM_OPENGL_FBO: c_int = 3; // mpv_opengl_fbo *
const PARAM_FLIP_Y: c_int = 4; // int *

/// mpv_opengl_init_params { get_proc_address, get_proc_address_ctx }
#[repr(C)]
struct GlInit {
    get_proc_address: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    ctx: *mut c_void,
}

/// mpv_opengl_fbo { int fbo; int w; int h; int internal_format; }
#[repr(C)]
struct Fbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

#[link(name = "OpenGL", kind = "framework")]
unsafe extern "C" {
    fn CGLGetCurrentContext() -> *mut c_void;
    fn CGLLockContext(ctx: *mut c_void) -> c_int;
    fn CGLUnlockContext(ctx: *mut c_void) -> c_int;
    fn CGLSetParameter(ctx: *mut c_void, pname: c_int, params: *const c_int) -> c_int;
    fn glClearColor(r: f32, g: f32, b: f32, a: f32);
    fn glClear(mask: u32);
}

const CGL_SWAP_INTERVAL: c_int = 222; // kCGLCPSwapInterval
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;

/// mpv resolves its OpenGL entry points here: `dlsym` in the OpenGL framework.
unsafe extern "C" fn get_proc_address(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    static GL: OnceLock<Option<Library>> = OnceLock::new();
    let lib = GL.get_or_init(|| {
        unsafe { Library::new("/System/Library/Frameworks/OpenGL.framework/OpenGL") }.ok()
    });
    let Some(lib) = lib else { return null_mut() };
    // SAFETY: mpv passes a NUL-terminated name; the symbol is only used as an address.
    unsafe { lib.get::<*mut c_void>(CStr::from_ptr(name).to_bytes_with_nul()) }
        .map_or(null_mut(), |s| *s)
}

/// Shared with the render thread: wake-up flag, size in backing pixels (width << 32 | height), stop.
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
static SIZE: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicBool = AtomicBool::new(false);

fn wake() {
    *WAKE.0.lock().unwrap_or_else(PoisonError::into_inner) = true;
    WAKE.1.notify_one();
}

/// mpv's update callback: may be called from any mpv thread, must not call back into mpv.
unsafe extern "C" fn on_update(_ctx: *mut c_void) {
    wake();
}

/// Size of the video window in physical (backing) pixels; the render thread picks it up.
pub fn set_render_size(w: u32, h: u32) {
    if SIZE.swap(u64::from(w) << 32 | u64::from(h), Ordering::Relaxed)
        != u64::from(w) << 32 | u64::from(h)
    {
        wake();
    }
}

pub struct Renderer {
    thread: Option<JoinHandle<()>>,
    /// Keeps the view alive (the window's view hierarchy holds it too).
    _view: Retained<AnyObject>,
}

impl Renderer {
    /// Stops the render thread, which frees the mpv render context first: to be called before
    /// `mpv_terminate_destroy`.
    pub fn stop(&mut self) {
        STOP.store(true, Ordering::Relaxed);
        wake();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// An `NSOpenGLView` filling `parent` (the video window's content view), with a 4.1 core context.
/// Returns the view and its context.
fn gl_view(parent: *mut c_void) -> Result<(Retained<AnyObject>, Retained<AnyObject>), String> {
    const ERR: &str = "Impossible de créer la vue OpenGL de la fenêtre vidéo.";
    // NSOpenGLPFAOpenGLProfile = 99, NSOpenGLProfileVersion4_1Core = 0x4100, NSOpenGLPFADoubleBuffer = 5,
    // NSOpenGLPFAAccelerated = 73, NSOpenGLPFAAllowOfflineRenderers = 96, 0 ends the list.
    // A Mac without a GPU (the CI virtual machine) has no accelerated renderer: then Apple's software one.
    let attempts: [&[u32]; 2] = [&[99, 0x4100, 5, 73, 96, 0], &[99, 0x4100, 5, 96, 0]];
    // SAFETY: `parent` is tao's NSView of the video window, used on the main thread; selectors and
    // argument types follow AppKit's NSOpenGLPixelFormat / NSOpenGLView / NSView.
    unsafe {
        let parent = &*(parent as *const AnyObject);
        let format_class = AnyClass::get(c"NSOpenGLPixelFormat").ok_or(ERR)?;
        let view_class = AnyClass::get(c"NSOpenGLView").ok_or(ERR)?;
        let format = attempts
            .iter()
            .find_map(|attributes| {
                let alloc: Allocated<AnyObject> = msg_send![format_class, alloc];
                let format: Option<Retained<AnyObject>> = msg_send![alloc, initWithAttributes: attributes.as_ptr()];
                format
            })
            .ok_or(format!("{ERR} (NSOpenGLPixelFormat)"))?;
        let bounds: NSRect = msg_send![parent, bounds];
        let alloc: Allocated<AnyObject> = msg_send![view_class, alloc];
        let view: Option<Retained<AnyObject>> =
            msg_send![alloc, initWithFrame: bounds, pixelFormat: &*format];
        let view = view.ok_or(format!("{ERR} (NSOpenGLView)"))?;
        let _: () = msg_send![&*view, setWantsBestResolutionOpenGLSurface: true]; // Retina: draw in backing pixels
        let _: () = msg_send![&*view, setAutoresizingMask: 18usize]; // NSViewWidthSizable | NSViewHeightSizable
        let _: () = msg_send![parent, addSubview: &*view];
        let ctx: Option<Retained<AnyObject>> = msg_send![&*view, openGLContext];
        let ctx = ctx.ok_or(format!("{ERR} (openGLContext)"))?;
        let _: () = msg_send![&*ctx, setView: &*view];
        Ok((view, ctx))
    }
}

/// Creates the GL view in the video window and starts the thread that draws mpv into it.
/// To be called once, after `mpv_initialize`, from the main thread. `ns_view` is `video.ns_view()`.
pub fn start_render(m: &Mpv, ns_view: *mut c_void) -> Result<Renderer, String> {
    let (view, ctx) = gl_view(ns_view)?;
    let (api, mpv) = (m.render, m.h as usize);
    // The context pointer crosses to the render thread as a number, along with its +1 retain: the
    // thread releases it when it ends.
    let ctx_ptr = Retained::into_raw(ctx) as usize;
    let (tx, rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("mpv-render".into())
        .spawn(move || render_loop(api, mpv as *mut c_void, ctx_ptr as *mut AnyObject, &tx))
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(Renderer {
            thread: Some(thread),
            _view: view,
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("Le rendu mpv ne démarre pas.".into()),
    }
}

/// Owns the GL context: makes it current, creates the mpv render context, then draws on every
/// wake-up until `STOP`. The mpv render context is freed here, with the GL context still current.
fn render_loop(
    api: RenderApi,
    mpv: *mut c_void,
    ctx: *mut AnyObject,
    ready: &mpsc::Sender<Result<(), String>>,
) {
    // SAFETY: `ctx` is the retained NSOpenGLContext made by `gl_view` (released at the end); GL and
    // CGL calls happen on this thread only, with the context current, under CGLLockContext while
    // drawing. The mpv calls follow render.h: create / render / free on this thread.
    unsafe {
        let Some(ctx) = Retained::from_raw(ctx) else {
            let _ = ready.send(Err("Contexte OpenGL absent.".into()));
            return;
        };
        let _: () = msg_send![&*ctx, makeCurrentContext];
        let cgl = CGLGetCurrentContext();
        CGLSetParameter(cgl, CGL_SWAP_INTERVAL, &1); // vsync
        let mut init = GlInit {
            get_proc_address,
            ctx: null_mut(),
        };
        let mut params = [
            Param {
                kind: PARAM_API_TYPE,
                data: c"opengl".as_ptr() as *mut c_void,
            },
            Param {
                kind: PARAM_OPENGL_INIT,
                data: (&raw mut init).cast(),
            },
            PARAM_END,
        ];
        let mut rc: *mut c_void = null_mut();
        let err = (api.create)(&mut rc, mpv, params.as_mut_ptr());
        if err < 0 || rc.is_null() {
            let _ = ready.send(Err(format!("mpv_render_context_create a échoué ({err}).")));
            return;
        }
        (api.set_update_callback)(rc, on_update, null_mut());
        let _ = ready.send(Ok(()));

        let mut last = 0u64;
        wake(); // a first draw: the idle background
        loop {
            {
                let guard = WAKE.0.lock().unwrap_or_else(PoisonError::into_inner);
                let mut guard = WAKE
                    .1
                    .wait_while(guard, |w| !*w && !STOP.load(Ordering::Relaxed))
                    .unwrap_or_else(PoisonError::into_inner);
                *guard = false;
            }
            if STOP.load(Ordering::Relaxed) {
                break;
            }
            let size = SIZE.load(Ordering::Relaxed);
            let (w, h) = ((size >> 32) as c_int, (size & 0xFFFF_FFFF) as c_int);
            if w == 0 || h == 0 {
                continue;
            }
            CGLLockContext(cgl);
            if size != last {
                let _: () = msg_send![&*ctx, update]; // the drawable changed size
                last = size;
            }
            (api.update)(rc);
            // Idle screen and audio files: the logo tile colour (#1b1a1e); a video covers it.
            glClearColor(27.0 / 255.0, 26.0 / 255.0, 30.0 / 255.0, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);
            let mut fbo = Fbo {
                fbo: 0,
                w,
                h,
                internal_format: 0,
            };
            let mut flip: c_int = 1; // OpenGL's origin is the bottom left
            let mut params = [
                Param {
                    kind: PARAM_OPENGL_FBO,
                    data: (&raw mut fbo).cast(),
                },
                Param {
                    kind: PARAM_FLIP_Y,
                    data: (&raw mut flip).cast(),
                },
                PARAM_END,
            ];
            (api.render)(rc, params.as_mut_ptr());
            let _: () = msg_send![&*ctx, flushBuffer];
            (api.report_swap)(rc);
            CGLUnlockContext(cgl);
        }
        CGLLockContext(cgl);
        (api.free)(rc);
        CGLUnlockContext(cgl);
        if let Some(class) = AnyClass::get(c"NSOpenGLContext") {
            let _: () = msg_send![class, clearCurrentContext];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1000x800 pt primary screen, scale 2: a frame at the bottom-left maps to the bottom-left in pixels.
    #[test]
    fn frames_convert_both_ways() {
        let frame = NSRect::new(NSPoint::new(100.0, 50.0), NSSize::new(400.0, 300.0));
        let px = to_px(frame, 800.0, 2.0);
        assert_eq!(px, (200, 900, 800, 600));
        let back = from_px(px, 800.0, 2.0);
        assert_eq!(
            (
                back.origin.x,
                back.origin.y,
                back.size.width,
                back.size.height
            ),
            (100.0, 50.0, 400.0, 300.0)
        );
    }
}
