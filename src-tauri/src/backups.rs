//! Database backups (Step 9): making one, listing what the instance's `Backups` folder holds, and
//! putting one back.
//!
//! A backup is one compressed dump per schema, named `<schema>-<UTC yyyymmdd-hhmmss>-<label>.sql.gz`.
//! The files one run makes share the time and the label and are shown as one backup. The database
//! container writes them into `Backups`, through a temporary name, so a dump that failed never leaves
//! a file that looks finished.
//!
//! Restoring replaces every schema the backup holds, and it is careful about it:
//! - the servers must be stopped, because a running world server would write over the restored data;
//! - every file is checked to be a whole, finished dump **before** anything is dropped;
//! - what is in the database now is backed up first (label `before-restore`), so a restore can be
//!   undone by restoring that backup;
//! - a database that was stopped is started for the work and stopped again afterwards.
//!
//! The list is read from the folder, never remembered, and a file name is only ever handed to a
//! shell after it has been parsed here from a listing of that folder.

use crate::{
    builder,
    clock,
    compose::{self, BACKUP_MOUNT},
    database::{self, recreate_sql, script, script_spec, Credentials, DatabaseState, LEDGER_SCHEMA},
    paths::InstancePaths,
    recipe::Recipe,
    runtime::{self, ServerState},
    tasks::Task,
};
use serde::Serialize;
use std::{collections::BTreeMap, time::Duration};

/// Made by the *Back up now* button.
pub const MANUAL: &str = "manual";
/// Taken automatically before database updates are applied.
pub const BEFORE_UPDATE: &str = "before-update";
/// Taken automatically before a restore replaces what is there.
pub const BEFORE_RESTORE: &str = "before-restore";
/// Taken automatically before a reset deletes the database.
pub const BEFORE_RESET: &str = "before-reset";

const SUFFIX: &str = ".sql.gz";

// ---------------------------------------------------------------------------------------
// The scripts that run inside the database container
// ---------------------------------------------------------------------------------------

/// `sh -c <script> sh <backup dir> <label> <schema...>`. The dump goes through a temporary file
/// so that a failed dump is noticed and no half-written backup is left under its final name.
pub(crate) const BACKUP_SCRIPT: &str = r#"dir="$1"; label="$2"; shift 2
ts=$(date -u +%Y%m%d-%H%M%S)
for s in "$@"; do
  tmp="/tmp/worldstone-$s.sql"; out="$s-$ts-$label.sql.gz"
  echo "BACKUP $s"
  @DUMP@ -uroot --single-transaction --routines --default-character-set=utf8mb4 "$s" > "$tmp" || { rm -f "$tmp"; echo "FAILED backup of $s"; exit 1; }
  gzip -c "$tmp" > "$dir/$out.part" && mv "$dir/$out.part" "$dir/$out" || { rm -f "$tmp" "$dir/$out.part"; echo "FAILED backup of $s"; exit 1; }
  rm -f "$tmp"
  echo "BACKED $out $(wc -c < "$dir/$out")"
done"#;

/// `sh -c <script> sh <backup dir> <file...>`. A dump that ends without the marker the dump tool
/// writes last was cut short, and is refused before anything is dropped.
const CHECK_SCRIPT: &str = r#"dir="$1"; shift
for f in "$@"; do
  echo "CHECK $f"
  gzip -t "$dir/$f" || { echo "FAILED $f is not a complete compressed file"; exit 1; }
  zcat "$dir/$f" | tail -n 3 | grep -q '^-- Dump completed' || { echo "FAILED $f does not end like a finished dump"; exit 1; }
done"#;

/// `sh -c <script> sh <backup dir> <file> <schema>`, into a schema that has just been made empty.
pub(crate) const LOAD_SCRIPT: &str = r#"dir="$1"; f="$2"; schema="$3"
tmp="/tmp/worldstone-restore-$schema.sql"
zcat "$dir/$f" > "$tmp" || { rm -f "$tmp"; echo "FAILED unpacking $f"; exit 1; }
@CLIENT@ -uroot --default-character-set=utf8mb4 "$schema" < "$tmp" || { rm -f "$tmp"; echo "FAILED loading $f"; exit 1; }
rm -f "$tmp"
echo "LOADED $f""#;

