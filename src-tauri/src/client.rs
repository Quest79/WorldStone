//! The game client the user supplies: finding out whether it is one this server can use, and
//! remembering which folder was chosen.
//!
//! The client is only ever read. Its build comes from the version stamped into the game
//! executable, so a folder is accepted because of what it is, not because of what it is called.

use crate::{paths::InstancePaths, recipe::ClientSpec};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What was found in a client folder that passed every check.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    /// The folder that holds the game executable (the folder chosen, or its parent when the
    /// user picked the `Data` folder inside it).
    pub path: String,
    /// The `Data` folder's name as it is spelled on disk.
    pub data_dir: String,
    pub build: u32,
    /// The patch level players know the build by, such as `1.12.1`.
    pub version: String,
    /// Language folders inside `Data`. Empty for a client with its language merged into `Data`.
    pub locales: Vec<String>,
    /// Combined size of the archives in `Data`.
    pub data_bytes: u64,
}

/// The remembered client and whether it still passes, for the UI.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientStatus {
    /// The folder the user chose earlier, if any.
    pub path: Option<String>,
    pub info: Option<ClientInfo>,
    /// Why the remembered folder cannot be used now.
    pub problem: Option<String>,
}

// ---------------------------------------------------------------------------------------
// Reading the build from the executable
// ---------------------------------------------------------------------------------------

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

const RESOURCE_DIRECTORY: usize = 2;
const RT_VERSION: u32 = 16;
const VERSION_SIGNATURE: u32 = 0xFEEF_04BD;

/// The four-part file version of a Windows executable, such as `[1, 12, 1, 5875]`, read from
/// its version resource. `None` when the file is not a Windows executable or has no version.
pub fn exe_version(bytes: &[u8]) -> Option<[u16; 4]> {
    if bytes.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(bytes, 0x3C)? as usize;
    if bytes.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = u16_at(bytes, pe + 6)? as usize;
    let optional_size = u16_at(bytes, pe + 20)? as usize;
    let optional = pe + 24;
    let directories = match u16_at(bytes, optional)? {
        0x10B => optional + 96,  // PE32
        0x20B => optional + 112, // PE32+
        _ => return None,
    };
    let resource_rva = u32_at(bytes, directories + RESOURCE_DIRECTORY * 8)? as usize;
    if resource_rva == 0 {
        return None;
    }

    // Resources are addressed by RVA; the section table maps those back to file offsets.
    let table = optional + optional_size;
    let to_offset = |rva: usize| -> Option<usize> {
        (0..sections).find_map(|i| {
            let s = table + i * 40;
            let (size, va, raw_size, raw) = (
                u32_at(bytes, s + 8)? as usize,
                u32_at(bytes, s + 12)? as usize,
                u32_at(bytes, s + 16)? as usize,
                u32_at(bytes, s + 20)? as usize,
            );
            (rva >= va && rva < va + size.max(raw_size)).then(|| raw + (rva - va))
        })
    };
    let root = to_offset(resource_rva)?;

    // One level of the resource tree: the entries as (id, target). A target with the top bit
    // set is another directory, relative to the root; otherwise it is a data entry.
    let entries = |dir: usize| -> Option<Vec<(u32, u32)>> {
        let count = u16_at(bytes, dir + 12)? as usize + u16_at(bytes, dir + 14)? as usize;
        (0..count.min(64))
            .map(|i| Some((u32_at(bytes, dir + 16 + i * 8)?, u32_at(bytes, dir + 20 + i * 8)?)))
            .collect()
    };
    let subdirectory = |target: u32| (target & 0x8000_0000 != 0).then(|| root + (target & 0x7FFF_FFFF) as usize);

    // Type (RT_VERSION) -> name -> language -> data.
    let version_dir = entries(root)?
        .into_iter()
        .find(|(id, _)| *id == RT_VERSION)
        .and_then(|(_, target)| subdirectory(target))?;
    let name_dir = subdirectory(entries(version_dir)?.first()?.1)?;
    let data_entry = root + (entries(name_dir)?.first()?.1 & 0x7FFF_FFFF) as usize;
    let data = to_offset(u32_at(bytes, data_entry)? as usize)?;
    let size = u32_at(bytes, data_entry + 4)? as usize;

    // VS_VERSIONINFO is a small header, the key "VS_VERSION_INFO", then VS_FIXEDFILEINFO,
    // which starts with a fixed signature and is 4-byte aligned.
    let block = bytes.get(data..data.checked_add(size)?)?;
    let at = (0..block.len().min(128)).step_by(4).find(|&i| u32_at(block, i) == Some(VERSION_SIGNATURE))?;
    let (ms, ls) = (u32_at(block, at + 8)?, u32_at(block, at + 12)?);
    Some([(ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16])
}

// ---------------------------------------------------------------------------------------
// Checking a folder
// ---------------------------------------------------------------------------------------

/// The entry of `dir` whose name matches `name` ignoring case (Windows does not care how a
/// client's files are capitalised, and neither should the check).
fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

/// Language folders in `data`: those holding the locale archive named after themselves.
fn find_locales(data: &Path, spec: &ClientSpec) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(data) else { return Vec::new() };
    let mut found: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| find_ci(&data.join(name), &spec.locale_archive.replace("{locale}", name)).is_some())
        .collect();
    found.sort();
    found
}

