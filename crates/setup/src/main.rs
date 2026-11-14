//! The Rogeha Reader installer: one file that carries the reader and its assistant.
//!
//! It installs for the current user only, into `%LOCALAPPDATA%\Programs\Rogeha Reader`, so
//! it never asks for administrator rights. It copies the two programs, lets the reader
//! register itself with Windows (Start menu, "Open with", Installed apps), and offers to
//! open the Default apps page, because Windows only lets the user choose a default viewer.
//!
//! `--silent` installs without questions and without starting the reader;
//! `--dir <folder>` installs somewhere else.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("This installer is for Windows. On other systems, run the rogeha-reader program directly.");
}

#[cfg(windows)]
fn main() {
    std::process::exit(win::run());
}

#[cfg(windows)]
mod win {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO, MessageBoxW,
    };

    const READER: &[u8] = include_bytes!(env!("ROGEHA_READER_EXE"));
    const AGENT: &[u8] = include_bytes!(env!("ROGEHA_AGENT_EXE"));
    /// Must match `INSTALL_MARKER` in the reader's install.rs.
    const MARKER: &str = "installed-by-rogeha-reader-setup.txt";
    const TITLE: &str = "Rogeha Reader Setup";

    fn version() -> String {
        format!("{}.{}.{}", env!("CARGO_PKG_VERSION_MAJOR"), env!("CARGO_PKG_VERSION_MINOR"), env!("ROGEHA_BUILD_NUMBER"))
    }

    fn message(text: &str, style: u32) -> i32 {
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (text, title) = (wide(text), wide(TITLE));
        // SAFETY: both strings are NUL-terminated and outlive the call.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), style) }
    }

    fn default_folder() -> Option<PathBuf> {
        Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Programs").join("Rogeha Reader"))
    }

    fn install(folder: &Path) -> Result<PathBuf, String> {
        std::fs::create_dir_all(folder).map_err(|e| format!("Could not create {}: {e}", folder.display()))?;
        let reader = folder.join("rogeha-reader.exe");
        for (path, bytes) in [(&reader, READER), (&folder.join("rogeha-agent.exe"), AGENT)] {
            std::fs::write(path, bytes).map_err(|e| {
                format!("Could not write {}: {e}\n\nIf Rogeha Reader is open, close it and run the installer again.", path.display())
            })?;
        }
        std::fs::write(folder.join(MARKER), format!("Rogeha Reader {} was installed here by its installer.\r\n", version()))
            .map_err(|e| e.to_string())?;
        // The reader knows how to register itself; the same code backs its uninstall entry.
        let status = Command::new(&reader)
            .args(["--register", "--installed", "--silent"])
            .status()
            .map_err(|e| format!("Could not start the installed reader: {e}"))?;
        if !status.success() {
            return Err("The reader was copied, but could not be registered with Windows.".into());
        }
        Ok(reader)
    }

    pub fn run() -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let silent = args.iter().any(|a| a == "--silent");
        let folder = args
            .iter()
            .position(|a| a == "--dir")
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
            .or_else(default_folder);
        let Some(folder) = folder else {
            if !silent {
                message("The folder for programs was not found on this computer.", MB_OK | MB_ICONERROR);
            }
            return 1;
        };

        if !silent {
            let question = format!(
                "Install Rogeha Reader {} for your user account?\n\nIt will be installed in:\n{}\n\nNo administrator rights are needed.",
                version(),
                folder.display()
            );
            if message(&question, MB_YESNO | MB_ICONQUESTION) != IDYES {
                return 0;
            }
        }
        let reader = match install(&folder) {
            Ok(reader) => reader,
            Err(error) => {
                if !silent {
                    message(&error, MB_OK | MB_ICONERROR);
                }
                return 1;
            }
        };
        if silent {
            return 0;
        }
        let question = format!("Rogeha Reader is installed. You will find it in the Start menu.\n\n\
                        Make it your default PDF viewer?\n\n\
                        Windows will open its Default apps page. Choose Rogeha Reader for .pdf there.\n\n\
                        If Windows does not offer Rogeha Reader: right-click a PDF, choose\n\
                        Open with > Choose another app > Choose an app on your PC,\n\
                        pick rogeha-reader.exe in
{}
\n                        and confirm with Always.", folder.display());
        if message(&question, MB_YESNO | MB_ICONINFORMATION) == IDYES {
            let _ = Command::new("explorer.exe").arg("ms-settings:defaultapps?registeredAppUser=Rogeha%20Reader").spawn();
        }
        let _ = Command::new(&reader).spawn();
        0
    }
}