// ---------------------------------------------------------------------------------------
// What is in the folder
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub schema: String,
    pub file: String,
    pub bytes: u64,
}

/// The dumps one run made, shown as one backup.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupSet {
    /// The time and label in the file names, such as `20260929-101500-manual`.
    pub id: String,
    /// When it was made, in seconds since the epoch.
    pub taken: u64,
    pub label: String,
    /// Why it exists, in words.
    pub what: String,
    pub files: Vec<BackupFile>,
    pub bytes: u64,
    /// Every one of this server's databases is in it.
    pub complete: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BackupsStatus {
    /// The folder the files are in.
    pub folder: String,
    /// Newest first.
    pub sets: Vec<BackupSet>,
    /// Set when the folder could not be read.
    pub note: Option<String>,
    /// Why *Back up now* cannot run right now, if it cannot.
    pub backup_blocked: Option<String>,
    /// Why a restore cannot run right now, if it cannot.
    pub restore_blocked: Option<String>,
}

fn plain(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn digits(text: &str, count: usize) -> bool {
    text.len() == count && text.bytes().all(|b| b.is_ascii_digit())
}

/// `(schema, stamp, label)` from `realmd-20260929-101500-before-update.sql.gz`. Schema names and labels
/// may hold hyphens, so the time is what splits them. Anything that is not one of ours is `None`,
/// including a dump that is still being written (`….sql.gz.part`).
pub fn parse_name(file: &str) -> Option<(String, String, String)> {
    let parts: Vec<&str> = file.strip_suffix(SUFFIX)?.split('-').collect();
    let at = (1..parts.len().saturating_sub(2)).find(|&i| digits(parts[i], 8) && digits(parts[i + 1], 6))?;
    let (schema, stamp, label) = (parts[..at].join("-"), format!("{}-{}", parts[at], parts[at + 1]), parts[at + 2..].join("-"));
    (plain(&schema) && plain(&label)).then_some((schema, stamp, label))
}

/// The UTC time in a file name (`20260929-101500`) as seconds since the epoch.
fn stamp_epoch(stamp: &str) -> Option<u64> {
    let (date, time) = stamp.split_once('-')?;
    let field = |text: &str, from: usize, to: usize| text.get(from..to)?.parse::<i64>().ok();
    clock::epoch_from_civil(field(date, 0, 4)?, field(date, 4, 6)?, field(date, 6, 8)?, field(time, 0, 2)?, field(time, 2, 4)?, field(time, 4, 6)?)
}

/// Why a backup with this label exists.
pub fn describe(label: &str) -> String {
    match label {
        MANUAL => "Made with Back up now".to_string(),
        BEFORE_UPDATE => "Taken automatically before a database update".to_string(),
        BEFORE_RESTORE => "Taken automatically before a restore, so that restore can be undone".to_string(),
        BEFORE_RESET => "Taken automatically before a reset".to_string(),
        other => format!("Label: {other}"),
    }
}

/// Every schema a backup of this server can hold: its own databases and the app's ledger.
pub fn all_schemas(recipe: &Recipe) -> Vec<String> {
    recipe.database.schemas.iter().map(|s| s.name.clone()).chain([LEDGER_SCHEMA.to_string()]).collect()
}

/// Groups `(file name, bytes)` into backups, newest first. `expected` are the schemas a complete
/// backup holds.
pub fn group(files: &[(String, u64)], expected: &[String]) -> Vec<BackupSet> {
    let mut runs: BTreeMap<(String, String), Vec<BackupFile>> = BTreeMap::new();
    for (file, bytes) in files {
        if let Some((schema, stamp, label)) = parse_name(file) {
            runs.entry((stamp, label)).or_default().push(BackupFile { schema, file: file.clone(), bytes: *bytes });
        }
    }
    let mut sets: Vec<BackupSet> = runs
        .into_iter()
        .filter_map(|((stamp, label), mut files)| {
            let taken = stamp_epoch(&stamp)?;
            files.sort_by(|a, b| a.schema.cmp(&b.schema));
            let complete = expected.iter().all(|schema| files.iter().any(|f| &f.schema == schema));
            Some(BackupSet { id: format!("{stamp}-{label}"), taken, what: describe(&label), label, bytes: files.iter().map(|f| f.bytes).sum(), complete, files })
        })
        .collect();
    sets.sort_by(|a, b| b.taken.cmp(&a.taken).then_with(|| a.label.cmp(&b.label)));
    sets
}

/// The backups in the instance's `Backups` folder. A folder that does not exist yet holds none.
pub fn list(paths: &InstancePaths, recipe: &Recipe) -> Result<Vec<BackupSet>, String> {
    let entries = match std::fs::read_dir(&paths.backups) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Cannot read {}: {e}", paths.backups.display())),
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        if let (Ok(meta), Some(name)) = (entry.metadata(), entry.file_name().to_str().map(str::to_string)) {
            if meta.is_file() {
                files.push((name, meta.len()));
            }
        }
    }
    Ok(group(&files, &all_schemas(recipe)))
}

