//! Fonts installed on the computer.
//!
//! MuPDF asks for a font in three situations: a PDF names a font it does not carry
//! ("Calibri-Bold"), a PDF uses a Chinese, Japanese or Korean font it does not carry, or some
//! text (mostly in EPUBs) needs a character that none of the fonts at hand has. Without an
//! answer it falls back to its few built-in fonts, and text in other scripts stays empty.
//!
//! mupdf-rs can answer through the `font-kit` crate, but that hangs on Windows. This is a small
//! replacement for Windows only: font names come from the registry's list of installed fonts,
//! and each script has a font that ships with Windows. Nothing is read until MuPDF first asks,
//! so starting the reader and opening an ordinary PDF cost nothing extra.
//!
//! Set `ROGEHA_NO_SYSTEM_FONTS=1` to switch this off.

/// Tell MuPDF where to find installed fonts. Safe to call often; only the first call counts.
pub fn install() {
    #[cfg(windows)]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            if std::env::var_os("ROGEHA_NO_SYSTEM_FONTS").is_none() {
                mupdf::set_font_loader(win::WindowsFonts);
            }
        });
    }
}

/// What a font name in a PDF asks for: the family in lower-case letters and digits only,
/// extra style words such as "light", and whether it is bold or italic.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, PartialEq)]
struct Wanted {
    family: String,
    extra: String,
    bold: bool,
    italic: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
fn squash(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Take a PDF font name apart: "ABCDEF+TimesNewRomanPS-BoldItalicMT" is the family
/// "timesnewroman", bold and italic.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_name(name: &str, bold: bool, italic: bool) -> Wanted {
    // A subset prefix is six capital letters and a plus sign.
    let name = match name.split_once('+') {
        Some((prefix, rest)) if prefix.len() == 6 && prefix.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => name,
    };
    let (family, style) = match name.find(['-', ',']) {
        Some(at) => (&name[..at], &name[at + 1..]),
        None => (name, ""),
    };
    let family = family.strip_suffix("MT").unwrap_or(family);
    let family = family.strip_suffix("PS").unwrap_or(family);
    let mut style = squash(style);
    for ending in ["mt", "ps"] {
        if let Some(rest) = style.strip_suffix(ending) {
            style = rest.to_owned();
        }
    }
    let mut wanted = Wanted { family: squash(family), extra: String::new(), bold, italic };
    for (word, is_bold, is_italic) in
        [("bold", true, false), ("italic", false, true), ("oblique", false, true), ("regular", false, false), ("roman", false, false), ("normal", false, false)]
    {
        if style.contains(word) {
            style = style.replace(word, "");
            wanted.bold |= is_bold;
            wanted.italic |= is_italic;
        }
    }
    wanted.extra = style;
    wanted
}

/// The names to look for among the installed fonts, best match first.
#[cfg_attr(not(windows), allow(dead_code))]
fn candidates(wanted: &Wanted, exact: bool) -> Vec<String> {
    let base = format!("{}{}", wanted.family, wanted.extra);
    let styled = |base: &str, bold: bool, italic: &str| format!("{base}{}{italic}", if bold { "bold" } else { "" });
    let mut list = Vec::new();
    let italics: &[&str] = if wanted.italic { &["italic", "oblique"] } else { &[""] };
    for italic in italics {
        list.push(styled(&base, wanted.bold, italic));
    }
    if !exact {
        // A plainer face of the same family is still much closer than a built-in stand-in.
        if wanted.italic && wanted.bold {
            list.push(styled(&base, true, ""));
        }
        list.push(base.clone());
        if !wanted.extra.is_empty() {
            list.push(wanted.family.clone());
        }
    }
    list.dedup();
    list
}

/// The 14 standard PDF fonts are built in, with the exact widths PDFs expect.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_standard(family: &str) -> bool {
    matches!(family, "helvetica" | "times" | "courier" | "symbol" | "zapfdingbats")
}

/// Split a registry entry such as "Cambria & Cambria Math (TrueType)" into its face names.
#[cfg_attr(not(windows), allow(dead_code))]
fn faces(entry: &str) -> Vec<String> {
    let entry = match entry.rfind(" (") {
        Some(at) if entry.ends_with(')') => &entry[..at],
        _ => entry,
    };
    entry.split(" & ").map(squash).collect()
}

