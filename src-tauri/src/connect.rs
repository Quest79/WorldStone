//! Connecting (Step 7): making the game client log in to this server.
//!
//! The client reads the address of the realm list server from one line of a text file next to the
//! game (`set realmlist <address>`), or, for a game that keeps it per language, in the folder of
//! each language the client has. This module changes only that line and nothing else in the
//! file: the address it replaces is kept in a backup next to it first, a backup is never
//! overwritten, and "restore" puts the old address back without touching any other line the
//! person has changed since. The game itself is never touched.

use crate::{
    client, compose,
    paths::InstancePaths,
    recipe::{ClientSpec, Recipe},
    tasks::Task,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

const BACKUP_SUFFIX: &str = ".worldstone-backup";
const TEMP_SUFFIX: &str = ".worldstone-tmp";
const BOM: char = '\u{feff}';

// ---------------------------------------------------------------------------------------
// The line
// ---------------------------------------------------------------------------------------

/// One line of the file, split into its text and its line ending (which may be empty).
fn split_lines(text: &str) -> Vec<(&str, &str)> {
    text.split_inclusive('\n')
        .map(|piece| {
            let content = piece.trim_end_matches(['\r', '\n']);
            (content, &piece[content.len()..])
        })
        .collect()
}

/// Where the `n`th whitespace-separated word of a line starts and ends.
fn word_spans(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (at, c) in line.char_indices() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(at),
            (true, Some(from)) => {
                spans.push((from, at));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        spans.push((from, line.len()));
    }
    spans
}

/// Whether a line sets the realm list, as `set <key> ...` in any letter case.
fn is_setting(line: &str, key: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    words.len() >= 2 && words[0].eq_ignore_ascii_case("set") && words[1].eq_ignore_ascii_case(key)
}

/// The game obeys the last such line, so that is the one that counts.
fn last_setting(lines: &[(&str, &str)], key: &str) -> Option<usize> {
    lines.iter().rposition(|(line, _)| is_setting(line, key))
}

fn strip_bom(text: &str) -> (&str, &'static str) {
    match text.strip_prefix(BOM) {
        Some(rest) => (rest, "\u{feff}"),
        None => (text, ""),
    }
}

/// The address the file sets, without quotes. `Some("")` for a line that sets nothing.
pub fn read_address(text: &str, key: &str) -> Option<String> {
    let (text, _) = strip_bom(text);
    let lines = split_lines(text);
    let (line, _) = lines[last_setting(&lines, key)?];
    let address = word_spans(line).get(2).map(|&(from, to)| line[from..to].trim_matches('"').to_string());
    Some(address.unwrap_or_default())
}

/// The file with its realm list address set to `address`, or the line removed for `None`. Every
/// other line, and every line ending, stays as it was; a file without the line gets one at the end.
pub fn set_address(text: &str, key: &str, address: Option<&str>) -> String {
    let (body, bom) = strip_bom(text);
    let lines = split_lines(body);
    let found = last_setting(&lines, key);
    let ending = if body.contains("\r\n") || body.is_empty() { "\r\n" } else { "\n" };
    let mut out = String::from(bom);
    for (index, (line, end)) in lines.iter().enumerate() {
        if Some(index) != found {
            out.push_str(line);
            out.push_str(end);
            continue;
        }
        let Some(address) = address else { continue };
        let spans = word_spans(line);
        match spans.get(2) {
            Some(&(from, to)) => {
                out.push_str(&line[..from]);
                out.push_str(address);
                out.push_str(&line[to..]);
            }
            None => {
                out.push_str(line.trim_end());
                out.push(' ');
                out.push_str(address);
            }
        }
        out.push_str(end);
    }
    if let (None, Some(address)) = (found, address) {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(ending);
        }
        out.push_str(&format!("set {key} {address}{ending}"));
    }
    out
}

// ---------------------------------------------------------------------------------------
// Backups
// ---------------------------------------------------------------------------------------

