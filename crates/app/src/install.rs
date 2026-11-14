//! Telling Windows about Rogeha Reader: "Open with" for the file types it reads, the entry
//! on the Default apps page, the Start menu shortcut and the "Installed apps" entry.
//!
//! Everything is per user (the HKEY_CURRENT_USER part of the registry and the user's own
//! Start menu), so no administrator rights are needed. Windows does not let a program make
//! itself the default viewer; [`make_default`] registers the reader and opens the Default
//! apps page, where the user confirms the choice.
//!
//! Command line (used by the installer and by the uninstall entry):
//! `--register [--installed]`, `--unregister`, `--uninstall [--silent]`.
//!
//! For the automated check, `ROGEHA_REGISTRY_ROOT` moves every registry key under that key
//! and `ROGEHA_START_MENU` moves the shortcut, so a test never touches the real settings.

/// The file the installer leaves in the folder it created. Uninstall only ever removes a
/// folder that holds it.
pub const INSTALL_MARKER: &str = "installed-by-rogeha-reader-setup.txt";

#[cfg(not(windows))]
pub fn make_default() -> Result<String, String> {
    Err("Choosing the default viewer from inside the reader works on Windows only".into())
}

#[cfg(not(windows))]
pub fn command_line(_args: &[std::ffi::OsString]) -> Option<i32> {
    None
}

#[cfg(windows)]
pub use win::{command_line, make_default};

