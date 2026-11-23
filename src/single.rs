//! One Visiblement at a time: a second launch (a media opened from Explorer while the app is open)
//! hands its files to the running one through a named pipe, then quits. The running app adds them
//! to its playlist.

use std::ffi::c_void;
use std::io::{Read, Write};
use std::os::windows::io::FromRawHandle;
use std::path::PathBuf;
use std::time::{Duration, Instant};

type Handle = *mut c_void;
const INVALID_HANDLE: Handle = -1isize as Handle;
const PIPE_ACCESS_INBOUND: u32 = 0x0000_0001;
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
const ERROR_PIPE_CONNECTED: i32 = 535;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateNamedPipeW(name: *const u16, open: u32, mode: u32, max: u32, out: u32, inb: u32, timeout: u32, sa: *mut c_void) -> Handle;
    fn ConnectNamedPipe(pipe: Handle, overlapped: *mut c_void) -> i32;
    fn GetLastError() -> u32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn AllowSetForegroundWindow(pid: u32) -> i32;
}

/// Per user, so two accounts on one PC keep their own app.
fn pipe_name() -> String {
    format!(r"\\.\pipe\visiblement-{}", std::env::var("USERNAME").unwrap_or_default())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The pipe of the running app, created first (None when another app already owns it).
fn create(first: bool) -> Option<Handle> {
    let flags = PIPE_ACCESS_INBOUND | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    // SAFETY: a byte-mode inbound pipe, any number of instances (255), default security.
    let h = unsafe { CreateNamedPipeW(wide(&pipe_name()).as_ptr(), flags, 0, 255, 0, 64 * 1024, 0, std::ptr::null_mut()) };
    (h != INVALID_HANDLE).then_some(h)
}

/// Sends `paths` to the running app and quits. When there is none, this launch is the app: it gets
/// the pipe to listen on (None if the name is unavailable: it then runs on its own). Two launches
/// at once (several files opened together): the one that loses the pipe sends to the other.
pub fn forward(paths: &[PathBuf]) -> Option<Handle> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(mut pipe) = std::fs::OpenOptions::new().write(true).open(pipe_name()) {
            // Windows only lets the running app come to the front if this one (in front) allows it.
            // SAFETY: ASFW_ANY.
            unsafe { AllowSetForegroundWindow(u32::MAX) };
            let list: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            let _ = pipe.write_all(serde_json::to_string(&list).unwrap_or_default().as_bytes());
            std::process::exit(0);
        }
        if let Some(h) = create(true) {
            return Some(h); // first one: this launch is the app
        }
        if Instant::now() > deadline {
            return None; // something else holds the name: run on our own
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Listens on `first` (from `forward`) and every next instance of the pipe; each client's files
/// go to `on_open`.
pub fn serve(first: Handle, on_open: impl Fn(Vec<PathBuf>) + Send + 'static) {
    let first = first as usize; // a handle moves across threads as a number
    std::thread::spawn(move || {
        let mut h = first as Handle;
        loop {
            // SAFETY: a pipe handle of ours; a client that connected before the call is fine too.
            let ok = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } as i32 == ERROR_PIPE_CONNECTED;
            // SAFETY: the File owns and closes the handle.
            let mut file = unsafe { std::fs::File::from_raw_handle(h) };
            let mut body = String::new();
            if ok && file.read_to_string(&mut body).is_ok() {
                let paths: Vec<String> = serde_json::from_str(&body).unwrap_or_default();
                on_open(paths.into_iter().map(PathBuf::from).collect());
            }
            drop(file);
            match create(false) {
                Some(next) => h = next,
                None => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_name_is_per_user() {
        assert!(pipe_name().starts_with(r"\\.\pipe\visiblement-"));
    }
}