/// The saved copies of a file, oldest first: `name.worldstone-backup`, then `.2`, `.3` and so on.
fn backups(dir: &Path, file: &str) -> Vec<(u32, PathBuf)> {
    // The file may sit in a subfolder; its copies sit beside it.
    let full = dir.join(file);
    let (Some(folder), Some(name)) = (full.parent(), full.file_name()) else { return Vec::new() };
    let base = format!("{}{BACKUP_SUFFIX}", name.to_string_lossy());
    let mut found: Vec<(u32, PathBuf)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let number = if name == base { 1 } else { name.strip_prefix(&format!("{base}."))?.parse::<u32>().ok().filter(|n| *n >= 2)? };
            Some((number, entry.path()))
        })
        .collect();
    found.sort();
    found
}

fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("{} is not plain text, so it is left alone.", path.display()))
}

/// Written next to the file and moved into place, so a crash never leaves half a file.
fn write_text(path: &Path, text: &str) -> Result<(), String> {
    let mut name = path.file_name().ok_or("The file has no name.")?.to_os_string();
    name.push(TEMP_SUFFIX);
    let temp = path.with_file_name(name);
    let hint = "If the game is in a protected folder such as Program Files, move it or run WorldStone as an administrator.";
    std::fs::write(&temp, text).map_err(|e| format!("Cannot write {}: {e}. {hint}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("Cannot write {}: {e}. {hint}", path.display())
    })
}

// ---------------------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ConnectState {
    /// No client folder is chosen yet.
    NoClient,
    /// The chosen folder cannot be used, or its file cannot be read.
    Unknown,
    /// The client has no such file yet.
    NoFile,
    /// The file sends the client to this server.
    Pointed,
    /// The file sends the client somewhere else, or names no address.
    Elsewhere,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConnectStatus {
    pub state: ConnectState,
    pub note: Option<String>,
    pub client: Option<String>,
    /// The file that holds the address.
    pub file: Option<String>,
    /// The address the file sets now.
    pub address: Option<String>,
    /// The address this server is reached at from this machine.
    pub wanted: String,
    /// The newest saved copy, and the address it holds.
    pub backup: Option<String>,
    pub backup_address: Option<String>,
}

fn blank(state: ConnectState, note: Option<String>) -> ConnectStatus {
    ConnectStatus {
        state,
        note,
        client: None,
        file: None,
        address: None,
        wanted: compose::HOST_ADDRESS.to_string(),
        backup: None,
        backup_address: None,
    }
}

/// What the file in `client_dir` says, judged against `wanted`.
fn read_status(client_dir: &Path, spec: &ClientSpec, wanted: &str) -> ConnectStatus {
    let file = client_dir.join(&spec.realmlist_file);
    let base = ConnectStatus {
        client: Some(client_dir.to_string_lossy().into_owned()),
        file: Some(file.to_string_lossy().into_owned()),
        wanted: wanted.to_string(),
        ..blank(ConnectState::Unknown, None)
    };
    let newest = backups(client_dir, &spec.realmlist_file).pop();
    let saved = newest.as_ref().map(|(_, path)| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let saved_address = newest.as_ref().and_then(|(_, path)| read_text(path).ok()).map(|text| read_address(&text, &spec.realmlist_key).unwrap_or_default());
    let base = ConnectStatus { backup: saved, backup_address: saved_address, ..base };
    if !file.exists() {
        return ConnectStatus { state: ConnectState::NoFile, ..base };
    }
    match read_text(&file) {
        Err(note) => ConnectStatus { note: Some(note), ..base },
        Ok(text) => {
            let address = read_address(&text, &spec.realmlist_key);
            let pointed = address.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(wanted));
            ConnectStatus { state: if pointed { ConnectState::Pointed } else { ConnectState::Elsewhere }, address, ..base }
        }
    }
}

/// One copy of the client's description for each realm list file this client has: a game that keeps
/// the file in each language's folder (`{locale}`) has one per language the client holds.
fn per_file(spec: &ClientSpec, locales: &[String]) -> Result<Vec<ClientSpec>, String> {
    if !spec.realmlist_file.contains("{locale}") {
        return Ok(vec![spec.clone()]);
    }
    if locales.is_empty() {
        return Err("The client has no language folder, so it has no realm list file to change.".to_string());
    }
    Ok(locales.iter().map(|l| ClientSpec { realmlist_file: spec.realmlist_file.replace("{locale}", l), ..spec.clone() }).collect())
}