/// Total size of the files directly in `dir` that end in `.mpq`, ignoring case.
fn archive_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".mpq"))
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

fn list_builds(spec: &ClientSpec) -> String {
    let all: Vec<String> = spec.builds.iter().map(|b| format!("{} ({})", b.name, b.number)).collect();
    match all.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Decides whether `chosen` is a client this emulator can extract from, and says exactly why
/// not when it is not. The folder is only read.
pub fn inspect(chosen: &Path, spec: &ClientSpec) -> Result<ClientInfo, String> {
    if !chosen.is_dir() {
        return Err(format!("{} is not a folder.", chosen.display()));
    }
    let executable = spec.executable.as_str();

    // Someone who picked the Data folder inside the client meant the client.
    let mut root = chosen.to_path_buf();
    if find_ci(&root, executable).is_none() {
        let inside_data = root.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&spec.data_dir));
        match root.parent() {
            Some(parent) if inside_data && find_ci(parent, executable).is_some() => root = parent.to_path_buf(),
            _ => {
                return Err(format!(
                    "{executable} is not in {}. Choose the folder that contains {executable}.",
                    chosen.display()
                ))
            }
        }
    }

    let exe_path = find_ci(&root, executable).expect("checked above");
    let bytes = std::fs::read(&exe_path).map_err(|e| format!("Cannot read {}: {e}", exe_path.display()))?;
    let version = exe_version(&bytes).ok_or_else(|| {
        format!("{executable} has no version information, so its build cannot be identified. Is it the real game executable?")
    })?;
    let build = u32::from(version[3]);
    let Some(known) = spec.builds.iter().find(|b| b.number == build) else {
        return Err(format!(
            "This is client build {build} ({}.{}.{}). This server needs {}.",
            version[0],
            version[1],
            version[2],
            list_builds(spec)
        ));
    };

    let data = find_ci(&root, &spec.data_dir)
        .filter(|d| d.is_dir())
        .ok_or_else(|| format!("The {} folder is missing from {}.", spec.data_dir, root.display()))?;
    let missing: Vec<&str> = spec.archives.iter().filter(|a| find_ci(&data, a).is_none()).map(String::as_str).collect();
    if !missing.is_empty() {
        return Err(format!(
            "The client is incomplete: {} {} missing from {}.",
            missing.join(", "),
            if missing.len() == 1 { "is" } else { "are" },
            data.display()
        ));
    }

    Ok(ClientInfo {
        path: root.to_string_lossy().into_owned(),
        data_dir: data.file_name().map_or_else(|| spec.data_dir.clone(), |n| n.to_string_lossy().into_owned()),
        build,
        version: known.name.clone(),
        locales: find_locales(&data, spec),
        data_bytes: archive_bytes(&data),
    })
}

// ---------------------------------------------------------------------------------------
// The remembered choice
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Saved {
    path: String,
}

fn saved_file(paths: &InstancePaths) -> PathBuf {
    paths.config.join("client.json")
}

/// The folder chosen earlier, if one was saved and the file is readable.
pub fn saved_path(paths: &InstancePaths) -> Option<String> {
    let text = std::fs::read_to_string(saved_file(paths)).ok()?;
    serde_json::from_str::<Saved>(&text).ok().map(|s| s.path).filter(|p| !p.is_empty())
}

/// Remembers a client folder. Written to a temporary file first so a crash never leaves half a file.
fn save(paths: &InstancePaths, path: &str) -> Result<(), String> {
    paths.ensure()?;
    let file = saved_file(paths);
    let temp = file.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(&Saved { path: path.to_string() }).map_err(|e| e.to_string())?;
    std::fs::write(&temp, text).map_err(|e| format!("Cannot write {}: {e}", temp.display()))?;
    std::fs::rename(&temp, &file).map_err(|e| format!("Cannot write {}: {e}", file.display()))
}

/// Cleans up what a person pastes: surrounding spaces and quotes (Explorer's "Copy as path").
pub fn clean_input(raw: &str) -> &str {
    raw.trim().trim_matches('"').trim()
}