#[cfg(windows)]
mod win {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    use mupdf::{CjkFontOrdering, Font, FontHints, FontLoader};
    use mupdf_sys::*;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_READ, MapViewOfFile, PAGE_READONLY};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RegCloseKey, RegEnumValueW, RegOpenKeyExW,
    };

    use super::{candidates, faces, is_standard, parse_name};

    pub struct WindowsFonts;

    const _: () = assert!(std::mem::size_of::<Font>() == std::mem::size_of::<*mut fz_font>());

    fn fonts_folder() -> PathBuf {
        let windows = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir"));
        PathBuf::from(windows.unwrap_or_else(|| "C:\\Windows".into())).join("Fonts")
    }

    /// Installed fonts by face name: file and face number within the file.
    fn installed() -> &'static HashMap<String, (PathBuf, i32)> {
        static LIST: OnceLock<HashMap<String, (PathBuf, i32)>> = OnceLock::new();
        LIST.get_or_init(|| {
            let folder = fonts_folder();
            let mut list = HashMap::new();
            // Fonts for this user only come second, so they win over system fonts of the same name.
            for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
                for (entry, file) in registry_fonts(root) {
                    let lower = file.to_lowercase();
                    if !(lower.ends_with(".ttf") || lower.ends_with(".otf") || lower.ends_with(".ttc")) {
                        continue;
                    }
                    let path = if Path::new(&file).is_absolute() { PathBuf::from(&file) } else { folder.join(&file) };
                    for (index, face) in faces(&entry).into_iter().enumerate() {
                        // "Yu Gothic Regular" is also plain "Yu Gothic".
                        if let Some(plain) = face.strip_suffix("regular") {
                            list.entry(plain.to_owned()).or_insert_with(|| (path.clone(), index as i32));
                        }
                        list.insert(face, (path.clone(), index as i32));
                    }
                }
            }
            list
        })
    }

    fn registry_fonts(root: HKEY) -> Vec<(String, String)> {
        let key_name: Vec<u16> =
            "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Fonts".encode_utf16().chain(std::iter::once(0)).collect();
        let mut found = Vec::new();
        // SAFETY: plain registry reads; every buffer is sized as passed and outlives its call.
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            if RegOpenKeyExW(root, key_name.as_ptr(), 0, KEY_READ, &mut key) != ERROR_SUCCESS {
                return found;
            }
            let mut name = [0u16; 512];
            let mut data = [0u16; 1024];
            for index in 0.. {
                let mut name_len = name.len() as u32;
                let mut data_len = std::mem::size_of_val(&data) as u32;
                let mut kind = 0u32;
                let status = RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    &mut kind,
                    data.as_mut_ptr().cast(),
                    &mut data_len,
                );
                if status != ERROR_SUCCESS {
                    // The end of the list, or an entry too long to be a font we could use.
                    if status == windows_sys::Win32::Foundation::ERROR_MORE_DATA {
                        continue;
                    }
                    break;
                }
                if kind != REG_SZ {
                    continue;
                }
                let value = &data[..(data_len as usize / 2).min(data.len())];
                let value = String::from_utf16_lossy(value);
                found.push((String::from_utf16_lossy(&name[..name_len as usize]), value.trim_end_matches('\0').to_owned()));
            }
            RegCloseKey(key);
        }
        found
    }

    /// The bytes of a font file, for the rest of the program's life. The file is mapped, not
    /// read: Windows then brings in only the parts MuPDF really looks at, and those are shared
    /// with every other program using the font. Reading a Chinese font instead would cost
    /// 20 MB of memory for a few characters.
    fn map(path: &Path) -> Option<&'static [u8]> {
        use std::os::windows::io::AsRawHandle;
        let file = std::fs::File::open(path).ok()?;
        let len = usize::try_from(file.metadata().ok()?.len()).ok().filter(|len| *len > 0)?;
        // SAFETY: a read-only mapping of an open file. The mapping and the view are never
        // released, so the slice stays valid; font files in use cannot be changed on Windows
        // while they are mapped.
        unsafe {
            let mapping = CreateFileMappingW(file.as_raw_handle().cast(), std::ptr::null(), PAGE_READONLY, 0, 0, std::ptr::null());
            if mapping.is_null() {
                return None;
            }
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
            (!view.Value.is_null()).then(|| std::slice::from_raw_parts(view.Value.cast::<u8>(), len))
        }
    }

    /// Load a font file once and hand out further references to the same font afterwards:
    /// MuPDF asks again for every mix of bold, italic and serif.
    fn load(path: &Path, index: i32) -> Option<Font> {
        static LOADED: Mutex<Option<HashMap<(PathBuf, i32), usize>>> = Mutex::new(None);
        let mut loaded = LOADED.lock().ok()?;
        let loaded = loaded.get_or_insert_with(HashMap::new);
        let key = (path.to_path_buf(), index);
        let raw = match loaded.get(&key) {
            Some(&raw) => raw as *mut fz_font,
            None => {
                let data = map(path)?;
                let name = std::ffi::CString::new(path.file_stem()?.to_string_lossy().as_bytes()).ok()?;
                let mut error: *mut mupdf_error_t = std::ptr::null_mut();
                // SAFETY: valid context, a NUL-terminated name, and bytes that live for ever
                // (MuPDF keeps pointing into them). A failure comes back through `error`.
                let raw = unsafe {
                    let raw = mupdf_new_font_from_memory(
                        crate::engine::raw_context(),
                        name.as_ptr(),
                        index,
                        data.as_ptr(),
                        i32::try_from(data.len()).ok()?,
                        &mut error,
                    );
                    if !error.is_null() {
                        mupdf_drop_error(error);
                        return None;
                    }
                    raw
                };
                if raw.is_null() {
                    return None;
                }
                // The reference we got is kept for the life of the program.
                loaded.insert(key, raw as usize);
                raw
            }
        };
        // SAFETY: `raw` is a live font (we never drop our reference); the caller gets its own
        // reference, which the `Font` gives back when dropped. `Font` is a struct with a
        // single `*mut fz_font` field (size checked above).
        unsafe {
            fz_keep_font(crate::engine::raw_context(), raw);
            Some(std::mem::transmute::<*mut fz_font, Font>(raw))
        }
    }

    /// The first of these Windows font files that exists.
    fn first_of(files: &[(&str, i32)]) -> Option<Font> {
        let folder = fonts_folder();
        files.iter().find_map(|(file, index)| {
            let path = folder.join(file);
            path.exists().then(|| load(&path, *index)).flatten()
        })
    }

    // Fonts for Chinese, Japanese and Korean that come with Windows (some only when the
    // language is installed, hence several per language).
    const JAPANESE: &[(&str, i32)] = &[("YuGothR.ttc", 0), ("msgothic.ttc", 0), ("meiryo.ttc", 0)];
    const JAPANESE_SERIF: &[(&str, i32)] = &[("yumin.ttf", 0), ("msmincho.ttc", 0), ("YuGothR.ttc", 0), ("msgothic.ttc", 0)];
    const KOREAN: &[(&str, i32)] = &[("malgun.ttf", 0), ("gulim.ttc", 0)];
    const KOREAN_SERIF: &[(&str, i32)] = &[("batang.ttc", 0), ("malgun.ttf", 0)];
    const SIMPLIFIED: &[(&str, i32)] = &[("msyh.ttc", 0), ("simsun.ttc", 0), ("simhei.ttf", 0)];
    const SIMPLIFIED_SERIF: &[(&str, i32)] = &[("simsun.ttc", 0), ("msyh.ttc", 0)];
    const TRADITIONAL: &[(&str, i32)] = &[("msjh.ttc", 0), ("mingliu.ttc", 0), ("msyh.ttc", 0)];
    const TRADITIONAL_SERIF: &[(&str, i32)] = &[("mingliu.ttc", 0), ("msjh.ttc", 0), ("simsun.ttc", 0)];

    impl FontLoader for WindowsFonts {
        fn load_font(&self, name: &str, hints: FontHints) -> Option<Font> {
            let wanted = parse_name(name, hints.bold, hints.italic);
            if wanted.family.is_empty() || is_standard(&wanted.family) {
                return None;
            }
            let fonts = installed();
            // MuPDF's last resorts for symbols and emoji ask for Noto fonts by name. Use them when
            // they are installed, and otherwise the fonts Windows has for the same job.
            let stand_in: &[(&str, i32)] = match wanted.family.as_str() {
                _ if fonts.contains_key(&wanted.family) => &[],
                "notosanssymbols" | "notosanssymbols2" | "notomusic" => &[("seguisym.ttf", 0)],
                "notoemoji" => &[("seguiemj.ttf", 0)],
                "notosansmath" => &[("cambria.ttc", 1), ("seguisym.ttf", 0)],
                _ => &[],
            };
            if !stand_in.is_empty() {
                return first_of(stand_in);
            }
            candidates(&wanted, hints.needs_exact_metrics)
                .iter()
                .find_map(|name| fonts.get(name))
                .and_then(|(path, index)| load(path, *index))
        }

        fn load_cjk_font(&self, _name: &str, ordering: CjkFontOrdering, serif: bool) -> Option<Font> {
            first_of(match (ordering, serif) {
                (CjkFontOrdering::AdobeJapan, false) => JAPANESE,
                (CjkFontOrdering::AdobeJapan, true) => JAPANESE_SERIF,
                (CjkFontOrdering::AdobeKorea, false) => KOREAN,
                (CjkFontOrdering::AdobeKorea, true) => KOREAN_SERIF,
                (CjkFontOrdering::AdobeGb, false) => SIMPLIFIED,
                (CjkFontOrdering::AdobeGb, true) => SIMPLIFIED_SERIF,
                (CjkFontOrdering::AdobeCns, false) => TRADITIONAL,
                (CjkFontOrdering::AdobeCns, true) => TRADITIONAL_SERIF,
            })
        }

        #[allow(non_upper_case_globals)] // MuPDF spells its language constants this way
        fn load_fallback_font(&self, script: u32, language: u32, hints: FontHints) -> Option<Font> {
            let text = |regular: &'static str, bold: &'static str, italic: &'static str, both: &'static str| {
                let file = match (hints.bold, hints.italic) {
                    (false, false) => regular,
                    (true, false) => bold,
                    (false, true) => italic,
                    (true, true) => both,
                };
                first_of(&[(file, 0), (regular, 0)])
            };
            match script {
                UCDN_SCRIPT_HAN | UCDN_SCRIPT_BOPOMOFO => first_of(match (language as fz_text_language, hints.serif) {
                    (FZ_LANG_ja, false) => JAPANESE,
                    (FZ_LANG_ja, true) => JAPANESE_SERIF,
                    (FZ_LANG_ko, false) => KOREAN,
                    (FZ_LANG_ko, true) => KOREAN_SERIF,
                    (FZ_LANG_zh_Hant, false) => TRADITIONAL,
                    (FZ_LANG_zh_Hant, true) => TRADITIONAL_SERIF,
                    (_, false) => SIMPLIFIED,
                    (_, true) => SIMPLIFIED_SERIF,
                }),
                UCDN_SCRIPT_HIRAGANA | UCDN_SCRIPT_KATAKANA => first_of(if hints.serif { JAPANESE_SERIF } else { JAPANESE }),
                UCDN_SCRIPT_HANGUL => first_of(if hints.serif { KOREAN_SERIF } else { KOREAN }),
                UCDN_SCRIPT_LATIN | UCDN_SCRIPT_GREEK | UCDN_SCRIPT_CYRILLIC | UCDN_SCRIPT_HEBREW | UCDN_SCRIPT_ARABIC
                    if hints.serif =>
                {
                    text("times.ttf", "timesbd.ttf", "timesi.ttf", "timesbi.ttf")
                }
                UCDN_SCRIPT_LATIN
                | UCDN_SCRIPT_GREEK
                | UCDN_SCRIPT_CYRILLIC
                | UCDN_SCRIPT_HEBREW
                | UCDN_SCRIPT_ARABIC
                | UCDN_SCRIPT_ARMENIAN
                | UCDN_SCRIPT_GEORGIAN => text("segoeui.ttf", "segoeuib.ttf", "segoeuii.ttf", "segoeuiz.ttf"),
                UCDN_SCRIPT_THAI | UCDN_SCRIPT_LAO | UCDN_SCRIPT_KHMER => first_of(&[("LeelawUI.ttf", 0), ("tahoma.ttf", 0)]),
                // Devanagari, Bengali, Gurmukhi, Gujarati, Oriya, Tamil, Telugu, Kannada,
                // Malayalam and Sinhala are numbered in one run.
                UCDN_SCRIPT_DEVANAGARI..=UCDN_SCRIPT_SINHALA => first_of(&[("Nirmala.ttc", 0), ("Nirmala.ttf", 0)]),
                UCDN_SCRIPT_TIBETAN => first_of(&[("himalaya.ttf", 0)]),
                UCDN_SCRIPT_MYANMAR => first_of(&[("mmrtext.ttf", 0)]),
                UCDN_SCRIPT_ETHIOPIC => first_of(&[("ebrima.ttf", 0)]),
                UCDN_SCRIPT_CHEROKEE => first_of(&[("gadugi.ttf", 0)]),
                UCDN_SCRIPT_MONGOLIAN => first_of(&[("monbaiti.ttf", 0)]),
                UCDN_SCRIPT_YI => first_of(&[("msyi.ttf", 0)]),
                UCDN_SCRIPT_THAANA => first_of(&[("mvboli.ttf", 0)]),
                UCDN_SCRIPT_SYRIAC => first_of(&[("estre.ttf", 0)]),
                // Arrows, symbols, dingbats and emoji belong to no script.
                UCDN_SCRIPT_COMMON => first_of(&[("seguisym.ttf", 0), ("seguiemj.ttf", 0)]),
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(family: &str, extra: &str, bold: bool, italic: bool) -> Wanted {
        Wanted { family: family.into(), extra: extra.into(), bold, italic }
    }

    #[test]
    fn pdf_font_names_are_taken_apart() {
        assert_eq!(parse_name("ArialMT", false, false), wanted("arial", "", false, false));
        assert_eq!(parse_name("Arial-BoldMT", false, false), wanted("arial", "", true, false));
        assert_eq!(parse_name("ABCDEF+TimesNewRomanPS-BoldItalicMT", false, false), wanted("timesnewroman", "", true, true));
        assert_eq!(parse_name("Calibri,Bold", false, false), wanted("calibri", "", true, false));
        assert_eq!(parse_name("Calibri-Light", false, true), wanted("calibri", "light", false, true));
        assert_eq!(parse_name("Segoe UI", true, false), wanted("segoeui", "", true, false));
        assert_eq!(parse_name("Helvetica-Oblique", false, false), wanted("helvetica", "", false, true));
        // Not a subset prefix: the plus sign is part of the name.
        assert_eq!(parse_name("A+B", false, false), wanted("ab", "", false, false));
    }

    #[test]
    fn the_closest_installed_face_is_tried_first() {
        assert_eq!(candidates(&wanted("arial", "", true, true), false), ["arialbolditalic", "arialboldoblique", "arialbold", "arial"]);
        assert_eq!(candidates(&wanted("calibri", "light", false, false), false), ["calibrilight", "calibri"]);
        // When the widths must be right, only the exact face will do.
        assert_eq!(candidates(&wanted("arial", "", true, false), true), ["arialbold"]);
    }

    #[test]
    fn registry_entries_are_split_into_faces() {
        assert_eq!(faces("Arial Bold Italic (TrueType)"), ["arialbolditalic"]);
        assert_eq!(faces("Cambria & Cambria Math (TrueType)"), ["cambria", "cambriamath"]);
        assert_eq!(faces("Yu Gothic Regular & Yu Gothic UI Semilight (TrueType)"), ["yugothicregular", "yugothicuisemilight"]);
    }

    #[test]
    fn the_standard_fonts_stay_built_in() {
        assert!(is_standard(&parse_name("Helvetica-Bold", false, false).family));
        assert!(is_standard(&parse_name("Times-Roman", false, false).family));
        assert!(!is_standard(&parse_name("TimesNewRomanPSMT", false, false).family));
    }
}