/// Whether the name is one of our dumps that never finished (`….sql.gz.part`).
fn is_partial(file: &str) -> bool {
    file.strip_suffix(".part").is_some_and(|whole| parse_name(whole).is_some())
}

/// Dumps that were interrupted before they were finished and renamed. A backup that is running is
/// a task, and only one task runs at a time, so none of these belongs to work in progress.
pub(crate) fn partial_files(paths: &InstancePaths) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(&paths.backups) else { return Vec::new() };
    let mut found: Vec<_> = entries
        .flatten()
        .filter(|e| e.metadata().is_ok_and(|m| m.is_file()) && e.file_name().to_str().is_some_and(is_partial))
        .map(|e| e.path())
        .collect();
    found.sort();
    found
}

/// Removes the unfinished dumps. Finished backups are never touched.
pub(crate) fn remove_partial(task: &mut Task, paths: &InstancePaths) -> Result<(), String> {
    let files = partial_files(paths);
    if files.is_empty() {
        task.info("There are no unfinished backup files.");
        return Ok(());
    }
    for file in &files {
        std::fs::remove_file(file).map_err(|e| format!("Cannot remove {}: {e}", file.display()))?;
        task.info(&format!("Removed unfinished backup file {}.", file.file_name().and_then(|n| n.to_str()).unwrap_or("?")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------------------

/// Why the database cannot be asked to dump or load anything: no Docker, or no database yet.
async fn database_blocked(paths: &InstancePaths, instance: &str) -> Option<String> {
    if let Err(reason) = builder::engine().await {
        return Some(reason);
    }
    match Credentials::load(paths) {
        Err(why) => return Some(why),
        Ok(None) => return Some("There is no database yet. Set up the database first.".to_string()),
        Ok(Some(_)) => {}
    }
    (!database::is_healthy(instance).await && !database::volume_exists(instance).await).then(|| "There is no database yet. Set up the database first.".to_string())
}

/// Why a restore has to wait for the servers, or `None` when they are stopped.
async fn servers_not_stopped(paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Option<String> {
    let seen = runtime::status(paths, instance, recipe).await;
    match seen.state {
        ServerState::Stopped => None,
        ServerState::Unknown => Some(seen.note.unwrap_or_else(|| "Docker cannot be asked whether the servers are stopped.".to_string())),
        _ => Some("Stop the servers first: the world server would write over the restored data.".to_string()),
    }
}

pub async fn status(paths: &InstancePaths, instance: &str, recipe: &Recipe) -> BackupsStatus {
    let (sets, note) = match list(paths, recipe) {
        Ok(sets) => (sets, None),
        Err(why) => (Vec::new(), Some(why)),
    };
    let backup_blocked = database_blocked(paths, instance).await;
    let restore_blocked = match (&backup_blocked, sets.is_empty()) {
        (Some(why), _) => Some(why.clone()),
        (None, true) => Some("There is no backup to restore yet.".to_string()),
        (None, false) => servers_not_stopped(paths, instance, recipe).await,
    };
    BackupsStatus { folder: paths.backups.to_string_lossy().into_owned(), sets, note, backup_blocked, restore_blocked }
}

// ---------------------------------------------------------------------------------------
// Making a backup
// ---------------------------------------------------------------------------------------

/// The `BACKED <file> <bytes>` lines a backup prints; a backup with no bytes is a failure.
fn parse_backed(lines: &[String], schemas: &[&str]) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for schema in schemas {
        let backed = lines.iter().rev().find_map(|l| {
            let mut parts = l.split_whitespace();
            let (tag, file, bytes) = (parts.next()?, parts.next()?, parts.next()?);
            (tag == "BACKED" && file.starts_with(&format!("{schema}-"))).then(|| (file.to_string(), bytes.parse::<u64>().unwrap_or(0)))
        });
        match backed {
            Some((file, bytes)) if bytes > 0 => files.push(file),
            _ => return Err(format!("The backup of {schema} did not produce a file.")),
        }
    }
    Ok(files)
}

/// Dumps the schemas to compressed files in the instance's `Backups` folder and returns their names.
/// The database must be up.
pub(crate) async fn backup(task: &mut Task, instance: &str, creds: &Credentials, schemas: &[&str], label: &str) -> Result<Vec<String>, String> {
    task.step(&format!("Backing up {}", schemas.join(", ")));
    let mut args = vec![BACKUP_MOUNT.to_string(), label.to_string()];
    args.extend(schemas.iter().map(|s| s.to_string()));
    task.run(script_spec(instance, creds, &script(BACKUP_SCRIPT), &args))
        .await
        .map_err(|e| database::explain("The backup failed, so nothing was changed", &e, &task.recent_output()))?;
    let files = parse_backed(&task.recent_output(), schemas)?;
    for file in &files {
        task.info(&format!("Backup saved: Backups/{file}"));
    }
    Ok(files)
}

fn quoted(names: &[&str]) -> String {
    names.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(",")
}

/// The schemas from `names` that the database has. Names come from the recipe or from a file name
/// that [`parse_name`] accepted, so they hold nothing but letters, digits, `_` and `-`.
async fn existing(instance: &str, creds: &Credentials, names: &[&str]) -> Result<Vec<String>, String> {
    if names.iter().any(|n| !plain(n)) {
        return Err("A database name holds characters that are not allowed.".to_string());
    }
    let sql = format!("SELECT schema_name FROM information_schema.schemata WHERE schema_name IN ({});\n", quoted(names));
    let out = database::query(instance, creds, &sql).await.map_err(|e| format!("Could not ask the database which schemas it has: {e}"))?;
    Ok(names.iter().filter(|n| out.lines().any(|l| l.trim() == **n)).map(|n| n.to_string()).collect())
}

/// Backs up every one of this server's databases that exists, and returns the file names (none when
/// the database holds nothing yet). The database must be up.
pub(crate) async fn backup_everything(task: &mut Task, instance: &str, recipe: &Recipe, creds: &Credentials, label: &str) -> Result<Vec<String>, String> {
    let wanted = all_schemas(recipe);
    let names: Vec<&str> = wanted.iter().map(String::as_str).collect();
    let present = existing(instance, creds, &names).await?;
    if present.is_empty() {
        return Ok(Vec::new());
    }
    let present: Vec<&str> = present.iter().map(String::as_str).collect();
    backup(task, instance, creds, &present, label).await
}

/// Makes sure the database container is up for the work; says whether this call started it, so the
/// caller can stop it again and leave things as they were found.
pub(crate) async fn open_database(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<bool, String> {
    if database::is_healthy(instance).await {
        return Ok(false);
    }
    task.step("Starting the database");
    let file = compose::write(paths, recipe, instance)?;
    let up = runtime::compose_spec(paths, instance, &file, &["up", "-d", "--wait", compose::DATABASE_SERVICE], Duration::from_secs(10 * 60));
    task.run(up).await.map_err(|e| database::explain("The database did not start", &e, &task.recent_output()))?;
    Ok(true)
}

/// Stops the database again when [`open_database`] was the one that started it. A failure to stop it
/// is reported but never turns finished work into a failure.
pub(crate) async fn close_database(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe, opened: bool) {
    if !opened {
        return;
    }
    task.step("Stopping the database again");
    let stopped = match compose::write(paths, recipe, instance) {
        Ok(file) => task.run(runtime::compose_spec(paths, instance, &file, &["stop", compose::DATABASE_SERVICE], Duration::from_secs(2 * 60))).await,
        Err(why) => Err(why),
    };
    if let Err(why) = stopped {
        task.info(&format!("The database could not be stopped again: {why} Stop the servers to stop it."));
    }
}

async fn make(task: &mut Task, instance: &str, recipe: &Recipe, creds: &Credentials) -> Result<(), String> {
    let files = backup_everything(task, instance, recipe, creds, MANUAL).await?;
    if files.is_empty() {
        return Err("There is nothing to back up yet: the database holds none of this server's databases.".to_string());
    }
    task.outcome(&format!("Backed up {} database{}.", files.len(), if files.len() == 1 { "" } else { "s" }));
    Ok(())
}

/// *Back up now*: every one of this server's databases, while the servers run or stopped.
pub async fn create(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    task.step("Checking Docker and the database");
    if let Some(why) = database_blocked(paths, instance).await {
        return Err(why);
    }
    paths.ensure()?;
    let creds = Credentials::load(paths)?.ok_or_else(|| "There is no database yet. Set up the database first.".to_string())?;
    for secret in creds.secrets() {
        task.add_secret(secret);
    }
    let opened = open_database(task, paths, instance, recipe).await?;
    let result = make(task, instance, recipe, &creds).await;
    close_database(task, paths, instance, recipe, opened).await;
    result
}

// ---------------------------------------------------------------------------------------
// Restoring
// ---------------------------------------------------------------------------------------

/// How many tables each of the schemas has, from the `schema<TAB>count` lines a query printed.
fn parse_table_counts(text: &str) -> BTreeMap<String, u32> {
    text.lines()
        .filter_map(|line| {
            let (schema, count) = line.trim().split_once('\t')?;
            Some((schema.to_string(), count.trim().parse().ok()?))
        })
        .collect()
}

async fn load(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe, creds: &Credentials, set: &BackupSet) -> Result<(), String> {
    let mut args = vec![BACKUP_MOUNT.to_string()];
    args.extend(set.files.iter().map(|f| f.file.clone()));
    task.step("Checking that the backup is whole");
    task.run(script_spec(instance, creds, CHECK_SCRIPT, &args))
        .await
        .map_err(|e| database::explain("This backup cannot be used, so nothing was changed", &e, &task.recent_output()))?;

    let names: Vec<&str> = set.files.iter().map(|f| f.schema.as_str()).collect();
    let present = existing(instance, creds, &names).await?;
    if present.is_empty() {
        task.info("None of these databases exists yet, so there is nothing to put aside first.");
    } else {
        let present: Vec<&str> = present.iter().map(String::as_str).collect();
        backup(task, instance, creds, &present, BEFORE_RESTORE).await?;
        task.info("What is in the database now was saved first. To undo this restore, restore that backup.");
    }

    for file in &set.files {
        task.step(&format!("Restoring {}", file.schema));
        database::query(instance, creds, &recreate_sql(&recipe.database.charset, &file.schema))
            .await
            .map_err(|e| format!("Could not empty {} for the restore: {e} Nothing from the backup was loaded; the data from before is in the backup taken first.", file.schema))?;
        let load_args = [BACKUP_MOUNT.to_string(), file.file.clone(), file.schema.clone()];
        task.run(script_spec(instance, creds, LOAD_SCRIPT, &load_args)).await.map_err(|e| {
            database::explain(
                &format!("Restoring {} failed. What was there before is in the backup taken first (label {BEFORE_RESTORE}); restore that to go back", file.schema),
                &e,
                &task.recent_output(),
            )
        })?;
    }

    task.step("Checking the result");
    let sql = format!(
        "SELECT table_schema, COUNT(*) FROM information_schema.tables WHERE table_schema IN ({}) GROUP BY table_schema;\n",
        quoted(&names)
    );
    let counts = parse_table_counts(&database::query(instance, creds, &sql).await?);
    for file in &set.files {
        match counts.get(&file.schema) {
            Some(n) if *n > 0 => task.info(&format!("{}: {n} tables restored.", file.schema)),
            _ => return Err(format!("{} has no tables after the restore. The data from before is in the backup taken first.", file.schema)),
        }
    }

    // A backup made before an update is older than the scripts in the source; say so.
    let now = database::status(paths, instance, recipe).await;
    let pending: u32 = now.schemas.iter().map(|s| s.updates_pending).sum();
    if now.state == DatabaseState::Outdated && pending > 0 {
        task.info(&format!(
            "The restored data is older than the database scripts in the source: {pending} update{} waiting. Run Set up database to apply {}; it backs up first.",
            if pending == 1 { " is" } else { "s are" },
            if pending == 1 { "it" } else { "them" }
        ));
    }
    let n = set.files.len();
    task.outcome(&format!(
        "Restored {n} database{}. What {} held before is kept in the Backups folder as a before-restore backup.",
        if n == 1 { "" } else { "s" },
        if n == 1 { "it" } else { "they" }
    ));
    Ok(())
}

/// Puts a backup back: every database in it is emptied and loaded from the dump. See the module notes
/// for what is checked and saved first.
pub async fn restore(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe, set_id: &str) -> Result<(), String> {
    task.step("Checking the backup, Docker and the servers");
    if let Some(why) = database_blocked(paths, instance).await {
        return Err(why);
    }
    let set = list(paths, recipe)?
        .into_iter()
        .find(|s| s.id == set_id)
        .ok_or_else(|| "That backup is no longer in the Backups folder.".to_string())?;
    let known = all_schemas(recipe);
    if let Some(stranger) = set.files.iter().find(|f| !known.contains(&f.schema)) {
        return Err(format!("This backup holds {}, which this server does not have, so it cannot be restored here.", stranger.schema));
    }
    if let Some(why) = servers_not_stopped(paths, instance, recipe).await {
        return Err(why);
    }
    let creds = Credentials::load(paths)?.ok_or_else(|| "There is no database yet. Set up the database first.".to_string())?;
    for secret in creds.secrets() {
        task.add_secret(secret);
    }
    let opened = open_database(task, paths, instance, recipe).await?;
    let result = load(task, paths, instance, recipe, &creds, &set).await;
    close_database(task, paths, instance, recipe, opened).await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero() -> Recipe {
        crate::recipe::builtin().unwrap().remove(0)
    }

    fn file(name: &str, bytes: u64) -> (String, u64) {
        (name.to_string(), bytes)
    }

    #[test]
    fn a_name_gives_its_schema_time_and_label() {
        assert_eq!(
            parse_name("realmd-20260929-101500-before-update.sql.gz"),
            Some(("realmd".into(), "20260929-101500".into(), "before-update".into()))
        );
        assert_eq!(parse_name("worldstone-20260929-101500-manual.sql.gz"), Some(("worldstone".into(), "20260929-101500".into(), "manual".into())));
        // A schema name with a hyphen and digits in it is not confused with the time.
        assert_eq!(parse_name("my-db2-20260929-101500-manual.sql.gz"), Some(("my-db2".into(), "20260929-101500".into(), "manual".into())));
    }

    #[test]
    fn files_that_are_not_finished_backups_are_ignored() {
        for other in [
            "realmd-20260929-101500-manual.sql.gz.part",
            "realmd-20260929-101500-manual.sql",
            "notes.txt",
            "realmd-20260929-manual.sql.gz",
            "-20260929-101500-manual.sql.gz",
            "realmd-20260929-101500.sql.gz",
            "real md-20260929-101500-manual.sql.gz",
            "realmd-20260929-101500-a b.sql.gz",
        ] {
            assert_eq!(parse_name(other), None, "{other}");
        }
    }

    #[test]
    fn the_time_in_a_name_is_utc() {
        assert_eq!(stamp_epoch("20260929-082112"), clock::parse_rfc3339_utc("2026-09-29T08:21:12Z"));
        assert_eq!(stamp_epoch("20261329-082112"), None);
        assert_eq!(stamp_epoch("garbage"), None);
    }

    #[test]
    fn the_files_of_one_run_are_one_backup_and_the_newest_comes_first() {
        let expected = vec!["realmd".to_string(), "character0".to_string(), "worldstone".to_string()];
        let files = vec![
            file("realmd-20260929-101500-manual.sql.gz", 100),
            file("character0-20260929-101500-manual.sql.gz", 300),
            file("worldstone-20260929-101500-manual.sql.gz", 1),
            file("realmd-20260930-090000-before-update.sql.gz", 50),
            file("realmd-20260929-101500-manual.sql.gz.part", 7),
            file("readme.txt", 3),
        ];
        let sets = group(&files, &expected);
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].id, "20260930-090000-before-update");
        assert!(!sets[0].complete, "one database of three");
        assert_eq!(sets[0].what, "Taken automatically before a database update");
        assert_eq!(sets[1].id, "20260929-101500-manual");
        assert!(sets[1].complete);
        assert_eq!(sets[1].bytes, 401);
        assert_eq!(sets[1].files.iter().map(|f| f.schema.as_str()).collect::<Vec<_>>(), ["character0", "realmd", "worldstone"]);
    }

    #[test]
    fn two_labels_at_the_same_time_stay_apart() {
        let files = vec![file("realmd-20260929-101500-manual.sql.gz", 1), file("realmd-20260929-101500-before-restore.sql.gz", 1)];
        let sets = group(&files, &["realmd".to_string()]);
        assert_eq!(sets.len(), 2);
        assert_ne!(sets[0].id, sets[1].id);
    }

    #[test]
    fn the_folder_is_listed_from_disk_and_a_missing_one_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        let recipe = zero();
        assert!(list(&paths, &recipe).unwrap().is_empty(), "no Backups folder yet");
        std::fs::create_dir_all(&paths.backups).unwrap();
        std::fs::write(paths.backups.join("realmd-20260929-101500-manual.sql.gz"), b"x").unwrap();
        std::fs::write(paths.backups.join("realmd-20260929-101500-manual.sql.gz.part"), b"partial").unwrap();
        std::fs::create_dir_all(paths.backups.join("realmd-20260101-000000-folder.sql.gz")).unwrap();
        let sets = list(&paths, &recipe).unwrap();
        assert_eq!(sets.len(), 1, "a temporary file and a folder are not backups");
        assert_eq!(sets[0].files[0].bytes, 1);
    }

    #[test]
    fn only_our_own_unfinished_dumps_count_as_leftovers() {
        assert!(is_partial("realmd-20260929-101500-manual.sql.gz.part"));
        for other in ["realmd-20260929-101500-manual.sql.gz", "notes.part", "movie.sql.gz.part", "realmd-20260929-101500-manual.sql.part"] {
            assert!(!is_partial(other), "{other}");
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        assert!(partial_files(&paths).is_empty(), "no Backups folder yet");
        std::fs::create_dir_all(&paths.backups).unwrap();
        std::fs::write(paths.backups.join("realmd-20260929-101500-manual.sql.gz.part"), b"partial").unwrap();
        std::fs::write(paths.backups.join("realmd-20260929-101500-manual.sql.gz"), b"done").unwrap();
        std::fs::write(paths.backups.join("movie.sql.gz.part"), b"not ours").unwrap();
        let found = partial_files(&paths);
        assert_eq!(found.len(), 1, "a finished backup and a stranger's file are not leftovers");
        assert!(found[0].ends_with("realmd-20260929-101500-manual.sql.gz.part"));
    }

    #[test]
    fn a_complete_backup_holds_every_schema_of_the_recipe_and_the_ledger() {
        let all = all_schemas(&zero());
        assert_eq!(all, ["realmd", "character0", "mangos0", "worldstone"]);
    }

    #[test]
    fn a_backup_without_bytes_is_a_failure() {
        let lines = vec!["BACKUP realmd".to_string(), "BACKED realmd-20260929-101500-before-update.sql.gz 20480".to_string()];
        assert_eq!(parse_backed(&lines, &["realmd"]).unwrap(), ["realmd-20260929-101500-before-update.sql.gz"]);
        let empty = vec!["BACKED realmd-20260929-101500-x.sql.gz 0".to_string()];
        assert!(parse_backed(&empty, &["realmd"]).is_err());
        assert!(parse_backed(&lines, &["mangos0"]).is_err(), "another schema's file is not this one's");
    }

    #[test]
    fn what_the_restore_empties_is_named_and_keeps_the_accounts_grants() {
        let sql = recreate_sql("utf8mb3", "character0");
        assert!(sql.starts_with("DROP DATABASE IF EXISTS `character0`;"));
        assert!(sql.contains("CREATE DATABASE `character0` DEFAULT CHARACTER SET utf8mb3"));
        assert!(!sql.to_uppercase().contains("REVOKE"), "grants belong to the account and stay");
    }

    #[test]
    fn table_counts_ignore_lines_that_are_not_counts() {
        let counts = parse_table_counts("realmd\t12\nwarning: something\ncharacter0\t 80 \n");
        assert_eq!(counts.get("realmd"), Some(&12));
        assert_eq!(counts.get("character0"), Some(&80));
        assert_eq!(counts.len(), 2);
    }

    #[test]
    fn schema_names_reach_sql_only_when_plain() {
        assert!(plain("mangos0") && plain("my-db2") && plain("a_b"));
        assert!(!plain("a`b") && !plain("a'b") && !plain("") && !plain("a b") && !plain("a;b"));
        assert_eq!(quoted(&["realmd", "worldstone"]), "'realmd','worldstone'");
    }

    #[test]
    fn the_scripts_take_names_as_arguments_never_as_text() {
        for script in [BACKUP_SCRIPT, CHECK_SCRIPT] {
            assert!(script.contains(r#"for "#) && script.contains(r#""$@""#), "names arrive as positional arguments");
        }
        assert!(LOAD_SCRIPT.contains(r#"f="$2"; schema="$3""#), "names arrive as positional arguments");
        for script in [BACKUP_SCRIPT, CHECK_SCRIPT, LOAD_SCRIPT] {
            assert!(!script.contains("{schema}") && !script.contains("{file}") && !script.contains("{}"), "no placeholder is filled in by pasting a name into the text");
        }
        assert!(CHECK_SCRIPT.contains("gzip -t") && CHECK_SCRIPT.contains("Dump completed"), "a cut-short dump is refused");
        assert!(LOAD_SCRIPT.contains(r#""$dir/$f""#) && LOAD_SCRIPT.contains(r#""$schema""#));
    }
}
