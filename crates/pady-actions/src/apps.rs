//! The apps installed on this computer, for the app's "Choose an app" menu.
//!
//! * **Windows**: what Start shows. Everything in the shell's Apps folder,
//!   which holds classic programs (through their Start menu shortcuts) and
//!   packaged apps (Microsoft Store, MSIX) alike. A packaged app has no file
//!   to point at: its program sits in a protected folder whose name changes
//!   with every update. What stays the same is its id, launched as
//!   `shell:AppsFolder\<id>`, so that is what a pick stores.
//! * **Linux**: the desktop entries (`*.desktop`) desktops build their app
//!   menus from. A pick stores the entry's id, which `launch` hands to
//!   `gtk-launch` when it isn't a program on PATH.

use serde::Serialize;

/// One app as the menu shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InstalledApp {
    /// The name people know it by ("Claude", "Notepad").
    pub name: String,
    /// What a `launch` action's `program` should hold to start it.
    pub program: String,
    /// The app's icon as a `data:image/png;base64,` address, where there is one.
    pub icon: Option<String>,
}

/// Every installed app, by name. Empty where the OS has no list to offer.
pub fn installed_apps() -> Vec<InstalledApp> {
    #[cfg(windows)]
    let mut apps = windows::installed_apps();
    #[cfg(target_os = "linux")]
    let mut apps = linux::installed_apps();
    #[cfg(not(any(windows, target_os = "linux")))]
    let mut apps: Vec<InstalledApp> = Vec::new();
    // Names and ids come from whatever is installed: keep them to what a
    // config may hold, and one line each.
    apps.retain(|a| {
        let fits = |s: &str| !s.trim().is_empty() && s.len() <= pady_core::Config::MAX_TEXT;
        fits(&a.name) && fits(&a.program) && !a.name.chars().chain(a.program.chars()).any(char::is_control)
    });
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.program.cmp(&b.program)));
    apps.dedup_by(|a, b| a.name == b.name && a.program == b.program);
    apps
}

/// Bytes as standard base64, for the icons' data addresses.
#[cfg(any(windows, test))]
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> shift & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(any(windows, test))]
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { crc >> 1 ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(any(windows, test))]
fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    b << 16 | a
}

/// `rgba` (`width` × `height`, four bytes a pixel, top row first) as a PNG
/// file. The pixels are stored uncompressed: an icon is a few kilobytes
/// either way, and this needs no compression library.
#[cfg(any(windows, test))]
fn png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let row = width as usize * 4;
    if width == 0 || height == 0 || width > 256 || height > 256 || rgba.len() != row * height as usize {
        return None;
    }
    // Each row starts with its filter type (0: none).
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for line in rgba.chunks_exact(row) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    // A zlib stream of "stored" blocks, each at most 65,535 bytes.
    let mut data = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    while let Some(block) = blocks.next() {
        data.push(u8::from(blocks.peek().is_none()));
        data.extend_from_slice(&(block.len() as u16).to_le_bytes());
        data.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        data.extend_from_slice(block);
    }
    data.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
    let mut chunk = |kind: &[u8; 4], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    };
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits a channel, RGBA
    chunk(b"IHDR", &header);
    chunk(b"IDAT", &data);
    chunk(b"IEND", &[]);
    Some(out)
}

