//! Reset (Step 9): return an instance to the state it had before its database existed. This is the
//! **only** thing in the app that deletes data, so it is deliberately hard to trigger by accident
//! and slow to be sure about:
//!
//! * the person must type the instance's id; the check is made here, not only in the page;
//! * the servers are stopped cleanly first, so the world is saved;
//! * every database is backed up to `Backups` first, and if that backup cannot be made **nothing is deleted**;
//! * only what is listed is removed: the database (its Docker volume), its credentials, the generated
//!   server configuration, the server logs, and, when asked, the extracted game data.
//!
//! What is never removed: the downloaded source, the built images, the backups (they are the way back),
//! and the person's settings and chosen game client.

use crate::{
    backups, builder,
    compose,
    config,
    database::{self, Credentials, CREDENTIALS_FILE},
    extractor,
    paths::InstancePaths,
    process::CommandSpec,
    recipe::Recipe,
    runtime::{self, ServerState},
    tasks::Task,
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResetItem {
    pub id: String,
    pub label: String,
    pub detail: String,
    /// Whether there is anything to delete.
    pub exists: bool,
    /// Size on this machine, when it is a folder here. Volumes inside Docker have no size shown.
    pub bytes: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ResetPreview {
    /// What has to be typed to confirm.
    pub phrase: String,
    pub items: Vec<ResetItem>,
    /// What a reset never touches, in words.
    pub kept: Vec<String>,
    /// Why a reset cannot run right now, if it cannot.
    pub blocked: Option<String>,
}

/// What exists now, read from Docker and the disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Existing {
    pub database_volume: bool,
    pub game_data_volume: bool,
    pub credentials: bool,
    pub config_bytes: Option<u64>,
    pub logs_bytes: Option<u64>,
}

/// What must be typed: the instance id, so the wrong instance cannot be confirmed by habit.
pub fn phrase(instance: &str) -> String {
    instance.to_string()
}

/// Whether what was typed confirms a reset of this instance. Exact, apart from surrounding spaces.
pub fn confirmed(instance: &str, typed: &str) -> bool {
    typed.trim() == phrase(instance)
}

pub fn items(now: &Existing) -> Vec<ResetItem> {
    vec![
        ResetItem {
            id: "database".into(),
            label: "Database".into(),
            detail: "Every account, character and change to the world. A full backup is taken first and kept in Backups.".into(),
            exists: now.database_volume,
            bytes: None,
        },
        ResetItem {
            id: "credentials".into(),
            label: "Database credentials".into(),
            detail: "The generated database passwords. New ones are made when the database is set up again.".into(),
            exists: now.credentials,
            bytes: None,
        },
        ResetItem {
            id: "configuration".into(),
            label: "Server configuration".into(),
            detail: "The generated server configuration files, including any change you made to them by hand.".into(),
            exists: now.config_bytes.is_some(),
            bytes: now.config_bytes,
        },
        ResetItem {
            id: "logs".into(),
            label: "Server logs".into(),
            detail: "The log files the servers wrote.".into(),
            exists: now.logs_bytes.is_some(),
            bytes: now.logs_bytes,
        },
        ResetItem {
            id: "gameData".into(),
            label: "Extracted game data".into(),
            detail: "Maps and other data extracted from the game client. It takes a long time to extract again, so it is only deleted when you tick this.".into(),
            exists: now.game_data_volume,
            bytes: None,
        },
    ]
}

fn kept() -> Vec<String> {
    [
        "The downloaded server source",
        "The built server images",
        "Your backups, including the one a reset takes first",
        "Your settings and the game client you chose",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Total size of the files under a folder, or `None` when it does not exist. Links are not followed.
pub fn folder_bytes(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_dir() {
        return Some(meta.len());
    }
    let mut total = 0u64;
    for entry in std::fs::read_dir(path).ok()?.flatten() {
        total = total.saturating_add(folder_bytes(&entry.path()).unwrap_or(0));
    }
    Some(total)
}

/// The files and folders a reset removes from this machine, whichever of them exist.
pub fn files_to_remove(paths: &InstancePaths) -> Vec<PathBuf> {
    [
        paths.config.join(CREDENTIALS_FILE),
        paths.config.join(compose::DATABASE_ENV_FILE),
        config::config_dir(paths),
        config::logs_dir(paths),
    ]
    .into_iter()
    .filter(|p| std::fs::symlink_metadata(p).is_ok())
    .collect()
}

/// Deletes [`files_to_remove`]. A path outside the instance's own folder is refused, so a wrong
/// value can never delete something elsewhere.
pub fn remove_files(task: &mut Task, paths: &InstancePaths) -> Result<(), String> {
    for path in files_to_remove(paths) {
        if !path.starts_with(&paths.root) {
            return Err(format!("Refusing to delete {}: it is outside this instance's folder.", path.display()));
        }
        let gone = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        gone.map_err(|e| format!("Cannot delete {}: {e}", path.display()))?;
        task.info(&format!("Deleted {}.", path.strip_prefix(&paths.root).unwrap_or(&path).display()));
    }
    Ok(())
}

async fn existing(paths: &InstancePaths, instance: &str) -> Existing {
    Existing {
        database_volume: database::volume_exists(instance).await,
        game_data_volume: extractor::volume_exists(&extractor::volume_name(instance)).await,
        credentials: paths.config.join(CREDENTIALS_FILE).is_file(),
        config_bytes: folder_bytes(&config::config_dir(paths)),
        logs_bytes: folder_bytes(&config::logs_dir(paths)),
    }
}

pub async fn preview(paths: &InstancePaths, instance: &str, _recipe: &Recipe) -> ResetPreview {
    let phrase = phrase(instance);
    if let Err(reason) = builder::engine().await {
        return ResetPreview { phrase, items: items(&Existing::default()), kept: kept(), blocked: Some(reason) };
    }
    let now = existing(paths, instance).await;
    let list = items(&now);
    let blocked = (!list.iter().any(|i| i.exists)).then(|| "There is nothing to reset yet.".to_string());
    ResetPreview { phrase, items: list, kept: kept(), blocked }
}

async fn remove_volume(task: &mut Task, volume: &str) -> Result<(), String> {
    task.run(CommandSpec::new("docker", &["volume", "rm", volume]).timeout(Duration::from_secs(60)))
        .await
        .map_err(|e| format!("Could not delete the Docker volume {volume}: {e}"))
}

/// Stops the servers so the world is saved and nothing writes while the database is read.
async fn stop_servers(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    let seen = runtime::status(paths, instance, recipe).await;
    match seen.state {
        ServerState::Stopped => Ok(()),
        ServerState::Unknown => Err(seen.note.unwrap_or_else(|| "Docker cannot be asked whether the servers are running.".to_string())),
        _ => {
            if let Err(why) = runtime::stop(task, paths, instance, recipe).await {
                // Stopped but not cleanly is still stopped: the backup below keeps what the database holds.
                if runtime::status(paths, instance, recipe).await.state != ServerState::Stopped {
                    return Err(format!("{why} Nothing was deleted."));
                }
                task.info(&format!("Warning: {why}"));
            }
            Ok(())
        }
    }
}

/// Backs every database up and checks the files are on this machine. Nothing has been deleted yet, so
/// any failure here ends the reset with everything intact.
async fn safety_backup(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    let creds = Credentials::load(paths)?.ok_or_else(|| {
        "The database exists but its credentials are missing, so it cannot be backed up. Nothing was deleted. \
         If you accept losing that data, delete the Docker volume in Docker Desktop and reset again."
            .to_string()
    })?;
    for secret in creds.secrets() {
        task.add_secret(secret);
    }
    paths.ensure()?;
    let opened = backups::open_database(task, paths, instance, recipe).await.map_err(|e| format!("{e} Nothing was deleted."))?;
    let result = backups::backup_everything(task, instance, recipe, &creds, backups::BEFORE_RESET).await;
    backups::close_database(task, paths, instance, recipe, opened).await;
    let files = result.map_err(|e| format!("{e} Nothing was deleted."))?;
    if files.is_empty() {
        task.info("The database holds none of this server's databases yet, so there was nothing to back up.");
    }
    for file in &files {
        let path = paths.backups.join(file);
        match std::fs::metadata(&path) {
            Ok(m) if m.is_file() && m.len() > 0 => {}
            _ => return Err(format!("The backup file {} is not on this machine, so nothing was deleted.", path.display())),
        }
    }
    Ok(())
}

/// Resets the instance. `typed` must be the confirmation phrase; `game_data` also deletes the
/// extracted game data.
pub async fn reset(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe, typed: &str, game_data: bool) -> Result<(), String> {
    if !confirmed(instance, typed) {
        return Err(format!("Not confirmed: type {} exactly. Nothing was deleted.", phrase(instance)));
    }
    task.step("Checking Docker");
    builder::engine().await?;
    let now = existing(paths, instance).await;
    if !items(&now).iter().any(|i| i.exists && (i.id != "gameData" || game_data)) {
        return Err("There is nothing to reset yet.".to_string());
    }

    stop_servers(task, paths, instance, recipe).await?;
    if now.database_volume {
        safety_backup(task, paths, instance, recipe).await?;
    }

    task.step("Removing the containers");
    let file = compose::write(paths, recipe, instance)?;
    task.run(runtime::compose_spec(paths, instance, &file, &["down", "--remove-orphans"], Duration::from_secs(5 * 60)))
        .await
        .map_err(|e| format!("The containers could not be removed: {e} The backup is safe in Backups; nothing else was deleted."))?;

    if now.database_volume {
        task.step("Deleting the database");
        remove_volume(task, &compose::database_volume(instance)).await?;
    }
    if game_data && now.game_data_volume {
        task.step("Deleting the extracted game data");
        remove_volume(task, &extractor::volume_name(instance)).await?;
    }
    task.step("Deleting the credentials, configuration and logs");
    remove_files(task, paths)?;

    let after = existing(paths, instance).await;
    let left: Vec<&str> = [
        (after.database_volume, "the database"),
        (game_data && after.game_data_volume, "the game data"),
        (after.credentials || after.config_bytes.is_some() || after.logs_bytes.is_some(), "some files"),
    ]
    .iter()
    .filter_map(|(still, what)| still.then_some(*what))
    .collect();
    if !left.is_empty() {
        return Err(format!("The reset finished, but {} still exist{}.", left.join(" and "), if left.len() == 1 { "s" } else { "" }));
    }
    task.outcome("The instance was reset. Your backups are in Backups; set the server up again from the Setup page, then restore one if you want your data back.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_instance_id_confirms_a_reset() {
        assert!(confirmed("mangos-zero", "mangos-zero"));
        assert!(confirmed("mangos-zero", "  mangos-zero \n"));
        for wrong in ["", "mangos", "MANGOS-ZERO", "mangos-zero!", "mangos zero", "reset", "mangos-zero mangos-zero"] {
            assert!(!confirmed("mangos-zero", wrong), "{wrong:?}");
        }
        assert!(!confirmed("mangos-one", "mangos-zero"), "another instance's id does not confirm this one");
    }

    #[test]
    fn the_list_says_what_exists_and_sizes_only_what_is_a_folder_here() {
        let list = items(&Existing { database_volume: true, game_data_volume: false, credentials: true, config_bytes: Some(2048), logs_bytes: None });
        let by = |id: &str| list.iter().find(|i| i.id == id).unwrap();
        assert!(by("database").exists && by("database").bytes.is_none());
        assert!(by("credentials").exists);
        assert_eq!((by("configuration").exists, by("configuration").bytes), (true, Some(2048)));
        assert_eq!((by("logs").exists, by("logs").bytes), (false, None));
        assert!(!by("gameData").exists);
        assert!(by("gameData").detail.contains("only deleted when you tick"));
        assert!(by("database").detail.contains("backup is taken first"));
        assert!(items(&Existing::default()).iter().all(|i| !i.exists), "an empty instance has nothing to reset");
    }

    #[test]
    fn what_is_never_deleted_is_told_to_the_person() {
        let text = kept().join(" | ");
        for word in ["source", "images", "backups", "settings"] {
            assert!(text.contains(word), "{word}: {text}");
        }
    }

    #[test]
    fn folder_sizes_add_up_and_a_missing_folder_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(folder_bytes(&dir.path().join("nope")), None);
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/one"), [0u8; 10]).unwrap();
        std::fs::write(dir.path().join("a/b/two"), [0u8; 32]).unwrap();
        assert_eq!(folder_bytes(&dir.path().join("a")), Some(42));
    }

    /// An instance folder holding everything a reset must remove and everything it must keep.
    fn instance() -> (tempfile::TempDir, InstancePaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        paths.ensure().unwrap();
        for file in [CREDENTIALS_FILE, compose::DATABASE_ENV_FILE, "settings.json", "client.json"] {
            std::fs::write(paths.config.join(file), "x").unwrap();
        }
        std::fs::create_dir_all(config::config_dir(&paths)).unwrap();
        std::fs::write(config::config_dir(&paths).join("mangosd.conf"), "edited by hand").unwrap();
        std::fs::create_dir_all(config::logs_dir(&paths)).unwrap();
        std::fs::write(config::logs_dir(&paths).join("Server.log"), "log").unwrap();
        std::fs::write(paths.backups.join("realmd-20260929-101500-before-reset.sql.gz"), "backup").unwrap();
        std::fs::create_dir_all(paths.repos.join("server")).unwrap();
        std::fs::write(paths.repos.join("server/README"), "source").unwrap();
        (dir, paths)
    }

    #[test]
    fn the_files_a_reset_removes_are_exactly_the_credentials_configuration_and_logs() {
        let (_keep, paths) = instance();
        let listed: Vec<String> = files_to_remove(&paths).iter().map(|p| p.strip_prefix(&paths.root).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(listed, ["Config/database.json", "Config/database.env", "Config/server", "Logs/server"]);
    }

    #[tokio::test]
    async fn removing_the_files_keeps_backups_source_settings_and_the_chosen_client() {
        let (keep, paths) = instance();
        let (manager, _) = crate::tasks::test_support::manager(keep.path());
        let mut task = manager.begin("Reset", false).unwrap();
        remove_files(&mut task, &paths).unwrap();
        task.finish(Ok(()));

        assert!(!paths.config.join(CREDENTIALS_FILE).exists() && !paths.config.join(compose::DATABASE_ENV_FILE).exists());
        assert!(!config::config_dir(&paths).exists() && !config::logs_dir(&paths).exists());
        assert!(paths.config.join("settings.json").is_file() && paths.config.join("client.json").is_file(), "the person's choices stay");
        assert!(paths.backups.join("realmd-20260929-101500-before-reset.sql.gz").is_file(), "the way back stays");
        assert!(paths.repos.join("server/README").is_file(), "the source stays");
        assert!(files_to_remove(&paths).is_empty(), "a second reset has nothing left to remove");
    }

    #[test]
    fn nothing_is_listed_when_nothing_exists() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        assert!(files_to_remove(&paths).is_empty());
    }
}
