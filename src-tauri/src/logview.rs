//! The log viewer (Step 9): what the launcher has done, what each container printed, and the log
//! files the servers write, both as they happen and from before.
//!
//! Nothing is remembered here. The launcher log is a file that outlives the app, Docker keeps what a
//! container printed until the container is removed, and the servers' own files sit in the
//! instance's `Logs` folder, so a restart of the launcher (or of the servers) loses nothing.
//! A source is named by an id the UI got from [`sources`]; whatever id comes back is checked against
//! that list again before anything is read, so a name typed into a request never becomes a path or
//! a Docker argument by itself.

use crate::{
    clock, compose, config, docker,
    paths::{valid_component, InstancePaths, Paths},
    process::{capture, CommandSpec},
    recipe::Recipe,
    runtime::clean_log_line_to,
    tasks::LogLevel,
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Duration,
};

pub const MIN_LINES: u32 = 20;
pub const MAX_LINES: u32 = 5000;
/// A line longer than this is cut, so one runaway line cannot swamp the viewer.
const LINE_WIDTH: usize = 2000;
/// The most of a log file that is read from its end, however long the file is.
const FILE_READ_LIMIT: u64 = 8 * 1024 * 1024;

pub const LAUNCHER: &str = "launcher";
const CONTAINER_PREFIX: &str = "container:";
const FILE_PREFIX: &str = "file:";

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Launcher,
    Container,
    File,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LogSource {
    pub id: String,
    pub label: String,
    pub kind: SourceKind,
    /// Something can be read from it now.
    pub available: bool,
    /// What it is, or why it cannot be read.
    pub detail: Option<String>,
    pub bytes: Option<u64>,
    /// When a file was last written, in seconds since the epoch.
    pub modified: Option<u64>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ViewLine {
    /// Milliseconds since the epoch, when the source says when the line was written.
    pub ts_ms: Option<u64>,
    pub level: LogLevel,
    pub text: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LogView {
    pub source: String,
    pub lines: Vec<ViewLine>,
    /// The source holds older lines than the ones shown.
    pub truncated: bool,
    /// Why there is nothing to show, or anything else worth saying.
    pub note: Option<String>,
}

/// How many lines a request gets: what was asked for, kept within the limits.
pub fn clamp_lines(asked: u32) -> usize {
    asked.clamp(MIN_LINES, MAX_LINES) as usize
}

#[derive(PartialEq, Eq, Debug)]
enum Source {
    Launcher,
    Container(String),
    File(String),
}

fn parse_source(id: &str) -> Option<Source> {
    if id == LAUNCHER {
        Some(Source::Launcher)
    } else if let Some(service) = id.strip_prefix(CONTAINER_PREFIX) {
        Some(Source::Container(service.to_string()))
    } else {
        id.strip_prefix(FILE_PREFIX).map(|name| Source::File(name.to_string()))
    }
}

/// The container a service id names: the database, or one of the recipe's servers.
fn container_for(instance: &str, recipe: &Recipe, service: &str) -> Option<String> {
    if service == compose::DATABASE_SERVICE {
        Some(compose::database_container(instance))
    } else {
        recipe.run.services.iter().find(|s| s.id == service).map(|s| compose::service_container(instance, &s.id))
    }
}

/// The path of a server log file. Only a plain `*.log` name that is a file in the folder qualifies.
fn log_file_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let is_log = Path::new(name).extension().is_some_and(|e| e.eq_ignore_ascii_case("log"));
    if !valid_component(name) || !is_log {
        return None;
    }
    let path = dir.join(name);
    path.is_file().then_some(path)
}

fn modified_secs(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified().ok().and_then(clock::epoch_of)
}

/// The `*.log` files in a folder as `(name, bytes, modified)`, by name.
fn log_files(dir: &Path) -> Vec<(String, u64, Option<u64>)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<(String, u64, Option<u64>)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok().filter(|m| m.is_file())?;
            let name = entry.file_name().into_string().ok()?;
            Path::new(&name).extension().is_some_and(|e| e.eq_ignore_ascii_case("log")).then(|| (name, meta.len(), modified_secs(&meta)))
        })
        .collect();
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    files
}

// ---------------------------------------------------------------------------------------
// What can be read
// ---------------------------------------------------------------------------------------

