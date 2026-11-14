//! Host side of the AI agent.
//!
//! The agent lives in a separate executable (`rogeha-agent`). Nothing here runs until the
//! user opens the AI panel, so the agent costs nothing at startup or while reading.
//! Protocol: one JSON object per line over the child's stdin/stdout.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

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
