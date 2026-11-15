//! One reader window: a document opened from File Explorer goes to a new tab of the reader that
//! is already open, instead of starting a second window.
//!
//! The first reader claims a named pipe for the current user and listens on it. A reader started
//! later with a file first tries to hand the file over through that pipe; when that works it
//! exits at once, before any window is made, and the first reader opens the file in a tab and
//! comes to the front. When nothing listens (no reader open, or it is busy elsewhere), the new
//! reader simply opens its own window and becomes the one that listens.
//!
//! Only a plain start takes part: scripted runs (`--script`, `--screenshot`, `--bench`) never
//! hand over and never listen. The pipe's name includes the data folder, so a reader started
//! with its own `ROGEHA_DATA_DIR` is separate from the everyday one.
//!
//! Windows only; elsewhere every start opens its own window as before.

use std::path::{Path, PathBuf};

/// Hand `file` to a reader that is already open. True when it took it.
pub fn hand_over(file: &Path) -> bool {
    #[cfg(windows)]
    {
        win::hand_over(file)
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        false
    }
}

/// Become the reader that receives documents. Returns a claim to pass to [`listen`], or `None`
/// when another reader already receives them (this one then works on its own).
pub fn claim() -> Option<Claim> {
    #[cfg(windows)]
    {
        win::claim()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The pipe this reader has claimed, waiting for a window to deliver documents to.
pub struct Claim {
    #[cfg(windows)]
    first: win::Pipe,
}

/// Listen on a background thread. Each document handed over arrives on the returned channel,
/// and `wake` is called so the window looks at it.
pub fn listen(claim: Claim, wake: impl Fn() + Send + 'static) -> std::sync::mpsc::Receiver<PathBuf> {
    let (tx, rx) = std::sync::mpsc::channel();
    #[cfg(windows)]
    std::thread::Builder::new()
        .name("open-requests".into())
        .spawn(move || win::serve(claim.first, tx, wake))
        .expect("could not start the thread that receives documents");
    #[cfg(not(windows))]
    let _ = (claim, tx, wake);
    rx
}

/// A short, stable name for this user and data folder (FNV-1a).
#[cfg_attr(not(windows), allow(dead_code))]
fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in user.bytes().chain(crate::app::app_data_dir().to_string_lossy().to_lowercase().bytes()) {
        hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!(r"\\.\pipe\RogehaReader-{hash:016x}")
}

#[cfg(windows)]
mod win {
    use std::ffi::OsString;
    use std::io::{Read, Write};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, ERROR_PIPE_CONNECTED};
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

    use super::Claim;

    /// One instance of the named pipe.
    pub struct Pipe(OwnedHandle);

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn create(first: bool) -> Option<Pipe> {
        let name = wide(&super::pipe_name());
        let flags = PIPE_ACCESS_INBOUND | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        // SAFETY: a NUL-terminated name and plain flags; the handle is checked before use and
        // owned by the returned value, which closes it.
        unsafe {
            let handle = CreateNamedPipeW(
                name.as_ptr(),
                flags,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                0,
                64 * 1024,
                0,
                std::ptr::null(),
            );
            (handle != INVALID_HANDLE_VALUE).then(|| Pipe(OwnedHandle::from_raw_handle(handle)))
        }
    }

    pub fn claim() -> Option<Claim> {
        // Only the first instance of the name succeeds with this flag: that is the claim.
        create(true).map(|first| Claim { first })
    }

    pub fn hand_over(file: &Path) -> bool {
        let file = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
        // The path as UTF-16, the way Windows keeps it, so no file name is mangled.
        let bytes: Vec<u8> = file.as_os_str().encode_wide().flat_map(u16::to_le_bytes).collect();
        // A reader that is just between two requests has no free instance for a moment.
        for _ in 0..20 {
            match std::fs::OpenOptions::new().write(true).open(super::pipe_name()) {
                Ok(mut pipe) => {
                    // Let the open reader come to the front: this process was started by the
                    // user, so it may pass that right on.
                    // SAFETY: a plain call without pointers.
                    unsafe {
                        AllowSetForegroundWindow(ASFW_ANY);
                    }
                    return pipe.write_all(&bytes).is_ok();
                }
                // 231 = ERROR_PIPE_BUSY: wait a little and try again.
                Err(e) if e.raw_os_error() == Some(231) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(_) => return false, // no reader is listening
            }
        }
        false
    }

    pub fn serve(first: Pipe, tx: std::sync::mpsc::Sender<PathBuf>, wake: impl Fn()) {
        let mut waiting = first;
        loop {
            // SAFETY: a valid pipe handle; no overlapped I/O.
            let connected = unsafe {
                use std::os::windows::io::AsRawHandle;
                ConnectNamedPipe(waiting.0.as_raw_handle(), std::ptr::null_mut()) != 0
                    || windows_sys::Win32::Foundation::GetLastError() == ERROR_PIPE_CONNECTED
            };
            // Have the next instance ready before reading, so a second document handed over
            // right after this one finds the reader listening.
            let Some(next) = create(false) else { return };
            let current = std::mem::replace(&mut waiting, next);
            if !connected {
                continue;
            }
            let mut bytes = Vec::new();
            let mut reader = std::fs::File::from(current.0);
            if reader.read_to_end(&mut bytes).is_err() || bytes.is_empty() || bytes.len() % 2 != 0 {
                continue;
            }
            let units: Vec<u16> = bytes.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
            let path = PathBuf::from(OsString::from_wide(&units));
            if tx.send(path).is_err() {
                return; // the window has closed
            }
            wake();
        }
    }
}