/// Checks `raw` and, only when it is a usable client, remembers it.
pub fn select(paths: &InstancePaths, spec: &ClientSpec, raw: &str) -> Result<ClientInfo, String> {
    let raw = clean_input(raw);
    if raw.is_empty() {
        return Err("Enter the folder that contains your game client.".to_string());
    }
    let info = inspect(Path::new(raw), spec)?;
    save(paths, &info.path)?;
    Ok(info)
}

/// The remembered client, checked again now: the folder may have been moved or emptied since.
pub fn status(paths: &InstancePaths, spec: &ClientSpec) -> ClientStatus {
    let Some(path) = saved_path(paths) else {
        return ClientStatus { path: None, info: None, problem: None };
    };
    match inspect(Path::new(&path), spec) {
        Ok(info) => ClientStatus { path: Some(path), info: Some(info), problem: None },
        Err(problem) => ClientStatus { path: Some(path), info: None, problem: Some(problem) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;

    fn spec() -> ClientSpec {
        crate::recipe::builtin().unwrap().remove(0).client
    }

    // -- a minimal Windows executable with a version resource ------------------------------

    fn put16(v: &mut [u8], at: usize, n: u16) {
        v[at..at + 2].copy_from_slice(&n.to_le_bytes());
    }

    fn put32(v: &mut [u8], at: usize, n: u32) {
        v[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }

    /// A PE32 file with one section holding a resource tree with an RT_VERSION entry.
    fn fake_exe(version: [u16; 4]) -> Vec<u8> {
        const SECTION_RVA: u32 = 0x3000;
        const SECTION_FILE: usize = 0x200;
        let mut v = vec![0u8; SECTION_FILE + 0x100];
        v[..2].copy_from_slice(b"MZ");
        put32(&mut v, 0x3C, 0x40);
        v[0x40..0x44].copy_from_slice(b"PE\0\0");
        put16(&mut v, 0x40 + 6, 1); // one section
        put16(&mut v, 0x40 + 20, 0xE0); // optional header size
        put16(&mut v, 0x40 + 24, 0x10B); // PE32
        put32(&mut v, 0x40 + 24 + 96 + RESOURCE_DIRECTORY * 8, SECTION_RVA);
        let section = 0x40 + 24 + 0xE0;
        put32(&mut v, section + 8, 0x100); // virtual size
        put32(&mut v, section + 12, SECTION_RVA);
        put32(&mut v, section + 16, 0x100); // raw size
        put32(&mut v, section + 20, SECTION_FILE as u32);

        // Resource tree, offsets relative to the section: type dir 0, name dir 24, language
        // dir 48, data entry 72, version block 88.
        let r = SECTION_FILE;
        put16(&mut v, r + 14, 1);
        put32(&mut v, r + 16, RT_VERSION);
        put32(&mut v, r + 20, 0x8000_0000 | 24);
        put16(&mut v, r + 24 + 14, 1);
        put32(&mut v, r + 24 + 16, 1);
        put32(&mut v, r + 24 + 20, 0x8000_0000 | 48);
        put16(&mut v, r + 48 + 14, 1);
        put32(&mut v, r + 48 + 16, 0x409);
        put32(&mut v, r + 48 + 20, 72);
        put32(&mut v, r + 72, SECTION_RVA + 88); // data RVA
        put32(&mut v, r + 76, 0x40); // size
        // VS_VERSIONINFO header (6 bytes), the UTF-16 key, padding, then VS_FIXEDFILEINFO at 40.
        let block = r + 88;
        put16(&mut v, block, 0x34);
        for (i, c) in "VS_VERSION_INFO".encode_utf16().enumerate() {
            put16(&mut v, block + 6 + i * 2, c);
        }
        put32(&mut v, block + 40, VERSION_SIGNATURE);
        put32(&mut v, block + 48, (u32::from(version[0]) << 16) | u32::from(version[1]));
        put32(&mut v, block + 52, (u32::from(version[2]) << 16) | u32::from(version[3]));
        v
    }

    #[test]
    fn reads_the_build_from_the_version_resource() {
        assert_eq!(exe_version(&fake_exe([1, 12, 1, 5875])), Some([1, 12, 1, 5875]));
        assert_eq!(exe_version(&fake_exe([3, 3, 5, 12340])), Some([3, 3, 5, 12340]));
    }

    #[test]
    fn a_file_that_is_not_an_executable_has_no_version() {
        assert_eq!(exe_version(b""), None);
        assert_eq!(exe_version(b"MZ"), None);
        assert_eq!(exe_version(&vec![0u8; 4096]), None);
        let mut truncated = fake_exe([1, 12, 1, 5875]);
        truncated.truncate(0x230);
        assert_eq!(exe_version(&truncated), None, "a cut-off file must not panic");
    }

    // -- a client folder -------------------------------------------------------------------

    fn client(build: u16, archives: &[&str], locale: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("WoW.exe"), fake_exe([1, 12, 1, build])).unwrap();
        let data = dir.path().join("Data");
        std::fs::create_dir(&data).unwrap();
        for a in archives {
            std::fs::write(data.join(a), vec![0u8; 1000]).unwrap();
        }
        if let Some(loc) = locale {
            std::fs::create_dir(data.join(loc)).unwrap();
            std::fs::write(data.join(loc).join(format!("locale-{loc}.MPQ")), b"x").unwrap();
        }
        dir
    }

    const ARCHIVES: [&str; 6] = ["base.MPQ", "dbc.MPQ", "misc.MPQ", "model.MPQ", "terrain.MPQ", "wmo.MPQ"];

    #[test]
    fn accepts_a_supported_client_and_reports_what_it_found() {
        let dir = client(5875, &ARCHIVES, Some("enUS"));
        let info = inspect(dir.path(), &spec()).unwrap();
        assert_eq!((info.build, info.version.as_str()), (5875, "1.12.1"));
        assert_eq!(info.locales, ["enUS"]);
        assert_eq!(info.data_bytes, 6000);
        assert_eq!(info.data_dir, "Data");
        assert_eq!(inspect(client(6141, &ARCHIVES, None).path(), &spec()).unwrap().version, "1.12.3");
    }

    #[test]
    fn a_client_with_its_language_merged_into_data_is_fine() {
        let info = inspect(client(5875, &ARCHIVES, None).path(), &spec()).unwrap();
        assert!(info.locales.is_empty());
    }

    #[test]
    fn choosing_the_data_folder_finds_the_client_around_it() {
        let dir = client(5875, &ARCHIVES, None);
        let info = inspect(&dir.path().join("Data"), &spec()).unwrap();
        assert_eq!(Path::new(&info.path), dir.path());
    }

    #[test]
    fn refuses_a_client_of_another_game_version_and_says_which_are_supported() {
        let error = inspect(client(8606, &ARCHIVES, None).path(), &spec()).unwrap_err();
        assert!(error.contains("build 8606"), "{error}");
        assert!(error.contains("1.12.1 (5875), 1.12.2 (6005) or 1.12.3 (6141)"), "{error}");
    }

    #[test]
    fn refuses_a_folder_without_the_game_or_with_missing_archives() {
        let empty = tempfile::tempdir().unwrap();
        assert!(inspect(empty.path(), &spec()).unwrap_err().contains("WoW.exe is not in"));
        assert!(inspect(&empty.path().join("nope"), &spec()).unwrap_err().contains("is not a folder"));

        let error = inspect(client(5875, &["base.MPQ", "dbc.MPQ"], None).path(), &spec()).unwrap_err();
        assert!(error.contains("misc.MPQ") && error.contains("wmo.MPQ") && error.contains("are missing"), "{error}");

        let fake = tempfile::tempdir().unwrap();
        std::fs::write(fake.path().join("WoW.exe"), b"not a program").unwrap();
        assert!(inspect(fake.path(), &spec()).unwrap_err().contains("no version information"));
    }

    #[test]
    fn file_names_are_matched_ignoring_case() {
        let dir = client(5875, &["BASE.mpq", "Dbc.MPQ", "misc.mpq", "MODEL.MPQ", "terrain.mpq", "WMO.MPQ"], None);
        std::fs::rename(dir.path().join("WoW.exe"), dir.path().join("WOW.EXE")).unwrap();
        assert!(inspect(dir.path(), &spec()).is_ok());
    }

    #[test]
    fn pasted_paths_are_cleaned() {
        assert_eq!(clean_input("  \"C:\\Games\\WoW 1.12\"  "), "C:\\Games\\WoW 1.12");
        assert_eq!(clean_input("C:\\x"), "C:\\x");
    }

    #[test]
    fn only_a_usable_client_is_remembered() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().to_path_buf()).instance("mangos-zero").unwrap();
        assert_eq!(status(&paths, &spec()), ClientStatus { path: None, info: None, problem: None });

        assert!(select(&paths, &spec(), "  ").is_err());
        assert!(select(&paths, &spec(), &home.path().to_string_lossy()).is_err());
        assert_eq!(saved_path(&paths), None, "a rejected folder is not remembered");

        let dir = client(5875, &ARCHIVES, None);
        let quoted = format!("\"{}\"", dir.path().display());
        let info = select(&paths, &spec(), &quoted).unwrap();
        assert_eq!(saved_path(&paths).as_deref(), Some(info.path.as_str()));
        assert!(status(&paths, &spec()).info.is_some());

        // The folder disappears: the status says why instead of pretending.
        drop(dir);
        let after = status(&paths, &spec());
        assert!(after.info.is_none() && after.problem.unwrap().contains("not a folder"));
        assert!(after.path.is_some());
    }
}