/// The remembered client's realm list file, read now; nothing is changed. With several languages it
/// is the first file that does not point here, so a half-done change is not reported as done.
pub fn status(paths: &InstancePaths, recipe: &Recipe) -> ConnectStatus {
    let known = client::status(paths, &recipe.client);
    match (known.info, known.problem) {
        (Some(info), _) => match per_file(&recipe.client, &info.locales) {
            Ok(specs) => {
                let all: Vec<ConnectStatus> = specs.iter().map(|s| read_status(Path::new(&info.path), s, compose::HOST_ADDRESS)).collect();
                let at = all.iter().position(|s| s.state != ConnectState::Pointed).unwrap_or(0);
                all.into_iter().nth(at).expect("a client has at least one realm list file")
            }
            Err(problem) => ConnectStatus { client: Some(info.path.clone()), ..blank(ConnectState::Unknown, Some(problem)) },
        },
        (None, Some(problem)) => ConnectStatus { client: known.path, ..blank(ConnectState::Unknown, Some(problem)) },
        (None, None) => blank(ConnectState::NoClient, Some("Choose your game client first.".to_string())),
    }
}

// ---------------------------------------------------------------------------------------
// Changing the file
// ---------------------------------------------------------------------------------------

/// The remembered client's folder and the realm list files in it, checked again now.
fn client_folder(paths: &InstancePaths, recipe: &Recipe) -> Result<(PathBuf, Vec<ClientSpec>), String> {
    let known = client::status(paths, &recipe.client);
    match (known.info, known.problem) {
        (Some(info), _) => Ok((PathBuf::from(&info.path), per_file(&recipe.client, &info.locales)?)),
        (None, Some(problem)) => Err(problem),
        (None, None) => Err("Choose your game client first.".to_string()),
    }
}

/// Sends the client to `wanted`, keeping the address it had.
fn point_in(task: &mut Task, dir: &Path, spec: &ClientSpec, wanted: &str) -> Result<(), String> {
    let file = dir.join(&spec.realmlist_file);
    let key = spec.realmlist_key.as_str();
    task.step("Reading the client's realm list file");
    if !file.exists() {
        write_text(&file, &set_address("", key, Some(wanted)))?;
        task.info(&format!("Created {} with the address {wanted}", file.display()));
        return verify(&file, key, wanted);
    }
    let text = read_text(&file)?;
    if read_address(&text, key).is_some_and(|a| a.eq_ignore_ascii_case(wanted)) {
        task.info(&format!("The client already logs in to {wanted}; nothing was changed"));
        return Ok(());
    }

    task.step("Saving the address it has now");
    let saved = backups(dir, &spec.realmlist_file);
    let identical = match saved.last() {
        Some((_, path)) => read_text(path).is_ok_and(|old| old == text),
        None => false,
    };
    if identical {
        task.info("The current file is already saved");
    } else {
        let number = saved.last().map_or(1, |(n, _)| n + 1);
        let mut name = format!("{}{BACKUP_SUFFIX}", spec.realmlist_file);
        if number > 1 {
            name.push_str(&format!(".{number}"));
        }
        let path = dir.join(&name);
        std::fs::copy(&file, &path).map_err(|e| format!("Cannot save {}: {e}", path.display()))?;
        task.info(&format!("Saved a copy as {name}"));
    }

    task.step(&format!("Pointing the client at {wanted}"));
    write_text(&file, &set_address(&text, key, Some(wanted)))?;
    verify(&file, key, wanted)?;
    task.info(&format!("The client now logs in to {wanted}"));
    Ok(())
}

fn verify(file: &Path, key: &str, wanted: &str) -> Result<(), String> {
    let now = read_address(&read_text(file)?, key);
    if now.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(wanted)) {
        Ok(())
    } else {
        Err(format!("{} was written, but it does not say {wanted} afterwards.", file.display()))
    }
}