#[cfg(windows)]
mod win {
    use std::ffi::OsString;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
        RegDeleteKeyValueW, RegDeleteTreeW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};

    use super::INSTALL_MARKER;

    const APP: &str = "Rogeha Reader";
    const PROG_ID: &str = "RogehaReader.Document";
    const EXE: &str = "rogeha-reader.exe";
    const CAPABILITIES: &str = r"Software\Rogeha Studio\Rogeha Reader\Capabilities";
    const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\RogehaReader";
    /// What the reader opens. PDF is the one offered on the Default apps page first.
    const TYPES: [&str; 7] = [".pdf", ".epub", ".cbz", ".xps", ".oxps", ".fb2", ".mobi"];
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Where a key really goes: under the test root when one is set.
    fn key_path(path: &str) -> String {
        match std::env::var("ROGEHA_REGISTRY_ROOT") {
            Ok(root) if !root.trim().is_empty() => format!(r"{}\{path}", root.trim_end_matches('\\')),
            _ => path.to_owned(),
        }
    }

    fn testing() -> bool {
        std::env::var_os("ROGEHA_REGISTRY_ROOT").is_some()
    }

    enum Data<'a> {
        Text(&'a str),
        Number(u32),
    }

    /// Set a value under HKEY_CURRENT_USER, creating the key. `name = ""` is the default value.
    fn set(path: &str, name: &str, data: Data) -> Result<(), String> {
        let (path_w, name_w) = (wide(&key_path(path)), wide(name));
        // SAFETY: all strings are NUL-terminated and outlive the calls; the key is closed.
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            let opened = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path_w.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            );
            if opened != ERROR_SUCCESS {
                return Err(format!("Could not write the registry key {path} (error {opened})"));
            }
            let written = match data {
                Data::Text(text) => {
                    let text = wide(text);
                    RegSetValueExW(key, name_w.as_ptr(), 0, REG_SZ, text.as_ptr().cast(), (text.len() * 2) as u32)
                }
                Data::Number(n) => RegSetValueExW(key, name_w.as_ptr(), 0, REG_DWORD, (&n as *const u32).cast(), 4),
            };
            RegCloseKey(key);
            if written != ERROR_SUCCESS {
                return Err(format!("Could not write the registry value {path}\\{name} (error {written})"));
            }
        }
        Ok(())
    }

    fn delete_tree(path: &str) {
        let path = wide(&key_path(path));
        // SAFETY: NUL-terminated string; a missing key is simply an error code we ignore.
        unsafe {
            RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr());
        }
    }

    fn delete_value(path: &str, name: &str) {
        let (path, name) = (wide(&key_path(path)), wide(name));
        // SAFETY: as above.
        unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, path.as_ptr(), name.as_ptr());
        }
    }

    fn tell_the_shell() {
        if !testing() {
            // SAFETY: a notification without item pointers.
            unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
        }
    }

    fn shortcut_path() -> Option<PathBuf> {
        let folder = match std::env::var_os("ROGEHA_START_MENU") {
            Some(dir) => PathBuf::from(dir),
            None => PathBuf::from(std::env::var_os("APPDATA")?).join(r"Microsoft\Windows\Start Menu\Programs"),
        };
        Some(folder.join(format!("{APP}.lnk")))
    }

    /// Create the Start menu shortcut. Windows has no simple call for this, so the system's
    /// own script host object does it, through PowerShell.
    fn create_shortcut(exe: &Path) -> Result<(), String> {
        let link = shortcut_path().ok_or("The Start menu folder was not found")?;
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // Paths go in through the environment, so nothing in them is read as script.
        let script = "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:RR_LINK); \
                      $s.TargetPath = $env:RR_TARGET; $s.WorkingDirectory = $env:RR_DIR; \
                      $s.Description = 'Rogeha Reader'; $s.Save()";
        let status = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
            .env("RR_LINK", &link)
            .env("RR_TARGET", exe)
            .env("RR_DIR", exe.parent().unwrap_or(Path::new(".")))
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| format!("Could not create the Start menu shortcut: {e}"))?;
        if status.success() && link.exists() {
            Ok(())
        } else {
            Err("Could not create the Start menu shortcut".into())
        }
    }

    /// Register `exe` as a program that opens the reader's file types. With `installed`, also
    /// add the Start menu shortcut and the entry under Installed apps.
    pub fn register(exe: &Path, installed: bool) -> Result<(), String> {
        let exe_text = exe.to_string_lossy();
        let open = format!("\"{exe_text}\" \"%1\"");
        let classes = r"Software\Classes";

        // What a document opened with the reader is called, and how it is opened.
        set(&format!(r"{classes}\{PROG_ID}"), "", Data::Text("Document (Rogeha Reader)"))?;
        set(&format!(r"{classes}\{PROG_ID}\DefaultIcon"), "", Data::Text(&format!("\"{exe_text}\",0")))?;
        set(&format!(r"{classes}\{PROG_ID}\shell\open\command"), "", Data::Text(&open))?;
        // The program itself, for "Open with".
        set(&format!(r"{classes}\Applications\{EXE}"), "FriendlyAppName", Data::Text(APP))?;
        set(&format!(r"{classes}\Applications\{EXE}\shell\open\command"), "", Data::Text(&open))?;
        for ext in TYPES {
            set(&format!(r"{classes}\Applications\{EXE}\SupportedTypes"), ext, Data::Text(""))?;
            // Offer the reader for this type without taking the type away from anyone.
            set(&format!(r"{classes}\{ext}\OpenWithProgids"), PROG_ID, Data::Text(""))?;
            set(&format!(r"{CAPABILITIES}\FileAssociations"), ext, Data::Text(PROG_ID))?;
        }
        // The entry on the Default apps page.
        set(CAPABILITIES, "ApplicationName", Data::Text(APP))?;
        set(CAPABILITIES, "ApplicationDescription", Data::Text("A fast, free PDF and EPUB reader"))?;
        set(r"Software\RegisteredApplications", APP, Data::Text(CAPABILITIES))?;

        if installed {
            let folder = exe.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let version = crate::app::version_text().trim_start_matches("Version ").to_owned();
            set(UNINSTALL, "DisplayName", Data::Text(APP))?;
            set(UNINSTALL, "DisplayVersion", Data::Text(version.split(' ').next().unwrap_or_default()))?;
            set(UNINSTALL, "Publisher", Data::Text("Rogeha Studio"))?;
            set(UNINSTALL, "InstallLocation", Data::Text(&folder))?;
            set(UNINSTALL, "DisplayIcon", Data::Text(&exe_text))?;
            set(UNINSTALL, "UninstallString", Data::Text(&format!("\"{exe_text}\" --uninstall")))?;
            set(UNINSTALL, "URLInfoAbout", Data::Text("https://github.com/rogeha-boop/rogeha-reader"))?;
            set(UNINSTALL, "NoModify", Data::Number(1))?;
            set(UNINSTALL, "NoRepair", Data::Number(1))?;
            create_shortcut(exe)?;
        }
        tell_the_shell();
        Ok(())
    }

    /// Take back everything [`register`] wrote. The user's documents and the reader's own
    /// data (saved signatures, recent files, OCR languages) are left alone.
    pub fn unregister() {
        let classes = r"Software\Classes";
        delete_tree(&format!(r"{classes}\{PROG_ID}"));
        delete_tree(&format!(r"{classes}\Applications\{EXE}"));
        for ext in TYPES {
            delete_value(&format!(r"{classes}\{ext}\OpenWithProgids"), PROG_ID);
        }
        delete_tree(r"Software\Rogeha Studio\Rogeha Reader");
        delete_value(r"Software\RegisteredApplications", APP);
        delete_tree(UNINSTALL);
        if let Some(link) = shortcut_path() {
            let _ = std::fs::remove_file(link);
        }
        tell_the_shell();
    }

    /// Register this copy of the reader and open Windows' Default apps page on its entry.
    pub fn make_default() -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // An installed copy keeps its Installed apps entry; a portable copy only registers.
        register(&exe, exe.with_file_name(INSTALL_MARKER).exists())?;
        Command::new("explorer.exe")
            .arg("ms-settings:defaultapps?registeredAppUser=Rogeha%20Reader")
            .spawn()
            .map_err(|e| format!("Could not open the Default apps page: {e}"))?;
        Ok("Windows opened Default apps: choose Rogeha Reader for .pdf there. Not offered? Right-click a PDF > Open with > Choose an app on your PC".into())
    }

    fn uninstall(silent: bool) -> i32 {
        let Ok(exe) = std::env::current_exe() else { return 1 };
        let Some(folder) = exe.parent().map(Path::to_path_buf) else { return 1 };
        if !silent {
            let answer = rfd::MessageDialog::new()
                .set_title("Remove Rogeha Reader")
                .set_description("Remove Rogeha Reader from this computer?\n\nYour documents, saved signatures and settings are kept.")
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if answer != rfd::MessageDialogResult::Yes {
                return 0;
            }
        }
        unregister();
        // Only a folder the installer made is removed, and only after this program has ended
        // (a running program cannot delete itself).
        if folder.join(INSTALL_MARKER).exists() {
            let _ = Command::new("cmd.exe")
                // Passed as written: cmd has its own quoting rules.
                .raw_arg("/d /c \"ping -n 3 127.0.0.1 >nul & rmdir /s /q \"%RR_FOLDER%\"\"")
                .env("RR_FOLDER", &folder)
                .current_dir(std::env::temp_dir())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
        }
        if !silent {
            rfd::MessageDialog::new().set_title("Rogeha Reader").set_description("Rogeha Reader was removed.").show();
        }
        0
    }

    /// Handle the install-related command lines. `None` means "not one of ours".
    pub fn command_line(args: &[OsString]) -> Option<i32> {
        let has = |flag: &str| args.iter().any(|a| a == flag);
        if has("--register") {
            let result = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| register(&exe, has("--installed")));
            return Some(match result {
                Ok(()) => 0,
                Err(message) => {
                    if !has("--silent") {
                        rfd::MessageDialog::new().set_title("Rogeha Reader").set_description(message).show();
                    }
                    1
                }
            });
        }
        if has("--unregister") {
            unregister();
            return Some(0);
        }
        if has("--uninstall") {
            return Some(uninstall(has("--silent")));
        }
        None
    }
}
