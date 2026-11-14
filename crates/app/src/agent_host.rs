//! Host side of the AI agent.
//!
//! The agent lives in a separate executable (`rogeha-agent`). Nothing here runs until the
//! user opens the AI panel, so the agent costs nothing at startup or while reading.
//! Protocol: one JSON object per line over the child's stdin/stdout.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// Call right before starting any helper program.
///
/// On Windows a child process inherits every inheritable handle of its parent, and the C
/// library MuPDF uses opens files inheritably. A helper (the AI agent lives as long as its
/// panel) would then keep the open PDF locked, and saving, which replaces the file, would
/// fail. This marks all of this process's disk-file handles as not inheritable; the pipes
/// the helper needs are set up by the standard library afterwards and are not affected.
pub fn keep_files_out_of_helpers() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileType(handle: isize) -> u32;
            fn SetHandleInformation(handle: isize, mask: u32, flags: u32) -> i32;
        }
        const FILE_TYPE_DISK: u32 = 1;
        const HANDLE_FLAG_INHERIT: u32 = 1;
        // Handle values are small multiples of four; asking about one that is not open is harmless.
        for handle in (4..=0x10000isize).step_by(4) {
            // SAFETY: both calls only inspect or change flags of handles owned by this process.
            unsafe {
                if GetFileType(handle) == FILE_TYPE_DISK {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }
}

pub struct AgentHost {
    child: Child,
    stdin: ChildStdin,
    pub lines: Receiver<String>,
}

impl AgentHost {
    /// Start the agent process. `wake` is called whenever a line arrives.
    pub fn start(wake: impl Fn() + Send + 'static) -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let name = if cfg!(windows) { "rogeha-agent.exe" } else { "rogeha-agent" };
        let agent = exe.with_file_name(name);
        if !agent.exists() {
            return Err(format!("{name} was not found next to the reader"));
        }
        let mut cmd = Command::new(agent);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        keep_files_out_of_helpers();
        let mut child = cmd.spawn().map_err(|e| format!("Could not start the AI agent: {e}"))?;
        let stdin = child.stdin.take().ok_or("agent has no stdin")?;
        let stdout = child.stdout.take().ok_or("agent has no stdout")?;
        let (tx, lines) = mpsc::channel();
        thread::Builder::new()
            .name("rr-agent-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if tx.send(line).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { child, stdin, lines })
    }

    pub fn send_line(&mut self, json: &str) -> Result<(), String> {
        writeln!(self.stdin, "{json}").map_err(|e| e.to_string())?;
        self.stdin.flush().map_err(|e| e.to_string())
    }
}

impl Drop for AgentHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