/// Puts the saved address back and removes the copy it came from.
fn restore_in(task: &mut Task, dir: &Path, spec: &ClientSpec) -> Result<(), String> {
    let file = dir.join(&spec.realmlist_file);
    let key = spec.realmlist_key.as_str();
    task.step("Finding the saved address");
    let (_, backup) = backups(dir, &spec.realmlist_file).pop().ok_or("There is no saved address to go back to.")?;
    let old = read_address(&read_text(&backup)?, key);
    let text = read_text(&file)?;
    // Only the address line goes back; whatever else the file says now stays.
    task.step("Putting the saved address back");
    write_text(&file, &set_address(&text, key, old.as_deref()))?;
    let now = read_address(&read_text(&file)?, key);
    if now != old {
        return Err(format!("{} was written, but its address is not the saved one afterwards.", file.display()));
    }
    std::fs::remove_file(&backup).map_err(|e| format!("The address is back, but the copy {} could not be removed: {e}", backup.display()))?;
    match old.as_deref() {
        Some(address) if !address.is_empty() => task.info(&format!("The client logs in to {address} again")),
        _ => task.info("The client's address setting is back to what it was"),
    }
    Ok(())
}

pub fn point(task: &mut Task, paths: &InstancePaths, recipe: &Recipe) -> Result<(), String> {
    let (dir, files) = client_folder(paths, recipe)?;
    for spec in &files {
        point_in(task, &dir, spec, compose::HOST_ADDRESS)?;
    }
    Ok(())
}