#[cfg(windows)]
mod windows {
    use super::{base64, png, InstalledApp};
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::core::{Interface, GUID, PWSTR};
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItem2, IShellItemImageFactory,
        SHGetKnownFolderItem, KF_FLAG_DEFAULT, SIGDN_NORMALDISPLAY, SIIGBF_ICONONLY,
    };

    /// System.AppUserModel.ID: what `shell:AppsFolder\<id>` launches.
    const APP_ID: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3), pid: 5 };
    /// System.Link.TargetParsingPath: the program a classic shortcut starts.
    const TARGET: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xB9B4B3FC_2B51_4A42_B5D8_324146AFCF25), pid: 2 };
    /// System.Link.Arguments: what the shortcut passes it.
    const ARGUMENTS: PROPERTYKEY =
        PROPERTYKEY { fmtid: GUID::from_u128(0x436F2667_14E2_4FEB_B30A_146C53B5B674), pid: 100 };

    /// Icons are asked for at this size (pixels) and shown smaller, so they
    /// stay sharp on high-density screens.
    const ICON: i32 = 32;
    /// More entries than any Start menu has: a bound on the work, whatever
    /// the shell reports.
    const MOST: usize = 2_000;

    /// A string the shell allocated, copied and freed.
    unsafe fn take(text: PWSTR) -> String {
        if text.is_null() {
            return String::new();
        }
        let owned = text.to_string().unwrap_or_default();
        CoTaskMemFree(Some(text.0 as *const c_void));
        owned
    }

    unsafe fn property(item: &IShellItem2, key: &PROPERTYKEY) -> String {
        item.GetString(key).map(|text| take(text)).unwrap_or_default()
    }

    /// Entries Start lists that aren't apps to launch with a gesture:
    /// uninstallers, read-me files, web links.
    fn is_noise(name: &str, id: &str, target: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        if id.starts_with("http://") || id.starts_with("https://") {
            return true;
        }
        let extension = target.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        lower.starts_with("uninstall")
            || lower.contains("readme")
            || lower.contains("read me")
            || ["url", "chm", "txt", "pdf", "htm", "html", "rtf", "ini"].contains(&extension.as_str())
    }

    /// What to store for this app. A classic program started without
    /// arguments is stored by its path, which "Switch to an app" can match
    /// against open windows. Everything else (packaged apps, shortcuts with
    /// arguments) goes by its id.
    fn program(id: &str, target: &str, arguments: &str) -> String {
        let is_program = target.to_ascii_lowercase().ends_with(".exe") && std::path::Path::new(target).is_absolute();
        if is_program && arguments.trim().is_empty() && !id.contains('!') {
            target.to_string()
        } else {
            format!(r"shell:AppsFolder\{id}")
        }
    }

    /// The pixels of a bitmap the shell made, as straight (not
    /// premultiplied) RGBA, top row first.
    unsafe fn pixels(bitmap: HBITMAP) -> Option<Vec<u8>> {
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: ICON,
                biHeight: -ICON, // top row first
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (ICON * ICON * 4) as usize];
        let screen = GetDC(None);
        let rows = GetDIBits(
            screen,
            bitmap,
            0,
            ICON as u32,
            Some(bgra.as_mut_ptr() as *mut c_void),
            &mut info,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, screen);
        if rows != ICON {
            return None;
        }
        // An icon without transparency reports every alpha as 0: it is opaque.
        let opaque = bgra.chunks_exact(4).all(|p| p[3] == 0);
        for p in bgra.chunks_exact_mut(4) {
            p.swap(0, 2); // BGRA -> RGBA
            if opaque {
                p[3] = 255;
            } else if p[3] != 0 && p[3] != 255 {
                // The shell hands out colours already multiplied by alpha.
                let alpha = u32::from(p[3]);
                for c in &mut p[..3] {
                    *c = ((u32::from(*c) * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
        Some(bgra)
    }

    unsafe fn icon(item: &IShellItem) -> Option<String> {
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let bitmap = factory.GetImage(SIZE { cx: ICON, cy: ICON }, SIIGBF_ICONONLY).ok()?;
        let rgba = pixels(bitmap);
        let _ = DeleteObject(bitmap);
        let file = png(ICON as u32, ICON as u32, &rgba?)?;
        Some(format!("data:image/png;base64,{}", base64(&file)))
    }

    unsafe fn list() -> Option<Vec<InstalledApp>> {
        let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None).ok()?;
        let entries: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems).ok()?;
        let mut apps = Vec::new();
        for _ in 0..MOST {
            let mut next = [None];
            let mut fetched = 0u32;
            if entries.Next(&mut next, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = next[0].take() else { break };
            let name = item.GetDisplayName(SIGDN_NORMALDISPLAY).map(|text| take(text)).unwrap_or_default();
            let Ok(details) = item.cast::<IShellItem2>() else { continue };
            let id = property(&details, &APP_ID);
            let target = property(&details, &TARGET);
            if name.trim().is_empty() || id.is_empty() || is_noise(&name, &id, &target) {
                continue;
            }
            let program = program(&id, &target, &property(&details, &ARGUMENTS));
            apps.push(InstalledApp { name: name.trim().to_string(), program, icon: icon(&item) });
        }
        Some(apps)
    }

    pub fn installed_apps() -> Vec<InstalledApp> {
        unsafe {
            // The shell's objects want a single-threaded apartment. If this
            // thread already has one of another kind, they still work.
            let initialised = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let apps = list().unwrap_or_default();
            if initialised {
                CoUninitialize();
            }
            apps
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_classic_program_goes_by_its_path_and_the_rest_by_id() {
            let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
            assert_eq!(program("Some.Id", &exe, ""), exe);
            assert_eq!(program("Some.Id", &exe, "  "), exe);
            // A shortcut with arguments would lose them if stored by path.
            assert_eq!(program("Some.Id", &exe, "--profile work"), r"shell:AppsFolder\Some.Id");
            // A packaged app's program moves with every update.
            assert_eq!(program("Claude_pzs8sxrjxfjjc!Claude", "", ""), r"shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude");
            assert_eq!(program("Pkg_1!App", &exe, ""), r"shell:AppsFolder\Pkg_1!App");
            assert_eq!(program("Docs.Id", r"C:\Docs\manual.pdf", ""), r"shell:AppsFolder\Docs.Id");
            assert_eq!(program("Rel.Id", r"relative\app.exe", ""), r"shell:AppsFolder\Rel.Id");
        }

        #[test]
        fn uninstallers_and_documents_are_left_out() {
            assert!(is_noise("Uninstall Thing", "a", r"C:\Thing\unins000.exe"));
            assert!(is_noise("Thing Readme", "a", r"C:\Thing\readme.txt"));
            assert!(is_noise("Thing on the Web", "a", r"C:\Thing\site.URL"));
            assert!(is_noise("Visit Java.com", "https://java.com/", ""));
            assert!(!is_noise("Thing", "a", r"C:\Thing\thing.exe"));
            assert!(!is_noise("Claude", "Claude_pzs8sxrjxfjjc!Claude", ""));
        }

        /// With this machine's real Start menu.
        #[test]
        fn the_apps_start_shows_are_listed_with_icons() {
            let apps = crate::apps::installed_apps();
            // A build server without a signed-in desktop may have no Start menu to list.
            if apps.len() < 5 {
                eprintln!("only {} apps listed here: nothing to check", apps.len());
                return;
            }
            assert!(apps.len() <= MOST);
            let mut with_icon = 0;
            for app in &apps {
                assert!(!app.name.trim().is_empty() && !app.program.is_empty(), "{app:?}");
                assert!(
                    app.program.starts_with(r"shell:AppsFolder\") || std::path::Path::new(&app.program).is_absolute(),
                    "{} would be looked up by name: {}",
                    app.name,
                    app.program
                );
                if let Some(icon) = &app.icon {
                    assert!(icon.starts_with("data:image/png;base64,") && icon.len() < 20_000, "{}", app.name);
                    with_icon += 1;
                }
            }
            assert!(with_icon * 2 >= apps.len(), "only {with_icon} of {} apps have an icon", apps.len());
            let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
            let mut sorted = names.clone();
            sorted.sort_by_key(|n| n.to_lowercase());
            assert_eq!(names, sorted, "sorted by name");
        }
    }
}

/// A desktop entry's name, if it is an app a menu would show.
#[cfg(any(target_os = "linux", test))]
fn desktop_entry_name(text: &str) -> Option<String> {
    let mut in_entry = false;
    let (mut name, mut is_app, mut hidden, mut has_exec) = (None, false, false, false);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            // Only the main group counts; actions ("[Desktop Action new]") follow it.
            if in_entry {
                break;
            }
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "Name" => name = Some(value.to_string()),
            "Type" => is_app = value == "Application",
            "NoDisplay" | "Hidden" => hidden |= value.eq_ignore_ascii_case("true"),
            "Exec" => has_exec = !value.is_empty(),
            _ => {}
        }
    }
    name.filter(|n| is_app && has_exec && !hidden && !n.is_empty())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{desktop_entry_name, InstalledApp};
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Where desktops look for entries, the user's own first (they win).
    fn folders() -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        let mut out: Vec<PathBuf> = data_home.into_iter().collect();
        out.extend(home.map(|h| h.join(".local/share/flatpak/exports/share")));
        out.extend(data_dirs.split(':').filter(|d| d.starts_with('/')).map(PathBuf::from));
        out.push("/var/lib/flatpak/exports/share".into());
        out.push("/var/lib/snapd/desktop".into());
        out.into_iter().map(|d| d.join("applications")).collect()
    }

    pub fn installed_apps() -> Vec<InstalledApp> {
        let mut seen = HashSet::new();
        let mut apps = Vec::new();
        for folder in folders() {
            let Ok(entries) = std::fs::read_dir(&folder) else { continue };
            for path in entries.flatten().map(|e| e.path()).take(5_000) {
                if path.extension().is_none_or(|x| x != "desktop") {
                    continue;
                }
                let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
                // The same id in a later folder is the entry this one overrides.
                if !seen.insert(id.clone()) {
                    continue;
                }
                // A desktop entry is a few hundred bytes; anything huge isn't one.
                if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                if let Some(name) = desktop_entry_name(&text) {
                    apps.push(InstalledApp { name, program: id, icon: None });
                }
            }
        }
        apps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_test_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
        assert_eq!(base64(&[0xFB, 0xFF, 0xFE]), "+//+");
    }

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn a_png_has_the_right_shape() {
        let rgba: Vec<u8> = (0..2 * 3 * 4).map(|i| i as u8).collect();
        let file = png(2, 3, &rgba).unwrap();
        assert_eq!(&file[..8], &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']);
        // IHDR: length 13, then width 2, height 3, 8-bit RGBA.
        assert_eq!(&file[8..16], &[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
        assert_eq!(&file[16..29], &[0, 0, 0, 2, 0, 0, 0, 3, 8, 6, 0, 0, 0]);
        assert_eq!(&file[file.len() - 12..], &[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]);
        // The pixel data: a zlib header, one final stored block of 3 rows of
        // (1 filter byte + 8 pixel bytes), then its checksum.
        let idat = file.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
        assert_eq!(&file[idat..idat + 7], &[0x78, 0x01, 1, 27, 0, !27u8, 0xFF]);
        assert_eq!(&file[idat + 7..idat + 9], &[0, 0], "row 1: no filter, then its first byte");
        // Sizes that don't match the pixels, or absurd ones, are refused.
        assert_eq!(png(2, 2, &rgba), None);
        assert_eq!(png(0, 3, &[]), None);
        assert_eq!(png(4_000, 4_000, &[]), None);
    }

    #[test]
    fn desktop_entries_are_read_like_a_menu_reads_them() {
        let entry = "[Desktop Entry]\nType=Application\nName=Files\nName[de]=Dateien\nExec=nautilus %U\n\n[Desktop Action new]\nName=New Window\nExec=nautilus --new-window\n";
        assert_eq!(desktop_entry_name(entry).as_deref(), Some("Files"));
        for not_shown in [
            "[Desktop Entry]\nType=Application\nName=Hidden\nExec=x\nNoDisplay=true\n",
            "[Desktop Entry]\nType=Application\nName=Gone\nExec=x\nHidden=True\n",
            "[Desktop Entry]\nType=Link\nName=A link\nURL=https://example.com\n",
            "[Desktop Entry]\nType=Application\nName=No program\n",
            "[Desktop Entry]\nType=Application\nExec=x\n",
            "[Other]\nType=Application\nName=Wrong group\nExec=x\n",
            "",
        ] {
            assert_eq!(desktop_entry_name(not_shown), None, "{not_shown:?}");
        }
    }

    #[test]
    fn the_list_is_sorted_and_within_what_a_config_holds() {
        let apps = installed_apps();
        for pair in apps.windows(2) {
            assert!(pair[0].name.to_lowercase() <= pair[1].name.to_lowercase());
        }
        for app in &apps {
            assert!(app.program.len() <= pady_core::Config::MAX_TEXT && !app.name.chars().any(char::is_control));
        }
    }
}