/// Everything the viewer can show, in the order the UI lists it: the launcher, the containers,
/// then the servers' own files.
pub async fn sources(paths: &Paths, instance_paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Vec<LogSource> {
    let mut out = Vec::new();

    let launcher = std::fs::metadata(paths.launcher_log()).ok().filter(|m| m.is_file());
    out.push(LogSource {
        id: LAUNCHER.to_string(),
        label: "WorldStone activity".to_string(),
        kind: SourceKind::Launcher,
        available: true,
        detail: Some(if launcher.is_some() {
            "Every operation the launcher has run, kept between sessions.".to_string()
        } else {
            "Nothing has been logged yet.".to_string()
        }),
        bytes: launcher.as_ref().map(std::fs::Metadata::len),
        modified: launcher.as_ref().and_then(modified_secs),
    });

    let listed = docker::containers(true).await;
    let present: BTreeSet<String> = listed.as_ref().map(|all| all.iter().map(|c| c.name.clone()).collect()).unwrap_or_default();
    let why_not = listed.as_ref().err().map(|why| format!("Docker cannot be asked: {}", why.lines().next().unwrap_or("no answer").trim()));
    let services = recipe.run.services.iter().map(|s| (s.id.clone(), s.label.clone())).chain([(compose::DATABASE_SERVICE.to_string(), "Database".to_string())]);
    for (service, label) in services {
        let Some(container) = container_for(instance, recipe, &service) else { continue };
        let there = present.contains(&container);
        out.push(LogSource {
            id: format!("{CONTAINER_PREFIX}{service}"),
            label: format!("{label} (container)"),
            kind: SourceKind::Container,
            available: there,
            detail: Some(match (&why_not, there) {
                (Some(why), _) => why.clone(),
                (None, true) => container,
                (None, false) => "The container does not exist yet. Starting the servers creates it.".to_string(),
            }),
            bytes: None,
            modified: None,
        });
    }

    for (name, bytes, modified) in log_files(&config::logs_dir(instance_paths)) {
        out.push(LogSource {
            id: format!("{FILE_PREFIX}{name}"),
            label: name,
            kind: SourceKind::File,
            available: true,
            detail: Some("A log file the server writes.".to_string()),
            bytes: Some(bytes),
            modified,
        });
    }
    out
}

// ---------------------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------------------

/// One line of the launcher log, `[<milliseconds>] [<Level>] <text>`. Anything that is not in that
/// form is kept as plain text: it is a line of something else, never a reason to drop it.
pub fn parse_launcher_line(line: &str) -> ViewLine {
    let parsed = (|| {
        let rest = line.strip_prefix('[')?;
        let (stamp, rest) = rest.split_once("] [")?;
        let (level, text) = rest.split_once(']')?;
        let level = match level {
            "Info" => LogLevel::Info,
            "Stdout" => LogLevel::Stdout,
            "Stderr" => LogLevel::Stderr,
            "Error" => LogLevel::Error,
            _ => return None,
        };
        Some(ViewLine { ts_ms: Some(stamp.parse::<u64>().ok()?), level, text: text.strip_prefix(' ').unwrap_or(text).to_string() })
    })();
    parsed.unwrap_or_else(|| ViewLine { ts_ms: None, level: LogLevel::Stdout, text: line.to_string() })
}

/// A line of `docker logs --timestamps`: the time Docker wrote it, then what the program printed.
fn split_docker_timestamp(line: &str) -> Option<(&str, u64, &str)> {
    let (stamp, text) = line.split_once(' ').unwrap_or((line, ""));
    Some((stamp, clock::parse_rfc3339_utc(stamp)?, text))
}

/// What `docker logs --timestamps` printed as lines in time order. Docker hands back what the
/// program wrote to standard output and what it wrote to standard error as two blocks; the time
/// Docker stamped on each line puts them back in the order they happened. A line without a time
/// (a wrapped one) stays after the line before it.
pub fn parse_container_log(text: &str) -> Vec<ViewLine> {
    let mut keyed: Vec<(String, ViewLine)> = Vec::new();
    let mut key = String::new();
    for raw in text.lines() {
        let (ts_ms, body) = match split_docker_timestamp(raw) {
            Some((stamp, secs, body)) => {
                key = stamp.to_string();
                (Some(secs * 1000), body)
            }
            None => (None, raw),
        };
        if let Some(text) = clean_log_line_to(&body.replace('\t', "    "), LINE_WIDTH) {
            keyed.push((key.clone(), ViewLine { ts_ms, level: LogLevel::Stdout, text }));
        }
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    keyed.into_iter().map(|(_, line)| line).collect()
}

/// The last `count` lines of a file, read from its end so a long log is never read whole. The flag
/// says the file holds older lines than the ones returned.
pub fn tail_file(path: &Path, count: usize) -> Result<(Vec<String>, bool), String> {
    let fail = |e: std::io::Error| format!("Cannot read {}: {e}", path.display());
    let mut file = std::fs::File::open(path).map_err(fail)?;
    let len = file.metadata().map_err(fail)?.len();
    let start = len.saturating_sub(FILE_READ_LIMIT);
    file.seek(SeekFrom::Start(start)).map_err(fail)?;
    let mut bytes = Vec::new();
    file.take(FILE_READ_LIMIT).read_to_end(&mut bytes).map_err(fail)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    let mut older = start > 0;
    if older && !lines.is_empty() {
        // The read began somewhere inside a line.
        lines.remove(0);
    }
    if lines.len() > count {
        lines.drain(..lines.len() - count);
        older = true;
    }
    Ok((lines.into_iter().map(str::to_string).collect(), older))
}

fn plain_lines(lines: Vec<String>) -> Vec<ViewLine> {
    lines
        .iter()
        .filter_map(|l| clean_log_line_to(&l.replace('\t', "    "), LINE_WIDTH))
        .map(|text| ViewLine { ts_ms: None, level: LogLevel::Stdout, text })
        .collect()
}

fn view(source: &str, lines: Vec<ViewLine>, truncated: bool, note: Option<String>) -> LogView {
    LogView { source: source.to_string(), lines, truncated, note }
}

/// The newest `lines` lines of a source.
pub async fn read(paths: &Paths, instance_paths: &InstancePaths, instance: &str, recipe: &Recipe, source: &str, lines: u32) -> Result<LogView, String> {
    let count = clamp_lines(lines);
    match parse_source(source).ok_or_else(|| format!("'{source}' is not a log this app knows."))? {
        Source::Launcher => {
            let path = paths.launcher_log();
            if !path.is_file() {
                return Ok(view(source, Vec::new(), false, Some("Nothing has been logged yet.".to_string())));
            }
            let (raw, older) = tail_file(&path, count)?;
            Ok(view(source, raw.iter().map(|l| parse_launcher_line(l)).collect(), older, None))
        }
        Source::Container(service) => {
            let container = container_for(instance, recipe, &service).ok_or_else(|| format!("'{service}' is not a server of this instance."))?;
            let count_arg = count.to_string();
            let out = capture(&CommandSpec::new("docker", &["logs", "--timestamps", "--tail", &count_arg, &container]).timeout(Duration::from_secs(30))).await;
            if !out.success {
                return if out.text.contains("No such container") {
                    Ok(view(source, Vec::new(), false, Some("The container does not exist yet. Starting the servers creates it.".to_string())))
                } else {
                    Err(format!("Docker could not read the log of {container}: {}", out.text.lines().next().unwrap_or("no answer").trim()))
                };
            }
            let lines = parse_container_log(&out.text);
            let truncated = out.text.lines().count() >= count;
            Ok(view(source, lines, truncated, None))
        }
        Source::File(name) => {
            let path = log_file_path(&config::logs_dir(instance_paths), &name).ok_or_else(|| format!("'{name}' is not a log file of this instance."))?;
            let (raw, older) = tail_file(&path, count)?;
            Ok(view(source, plain_lines(raw), older, None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero() -> Recipe {
        crate::recipe::builtin().unwrap().remove(0)
    }

    #[test]
    fn sources_are_named_by_kind_and_nothing_else_is_accepted() {
        assert_eq!(parse_source("launcher"), Some(Source::Launcher));
        assert_eq!(parse_source("container:mangosd"), Some(Source::Container("mangosd".into())));
        assert_eq!(parse_source("file:Server.log"), Some(Source::File("Server.log".into())));
        assert_eq!(parse_source("Server.log"), None);
        assert_eq!(parse_source(""), None);
    }

    #[test]
    fn only_this_instances_containers_can_be_read() {
        let recipe = zero();
        let first = &recipe.run.services[0].id;
        assert_eq!(container_for("mangos-zero", &recipe, first), Some(compose::service_container("mangos-zero", first)));
        assert_eq!(container_for("mangos-zero", &recipe, "database"), Some(compose::database_container("mangos-zero")));
        for other in ["", "nginx", "worldstone-other-db", "--all", "realmd; rm"] {
            assert_eq!(container_for("mangos-zero", &recipe, other), None, "{other}");
        }
    }

    #[test]
    fn only_plain_log_files_in_the_folder_can_be_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Server.log"), "x").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        std::fs::create_dir(dir.path().join("folder.log")).unwrap();
        assert!(log_file_path(dir.path(), "Server.log").is_some());
        for bad in ["notes.txt", "folder.log", "missing.log", "../Server.log", "a/Server.log", "a\\Server.log", "", "..", "C:Server.log"] {
            assert!(log_file_path(dir.path(), bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn the_files_listed_are_the_log_files_by_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.log"), "12345").unwrap();
        std::fs::write(dir.path().join("A.LOG"), "1").unwrap();
        std::fs::write(dir.path().join("readme.md"), "1").unwrap();
        let files = log_files(dir.path());
        assert_eq!(files.iter().map(|f| (f.0.as_str(), f.1)).collect::<Vec<_>>(), [("A.LOG", 1), ("b.log", 5)]);
        assert!(log_files(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn a_launcher_line_gives_its_time_level_and_text() {
        let line = parse_launcher_line("[1790670072123] [Error] The build failed [really]");
        assert_eq!(line, ViewLine { ts_ms: Some(1_790_670_072_123), level: LogLevel::Error, text: "The build failed [really]".into() });
        assert_eq!(parse_launcher_line("[5] [Info] ").text, "");
        assert_eq!(parse_launcher_line("[5] [Stderr] a").level, LogLevel::Stderr);
    }

    #[test]
    fn a_line_that_is_not_in_the_launcher_format_is_kept_as_it_is() {
        for other in ["plain text", "[abc] [Info] x", "[5] [Fatal] x", "[5] no level", ""] {
            let line = parse_launcher_line(other);
            assert_eq!((line.ts_ms, line.level, line.text.as_str()), (None, LogLevel::Stdout, other), "{other}");
        }
    }

    #[test]
    fn container_lines_get_their_times_and_the_two_streams_are_put_in_order() {
        // Docker printed the standard-output block first, then the standard-error block.
        let text = "2026-09-29T08:21:12.100000000Z first\n\
                    2026-09-29T08:21:14.000000000Z third\n\
                    2026-09-29T08:21:13.000000000Z second (stderr)\n\
                    wrapped part of second\n";
        let lines = parse_container_log(text);
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["first", "second (stderr)", "wrapped part of second", "third"]);
        assert_eq!(lines[0].ts_ms, Some(1_790_670_072_000));
        assert_eq!(lines[2].ts_ms, None);
    }

    #[test]
    fn container_lines_lose_colour_codes_and_empty_lines() {
        let lines = parse_container_log("2026-09-29T08:21:12.000000000Z \u{1b}[32mready\u{1b}[0m\n2026-09-29T08:21:13.000000000Z \n\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "ready");
    }

    #[test]
    fn the_end_of_a_file_is_read_and_older_lines_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Server.log");
        std::fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(tail_file(&path, 2).unwrap(), (vec!["three".to_string(), "four".to_string()], true));
        assert_eq!(tail_file(&path, 10).unwrap(), (vec!["one".to_string(), "two".to_string(), "three".to_string(), "four".to_string()], false));
        assert!(tail_file(&dir.path().join("missing.log"), 5).is_err());
    }

    #[test]
    fn a_huge_file_is_read_from_its_end_without_its_cut_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Big.log");
        let line = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n";
        let mut text = String::new();
        while (text.len() as u64) < FILE_READ_LIMIT + 5000 {
            text.push_str(line);
        }
        text.push_str("last line\n");
        std::fs::write(&path, &text).unwrap();
        let (lines, older) = tail_file(&path, 3).unwrap();
        assert!(older);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2], "last line");
        assert!(lines.iter().all(|l| l == "last line" || l.len() == 64), "no line was cut in half");
    }

    #[test]
    fn a_request_gets_a_sensible_number_of_lines() {
        assert_eq!(clamp_lines(0), MIN_LINES as usize);
        assert_eq!(clamp_lines(500), 500);
        assert_eq!(clamp_lines(u32::MAX), MAX_LINES as usize);
    }

    #[test]
    fn plain_lines_lose_control_characters_but_keep_their_words() {
        let lines = plain_lines(vec!["a\tb".to_string(), "".to_string(), "\u{1b}[31mred\u{1b}[0m".to_string()]);
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["a    b", "red"]);
    }
}