pub fn restore(task: &mut Task, paths: &InstancePaths, recipe: &Recipe) -> Result<(), String> {
    let (dir, files) = client_folder(paths, recipe)?;
    // A language whose file was never changed has nothing to put back.
    let saved: Vec<&ClientSpec> = files.iter().filter(|spec| !backups(&dir, &spec.realmlist_file).is_empty()).collect();
    if saved.is_empty() {
        return Err("There is no saved address to go back to.".to_string());
    }
    for spec in saved {
        restore_in(task, &dir, spec)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::test_support::manager;

    fn spec() -> ClientSpec {
        crate::recipe::builtin().unwrap().remove(0).client
    }

    fn task(dir: &Path) -> Task {
        let (manager, _) = manager(dir);
        manager.begin("test", false).unwrap()
    }

    fn per_language() -> ClientSpec {
        ClientSpec { realmlist_file: "Data/{locale}/realmlist.wtf".into(), ..spec() }
    }

    #[test]
    fn a_game_with_one_file_per_language_has_one_file_for_each_language_the_client_holds() {
        let names: Vec<String> = per_file(&per_language(), &["enUS".into(), "deDE".into()]).unwrap().into_iter().map(|s| s.realmlist_file).collect();
        assert_eq!(names, ["Data/enUS/realmlist.wtf", "Data/deDE/realmlist.wtf"]);
        assert!(per_file(&per_language(), &[]).unwrap_err().contains("no language folder"));
        let single = per_file(&spec(), &["enUS".into()]).unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].realmlist_file, "realmlist.wtf", "a game with one file ignores the languages");
    }

    #[test]
    fn a_file_in_a_language_folder_is_pointed_and_restored_like_any_other() {
        let dir = tempfile::tempdir().unwrap();
        let spec = per_file(&per_language(), &["enUS".into()]).unwrap().remove(0);
        let file = dir.path().join("Data").join("enUS").join("realmlist.wtf");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "set realmlist logon.example.com\r\n").unwrap();
        let mut t = task(dir.path());
        point_in(&mut t, dir.path(), &spec, "127.0.0.1").unwrap();
        assert_eq!(read_address(&std::fs::read_to_string(&file).unwrap(), "realmlist").as_deref(), Some("127.0.0.1"));
        assert!(dir.path().join("Data").join("enUS").join("realmlist.wtf.worldstone-backup").exists(), "the backup sits beside the file");
        restore_in(&mut t, dir.path(), &spec).unwrap();
        assert_eq!(read_address(&std::fs::read_to_string(&file).unwrap(), "realmlist").as_deref(), Some("logon.example.com"));
    }

    #[test]
    fn the_address_is_read_from_the_last_matching_line_in_any_case() {
        assert_eq!(read_address("set realmlist 127.0.0.1\r\n", "realmlist").as_deref(), Some("127.0.0.1"));
        assert_eq!(read_address("SET REALMLIST logon.example.com", "realmlist").as_deref(), Some("logon.example.com"));
        assert_eq!(read_address("set realmlist \"10.0.0.5\"\n", "realmlist").as_deref(), Some("10.0.0.5"));
        assert_eq!(read_address("set realmlist a\nset realmlist b\n", "realmlist").as_deref(), Some("b"));
        assert_eq!(read_address("set realmlist\n", "realmlist").as_deref(), Some(""));
        assert_eq!(read_address("set patchlist x\n", "realmlist"), None);
        assert_eq!(read_address("", "realmlist"), None);
        assert_eq!(read_address("\u{feff}set realmlist 1.2.3.4\n", "realmlist").as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn changing_the_address_keeps_every_other_line_and_line_ending() {
        let text = "set realmlist old.example.com\r\nset patchlist x.example.com\r\nset gxWindow 1";
        let changed = set_address(text, "realmlist", Some("127.0.0.1"));
        assert_eq!(changed, "set realmlist 127.0.0.1\r\nset patchlist x.example.com\r\nset gxWindow 1");
        // The spelling of the words in front is kept, and so is a quoted address's position.
        assert_eq!(set_address("SET Realmlist \"old\"\n", "realmlist", Some("127.0.0.1")), "SET Realmlist 127.0.0.1\n");
        // A line without an address gets one.
        assert_eq!(set_address("set realmlist\n", "realmlist", Some("127.0.0.1")), "set realmlist 127.0.0.1\n");
        // Only the last of several lines changes, since that is the one the game obeys.
        assert_eq!(set_address("set realmlist a\nset realmlist b\n", "realmlist", Some("c")), "set realmlist a\nset realmlist c\n");
        // A byte order mark survives.
        assert_eq!(set_address("\u{feff}set realmlist a\n", "realmlist", Some("b")), "\u{feff}set realmlist b\n");
    }

    #[test]
    fn a_file_without_the_line_gets_one_in_its_own_line_ending() {
        assert_eq!(set_address("", "realmlist", Some("127.0.0.1")), "set realmlist 127.0.0.1\r\n");
        assert_eq!(set_address("set gxWindow 1\n", "realmlist", Some("h")), "set gxWindow 1\nset realmlist h\n");
        assert_eq!(set_address("set gxWindow 1\r\n", "realmlist", Some("h")), "set gxWindow 1\r\nset realmlist h\r\n");
        assert_eq!(set_address("set gxWindow 1", "realmlist", Some("h")), "set gxWindow 1\nset realmlist h\n");
    }

    #[test]
    fn removing_the_line_leaves_the_rest() {
        assert_eq!(set_address("set a 1\r\nset realmlist x\r\nset b 2\r\n", "realmlist", None), "set a 1\r\nset b 2\r\n");
        assert_eq!(set_address("set a 1\n", "realmlist", None), "set a 1\n");
    }

    #[test]
    fn pointing_saves_the_old_file_first_and_never_overwrites_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = task(dir.path());
        let file = dir.path().join("realmlist.wtf");
        std::fs::write(&file, "set realmlist logon.example.com\r\nset patchlist p.example.com\r\n").unwrap();

        point_in(&mut t, dir.path(), &spec(), "127.0.0.1").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "set realmlist 127.0.0.1\r\nset patchlist p.example.com\r\n");
        let first = dir.path().join("realmlist.wtf.worldstone-backup");
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "set realmlist logon.example.com\r\nset patchlist p.example.com\r\n");

        // Already pointed: nothing changes, nothing new is saved.
        point_in(&mut t, dir.path(), &spec(), "127.0.0.1").unwrap();
        assert_eq!(backups(dir.path(), "realmlist.wtf").len(), 1);

        // Someone points the client elsewhere and back again: the earlier copy stays, a new one is added.
        std::fs::write(&file, "set realmlist other.example.com\r\n").unwrap();
        point_in(&mut t, dir.path(), &spec(), "127.0.0.1").unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("realmlist.wtf.worldstone-backup.2")).unwrap(), "set realmlist other.example.com\r\n");
        assert!(std::fs::read_to_string(&first).unwrap().contains("logon.example.com"));

        // The same file pointed away again is not saved twice.
        std::fs::write(&file, "set realmlist other.example.com\r\n").unwrap();
        point_in(&mut t, dir.path(), &spec(), "127.0.0.1").unwrap();
        assert_eq!(backups(dir.path(), "realmlist.wtf").len(), 2);
    }

    #[test]
    fn a_client_without_the_file_gets_one_and_nothing_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = task(dir.path());
        point_in(&mut t, dir.path(), &spec(), "127.0.0.1").unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("realmlist.wtf")).unwrap(), "set realmlist 127.0.0.1\r\n");
        assert!(backups(dir.path(), "realmlist.wtf").is_empty());
        assert!(!dir.path().join("realmlist.wtf.worldstone-tmp").exists());
    }

    #[test]
    fn restoring_puts_back_the_address_and_keeps_later_edits() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = task(dir.path());
        let file = dir.path().join("realmlist.wtf");
        std::fs::write(&file, "set realmlist logon.example.com\n").unwrap();
        point_in(&mut t, dir.path(), &spec(), "127.0.0.1").unwrap();
        // The person adds a setting of their own while the client points here.
        std::fs::write(&file, "set realmlist 127.0.0.1\nset gxWindow 1\n").unwrap();

        restore_in(&mut t, dir.path(), &spec()).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "set realmlist logon.example.com\nset gxWindow 1\n");
        assert!(backups(dir.path(), "realmlist.wtf").is_empty(), "the copy is used up");
        assert!(restore_in(&mut t, dir.path(), &spec()).unwrap_err().contains("no saved address"));
    }

    #[test]
    fn restoring_uses_the_newest_copy() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = task(dir.path());
        std::fs::write(dir.path().join("realmlist.wtf"), "set realmlist 127.0.0.1\n").unwrap();
        std::fs::write(dir.path().join("realmlist.wtf.worldstone-backup"), "set realmlist first\n").unwrap();
        std::fs::write(dir.path().join("realmlist.wtf.worldstone-backup.2"), "set realmlist second\n").unwrap();
        std::fs::write(dir.path().join("realmlist.wtf.worldstone-backup.x"), "set realmlist not-a-backup\n").unwrap();
        restore_in(&mut t, dir.path(), &spec()).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("realmlist.wtf")).unwrap(), "set realmlist second\n");
        assert!(dir.path().join("realmlist.wtf.worldstone-backup").exists());
        assert!(!dir.path().join("realmlist.wtf.worldstone-backup.2").exists());
        assert!(dir.path().join("realmlist.wtf.worldstone-backup.x").exists());
    }

    #[test]
    fn a_file_that_is_not_text_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = task(dir.path());
        let file = dir.path().join("realmlist.wtf");
        std::fs::write(&file, [0xff, 0xfe, 0x00, 0x80]).unwrap();
        assert!(point_in(&mut t, dir.path(), &spec(), "127.0.0.1").unwrap_err().contains("not plain text"));
        assert_eq!(std::fs::read(&file).unwrap(), vec![0xff, 0xfe, 0x00, 0x80]);
        assert!(backups(dir.path(), "realmlist.wtf").is_empty());
    }

    #[test]
    fn the_status_says_where_the_client_logs_in_and_what_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let spec = spec();
        assert_eq!(read_status(dir.path(), &spec, "127.0.0.1").state, ConnectState::NoFile);

        std::fs::write(dir.path().join("realmlist.wtf"), "set realmlist logon.example.com\n").unwrap();
        let s = read_status(dir.path(), &spec, "127.0.0.1");
        assert_eq!((s.state, s.address.as_deref(), s.backup), (ConnectState::Elsewhere, Some("logon.example.com"), None));

        std::fs::write(dir.path().join("realmlist.wtf"), "SET REALMLIST 127.0.0.1\n").unwrap();
        std::fs::write(dir.path().join("realmlist.wtf.worldstone-backup"), "set realmlist logon.example.com\n").unwrap();
        let s = read_status(dir.path(), &spec, "127.0.0.1");
        assert_eq!(s.state, ConnectState::Pointed);
        assert_eq!(s.backup.as_deref(), Some("realmlist.wtf.worldstone-backup"));
        assert_eq!(s.backup_address.as_deref(), Some("logon.example.com"));

        std::fs::write(dir.path().join("realmlist.wtf"), "set gxWindow 1\n").unwrap();
        let s = read_status(dir.path(), &spec, "127.0.0.1");
        assert_eq!((s.state, s.address), (ConnectState::Elsewhere, None));
    }
}
