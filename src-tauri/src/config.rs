//! The servers' configuration files (Step 5).
//!
//! Each file starts as the sample the build installs and is kept in the instance's `Config/server`
//! folder, which the containers see read-only. WorldStone owns the keys the recipe lists and
//! nothing else: they are set to the values the instance's credentials and settings call for,
//! every other line stays exactly as the person left it, and a file that already exists is edited
//! in place, never regenerated. A managed key that has drifted (a changed password, a changed bot
//! count, a hand edit) is put back by the next Configure and shown as out of date until then.

use crate::{
    builder,
    compose,
    database::Credentials,
    docker,
    paths::InstancePaths,
    process::{capture, CommandSpec},
    recipe::{ConfigFile, Recipe, KEPT_SOURCE_FOLDER},
    settings::{self, Settings},
    tasks::Task,
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Folder in `Config` holding the generated files; the containers mount it read-only.
pub const CONFIG_FOLDER: &str = "server";
/// Folder in `Logs` the servers write to.
pub const LOGS_FOLDER: &str = "server";

/// Where a managed file is kept: in its own folder, when it has one, so the path reads the same on
/// every system.
fn stored_path(dir: &Path, file: &ConfigFile) -> PathBuf {
    let mut path = dir.to_path_buf();
    if let Some(folder) = &file.folder {
        path.push(folder);
    }
    path.push(file.file_name());
    path
}

pub fn config_dir(paths: &InstancePaths) -> PathBuf {
    paths.config.join(CONFIG_FOLDER)
}

pub fn logs_dir(paths: &InstancePaths) -> PathBuf {
    paths.logs.join(LOGS_FOLDER)
}

// ---------------------------------------------------------------------------------------
// Editing a configuration file's text
// ---------------------------------------------------------------------------------------

/// `Key = value` split into the text up to and including the `=` and its spacing, and the value.
fn split_assignment(line: &str) -> Option<(&str, &str, &str)> {
    let eq = line.find('=')?;
    let key = line[..eq].trim();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    let after = &line[eq + 1..];
    let value = after.trim_start();
    let spacing = &after[..after.len() - value.len()];
    Some((key, &line[..eq + 1 + spacing.len()], value.trim_end()))
}

/// The assignment on an active (not commented-out) line, if this is one.
fn active(line: &str) -> Option<(&str, &str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with('[') {
        return None;
    }
    split_assignment(line)
}

/// A commented-out assignment (`#Key = default`), which is how the samples show optional keys.
fn commented(line: &str) -> Option<(&str, &str, &str)> {
    let trimmed = line.trim_start();
    let body = trimmed.strip_prefix('#')?;
    let (key, head, value) = split_assignment(body)?;
    // Prose such as "# Example: a = b" has a colon or a space in the key part; keys never do.
    (!key.contains([':', '"', ',', '(']) && !head.is_empty()).then_some((key, head, value))
}

/// The value of the first active line for `key`, as written (surrounding quotes included).
pub fn read_key(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(active)
        .find(|(k, ..)| k.eq_ignore_ascii_case(key))
        .map(|(.., value)| value.to_string())
}

/// The line without its ending, and the ending (LF, CRLF or nothing).
fn split_eol(line: &str) -> (&str, &str) {
    let content = line.trim_end_matches(['\n', '\r']);
    (content, &line[content.len()..])
}

/// Sets `key` to `value`. Every active line for the key is changed; failing that, the first
/// commented-out sample line becomes the setting; failing that, the key is added at the end.
/// Line endings and the spacing before the value are kept.
pub fn set_key(text: &str, key: &str, value: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let is_key = |k: &str| k.eq_ignore_ascii_case(key);
    let has_active = lines.iter().any(|l| active(split_eol(l).0).is_some_and(|(k, ..)| is_key(k)));
    let sample = if has_active {
        None
    } else {
        lines.iter().position(|l| commented(split_eol(l).0).is_some_and(|(k, ..)| is_key(k)))
    };

    let mut out = String::with_capacity(text.len() + key.len() + value.len() + 8);
    for (index, line) in lines.iter().enumerate() {
        let (content, eol) = split_eol(line);
        match (active(content), sample == Some(index)) {
            (Some((k, head, _)), _) if is_key(k) => out.push_str(&format!("{head}{value}{eol}")),
            (_, true) => {
                let (_, head, _) = commented(content).expect("the sample line was found by this test");
                let lead = &content[..content.len() - content.trim_start().len()];
                out.push_str(&format!("{lead}{}{value}{eol}", head.trim_start()));
            }
            _ => out.push_str(line),
        }
    }
    if has_active || sample.is_some() {
        return out;
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("{key} = {value}\n"));
    out
}

// ---------------------------------------------------------------------------------------
// What the managed keys should be
// ---------------------------------------------------------------------------------------

/// Everything a value can refer to. Holds the database password, so it is never printed.
pub struct Values<'a> {
    pub recipe: &'a Recipe,
    pub credentials: &'a Credentials,
    pub settings: &'a Settings,
}

impl Values<'_> {
    fn fill(&self, template: &str, service_port: u16) -> String {
        let run = &self.recipe.run;
        let bots = &self.settings.bots;
        [
            ("{db_host}", compose::DATABASE_SERVICE.to_string()),
            ("{db_port}", compose::DATABASE_PORT.to_string()),
            ("{db_user}", self.credentials.user.clone()),
            ("{db_password}", self.credentials.user_password.clone()),
            ("{data_dir}", format!("{}/{}/", run.data_mount, run.data_folder)),
            ("{logs_dir}", format!("{}/", run.logs_mount)),
            ("{source_dir}", format!("{}/{KEPT_SOURCE_FOLDER}", self.recipe.build.install_prefix)),
            ("{port}", service_port.to_string()),
            ("{realm_id}", self.settings.realm_id.to_string()),
            ("{player_limit}", self.settings.player_limit().to_string()),
            ("{bots_enabled}", if bots.enabled { "1" } else { "0" }.to_string()),
            ("{bots_min}", bots.min.to_string()),
            ("{bots_max}", bots.max.to_string()),
            ("{bots_accounts}", self.settings.bot_accounts(self.recipe.playerbots.classes_per_account).to_string()),
        ]
        .iter()
        .fold(template.to_string(), |text, (token, value)| text.replace(token, value))
    }

    /// The keys WorldStone owns in one file, with the values they must have.
    pub fn managed(&self, file: &ConfigFile) -> Vec<(String, String)> {
        let port = file
            .service
            .as_ref()
            .and_then(|id| self.recipe.run.services.iter().find(|s| &s.id == id))
            .map_or(0, |s| s.port);
        file.set.iter().map(|(key, value)| (key.clone(), self.fill(value, port))).collect()
    }
}

/// The file's text with every managed key set. Nothing else changes.
pub fn apply(text: &str, managed: &[(String, String)]) -> String {
    managed.iter().fold(text.to_string(), |text, (key, value)| set_key(&text, key, value))
}

/// The managed keys whose value in `text` is not the one wanted.
pub fn stale_keys(text: &str, managed: &[(String, String)]) -> Vec<String> {
    managed
        .iter()
        .filter(|(key, value)| read_key(text, key).as_deref() != Some(value.as_str()))
        .map(|(key, _)| key.clone())
        .collect()
}

// ---------------------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum FileState {
    Missing,
    /// The file exists but some managed key does not have the wanted value.
    Outdated,
    Ready,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileStatus {
    pub id: String,
    pub label: String,
    /// Where the file is, for the person to open and edit.
    pub path: String,
    pub state: FileState,
    /// Managed keys that are not as wanted. Names only, never values.
    pub stale_keys: Vec<String>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ConfigState {
    /// The settings or credentials could not be read.
    Unknown,
    /// Nothing has been generated yet.
    NotConfigured,
    /// Some file is missing or some managed key is not as wanted.
    Outdated,
    Ready,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConfigStatus {
    pub state: ConfigState,
    /// Why Configure cannot run right now, if it cannot.
    pub blocked: Option<String>,
    pub note: Option<String>,
    pub files: Vec<FileStatus>,
    /// Bots as configured, for the panels that show them.
    pub bots: Option<BotsConfigured>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BotsConfigured {
    pub enabled: bool,
    pub min: u32,
    pub max: u32,
    pub accounts: u32,
    pub player_limit: u32,
}

/// Reads what is on disk; starts nothing and needs no Docker.
pub fn status(paths: &InstancePaths, recipe: &Recipe) -> ConfigStatus {
    let dir = config_dir(paths);
    let file_at = |file: &ConfigFile| stored_path(&dir, file);
    let path_of = |file: &ConfigFile| file_at(file).to_string_lossy().into_owned();
    let none = |state, blocked: Option<String>, note: Option<String>| ConfigStatus {
        state,
        blocked,
        note,
        files: recipe
            .config
            .files
            .iter()
            .map(|f| FileStatus {
                id: f.id.clone(),
                label: f.label.clone(),
                path: path_of(f),
                state: if file_at(f).exists() { FileState::Outdated } else { FileState::Missing },
                stale_keys: Vec::new(),
            })
            .collect(),
        bots: None,
    };

    let credentials = match Credentials::load(paths) {
        Ok(Some(c)) => c,
        Ok(None) => return none(ConfigState::NotConfigured, Some("Set up the database first: its passwords are written into the configuration.".into()), None),
        Err(e) => return none(ConfigState::Unknown, Some(e.clone()), Some(e)),
    };
    let settings = match settings::load(paths) {
        Ok(Some(s)) => s,
        Ok(None) => return none(ConfigState::NotConfigured, None, None),
        Err(e) => return none(ConfigState::Unknown, Some(e.clone()), Some(e)),
    };
    let values = Values { recipe, credentials: &credentials, settings: &settings };

    let files: Vec<FileStatus> = recipe
        .config
        .files
        .iter()
        .map(|file| {
            let (state, stale) = match std::fs::read_to_string(file_at(file)) {
                Err(_) => (FileState::Missing, Vec::new()),
                Ok(text) => {
                    let stale = stale_keys(&text, &values.managed(file));
                    (if stale.is_empty() { FileState::Ready } else { FileState::Outdated }, stale)
                }
            };
            FileStatus { id: file.id.clone(), label: file.label.clone(), path: path_of(file), state, stale_keys: stale }
        })
        .collect();
    let state = if files.iter().all(|f| f.state == FileState::Ready) {
        ConfigState::Ready
    } else if files.iter().all(|f| f.state == FileState::Missing) {
        ConfigState::NotConfigured
    } else {
        ConfigState::Outdated
    };
    ConfigStatus {
        state,
        blocked: None,
        note: None,
        files,
        bots: Some(BotsConfigured {
            enabled: settings.bots.enabled,
            min: settings.bots.min,
            max: settings.bots.max,
            accounts: settings.bot_accounts(recipe.playerbots.classes_per_account),
            player_limit: settings.player_limit(),
        }),
    }
}

// ---------------------------------------------------------------------------------------
// Generating
// ---------------------------------------------------------------------------------------

/// The sample file inside the built image, which is what the running server expects to find.
async fn read_template(instance: &str, recipe: &Recipe, file: &ConfigFile) -> Result<String, String> {
    let image = builder::tools_image(instance, recipe);
    let inside = format!("{}/{}", recipe.build.install_prefix, file.template);
    let spec = CommandSpec::new("docker", &["run", "--rm", "--entrypoint", "cat", &image, &inside]).timeout(Duration::from_secs(120));
    let read = capture(&spec).await;
    if !read.success || !read.text.contains('=') {
        return Err(format!("Could not read {inside} from the server image: {}", read.text.trim()));
    }
    Ok(read.text)
}

/// Writes the file through a temporary name so a crash cannot leave half a configuration, and
/// keeps the previous version next to it.
fn replace_file(path: &std::path::Path, text: &str) -> Result<(), String> {
    if path.exists() {
        let previous = path.with_extension("conf.previous");
        std::fs::copy(path, &previous).map_err(|e| format!("Cannot keep a copy of {}: {e}", path.display()))?;
    }
    let partial = path.with_extension("conf.partial");
    std::fs::write(&partial, text).map_err(|e| format!("Cannot write {}: {e}", partial.display()))?;
    std::fs::rename(&partial, path).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

/// Creates the configuration files, or brings the managed keys of existing ones back to what the
/// instance's credentials and settings call for. Safe to run again at any time.
pub async fn configure(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    task.step("Checking Docker and the server image");
    let engine = builder::engine().await?;
    let image = builder::tools_image(instance, recipe);
    if docker::image(&image).await.is_none() {
        return Err("Build the server first: its sample configuration files come from the built image.".to_string());
    }
    let credentials = Credentials::load(paths)?
        .ok_or_else(|| "Set up the database first: its passwords are written into the configuration.".to_string())?;
    for secret in credentials.secrets() {
        task.add_secret(secret);
    }
    paths.ensure()?;
    let settings = settings::load_or_create(paths, &engine)?;
    let values = Values { recipe, credentials: &credentials, settings: &settings };

    let dir = config_dir(paths);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    let logs = logs_dir(paths);
    std::fs::create_dir_all(&logs).map_err(|e| format!("Cannot create {}: {e}", logs.display()))?;

    for file in &recipe.config.files {
        task.step(&format!("Writing the {} configuration", file.label.to_lowercase()));
        let path = stored_path(&dir, file);
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|e| format!("Cannot create {}: {e}", folder.display()))?;
        }
        let existing = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
        };
        let managed = values.managed(file);
        let base = match &existing {
            Some(text) => text.clone(),
            None => read_template(instance, recipe, file).await?,
        };
        let text = apply(&base, &managed);
        match &existing {
            None => {
                replace_file(&path, &text)?;
                task.info(&format!("Created {}", file.file_name()));
            }
            Some(old) if *old != text => {
                let stale = stale_keys(old, &managed);
                replace_file(&path, &text)?;
                task.info(&format!("Updated {}: {}", file.file_name(), stale.join(", ")));
            }
            Some(_) => task.info(&format!("{} is already as it should be", file.file_name())),
        }
    }

    task.step("Checking the result");
    let after = status(paths, recipe);
    if after.state != ConfigState::Ready {
        let names: Vec<String> = after.files.iter().filter(|f| f.state != FileState::Ready).map(|f| f.label.clone()).collect();
        return Err(format!("The configuration is still not right after writing it: {}.", names.join(", ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero() -> Recipe {
        crate::recipe::builtin().unwrap().remove(0)
    }

    fn credentials() -> Credentials {
        Credentials { root_password: "r".repeat(64), user: "mangos".into(), user_password: "p".repeat(64), engine: Default::default() }
    }

    fn settings() -> Settings {
        Settings::for_machine(&builder::Engine { cpus: 12, memory: 16 << 30 })
    }

    const SAMPLE: &str = "[MangosdConf]\nConfVersion=1\n\n# The realm\n#    RealmID\n#        must match\nRealmID                      = 1\nDataDir                      = \"/mangos/etc\"   \nPlayerLimit = 100\n#AiPlayerbot.MinRandomBots = 50\nMotd = \"Hello = there\"\n";

    #[test]
    fn a_value_is_read_from_the_first_active_line_only() {
        assert_eq!(read_key(SAMPLE, "RealmID").as_deref(), Some("1"));
        assert_eq!(read_key(SAMPLE, "DataDir").as_deref(), Some("\"/mangos/etc\""), "trailing spaces are not part of the value");
        assert_eq!(read_key(SAMPLE, "realmid").as_deref(), Some("1"), "keys are not case sensitive");
        assert_eq!(read_key(SAMPLE, "AiPlayerbot.MinRandomBots"), None, "a commented-out sample is not a setting");
        assert_eq!(read_key(SAMPLE, "Motd").as_deref(), Some("\"Hello = there\""), "only the first = splits");
        assert_eq!(read_key(SAMPLE, "MangosdConf"), None, "a section header is not a key");
    }

    #[test]
    fn setting_a_key_changes_that_line_and_nothing_else() {
        let out = set_key(SAMPLE, "PlayerLimit", "196");
        assert_eq!(out, SAMPLE.replace("PlayerLimit = 100", "PlayerLimit = 196"));
        let out = set_key(SAMPLE, "RealmID", "2");
        assert!(out.contains("RealmID                      = 2\n"), "the alignment is kept: {out}");
        assert!(out.contains("Motd = \"Hello = there\""));
    }

    #[test]
    fn a_commented_sample_becomes_the_setting_and_a_missing_key_is_appended() {
        let out = set_key(SAMPLE, "AiPlayerbot.MinRandomBots", "48");
        assert!(out.contains("\nAiPlayerbot.MinRandomBots = 48\n") && !out.contains("#AiPlayerbot.MinRandomBots"), "{out}");
        assert_eq!(out.matches("MinRandomBots").count(), 1);
        let out = set_key(SAMPLE, "Console.Enable", "1");
        assert!(out.ends_with("Motd = \"Hello = there\"\nConsole.Enable = 1\n"), "{out}");
        assert!(set_key("A = 1", "B", "2").ends_with("A = 1\nB = 2\n"), "a file without a final newline is still handled");
    }

    #[test]
    fn prose_in_comments_is_never_taken_for_a_sample() {
        let prose = "#    Example: \"/mangos/share\" = the default\n";
        assert!(set_key(prose, "Example", "1").ends_with("\nExample = 1\n"), "a prose line does not count as a sample");
        assert!(set_key(prose, "Example", "1").starts_with(prose), "and it is left as it was");
        assert_eq!(set_key("#Key = 5\n", "Key", "6"), "Key = 6\n", "a bare commented key does");
    }

    #[test]
    fn windows_line_endings_survive() {
        let text = "A = 1\r\nB = 2\r\n";
        assert_eq!(set_key(text, "B", "3"), "A = 1\r\nB = 3\r\n");
    }

    #[test]
    fn managed_values_are_filled_in_from_the_instance() {
        let recipe = zero();
        let (creds, settings) = (credentials(), settings());
        let values = Values { recipe: &recipe, credentials: &creds, settings: &settings };
        let world = recipe.config.files.iter().find(|f| f.id == "mangosd").unwrap();
        let managed: std::collections::HashMap<_, _> = values.managed(world).into_iter().collect();
        assert_eq!(managed["WorldDatabaseInfo"], format!("\"database;3306;mangos;{};mangos0\"", creds.user_password));
        assert_eq!(managed["DataDir"], "\"/worldstone/data/current/\"");
        assert_eq!(managed["WorldServerPort"], "8085");
        assert_eq!(managed["RealmID"], "1");
        assert_eq!(managed["PlayerLimit"], "196");
        let bots = recipe.config.files.iter().find(|f| f.id == "aiplayerbot").unwrap();
        let managed: std::collections::HashMap<_, _> = values.managed(bots).into_iter().collect();
        assert_eq!(managed["AiPlayerbot.Enabled"], "1");
        assert_eq!(managed["AiPlayerbot.MinRandomBots"], "48");
        assert_eq!(managed["AiPlayerbot.RandomBotAccountCount"], "16");
        assert!(managed.values().all(|v| !v.contains('{')), "an unfilled placeholder would reach the file: {managed:?}");
    }

    #[test]
    fn every_placeholder_the_recipe_uses_is_one_the_engine_fills() {
        let recipe = zero();
        let (creds, settings) = (credentials(), settings());
        let values = Values { recipe: &recipe, credentials: &creds, settings: &settings };
        for file in &recipe.config.files {
            for (key, value) in values.managed(file) {
                assert!(!value.contains('{') && !value.contains('}'), "{}: {key} = {value}", file.id);
            }
        }
    }

    #[test]
    fn applying_keeps_hand_edits_and_rewrites_only_drifted_managed_keys() {
        let recipe = zero();
        let (creds, settings) = (credentials(), settings());
        let values = Values { recipe: &recipe, credentials: &creds, settings: &settings };
        let world = recipe.config.files.iter().find(|f| f.id == "mangosd").unwrap();
        let managed = values.managed(world);

        let first = apply("Motd = \"Welcome\"\nMaxPingTime = 5\nRealmID = 1\nPlayerLimit = 100\n", &managed);
        assert!(stale_keys(&first, &managed).is_empty(), "everything managed is right after applying");

        // The person edits their own key and someone changes a managed one.
        let edited = first.replace("Motd = \"Welcome\"", "Motd = \"My realm\"").replace("PlayerLimit = 196", "PlayerLimit = 5");
        assert_eq!(stale_keys(&edited, &managed), ["PlayerLimit"]);
        let again = apply(&edited, &managed);
        assert!(again.contains("Motd = \"My realm\""), "their edit survives: {again}");
        assert!(again.contains("PlayerLimit = 196") && stale_keys(&again, &managed).is_empty());
        assert_eq!(apply(&again, &managed), again, "applying twice changes nothing");
    }

    #[test]
    fn a_changed_password_makes_the_database_lines_stale_without_naming_it() {
        let recipe = zero();
        let settings = settings();
        let (old, new) = (credentials(), Credentials { user_password: "n".repeat(64), ..credentials() });
        let world = recipe.config.files.iter().find(|f| f.id == "mangosd").unwrap();
        let written = apply("", &Values { recipe: &recipe, credentials: &old, settings: &settings }.managed(world));
        let stale = stale_keys(&written, &Values { recipe: &recipe, credentials: &new, settings: &settings }.managed(world));
        assert_eq!(stale, ["CharacterDatabaseInfo", "LoginDatabaseInfo", "WorldDatabaseInfo"]);
        assert!(stale.iter().all(|k| !k.contains(&new.user_password)));
    }

    #[test]
    fn status_walks_from_nothing_to_ready_and_notices_drift() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        paths.ensure().unwrap();
        let recipe = zero();

        let s = status(&paths, &recipe);
        assert_eq!(s.state, ConfigState::NotConfigured);
        assert!(s.blocked.unwrap().contains("database"), "no passwords yet, so nothing can be written");

        let creds = Credentials::load_or_create(&paths, "mangos", Default::default()).unwrap();
        assert_eq!(status(&paths, &recipe).state, ConfigState::NotConfigured);
        assert_eq!(status(&paths, &recipe).blocked, None, "the database is set up, so Configure may run");

        let settings = settings::load_or_create(&paths, &builder::Engine { cpus: 12, memory: 16 << 30 }).unwrap();
        let values = Values { recipe: &recipe, credentials: &creds, settings: &settings };
        std::fs::create_dir_all(config_dir(&paths)).unwrap();
        for file in &recipe.config.files {
            std::fs::write(config_dir(&paths).join(file.file_name()), apply("[Conf]\n", &values.managed(file))).unwrap();
        }
        let s = status(&paths, &recipe);
        assert_eq!(s.state, ConfigState::Ready, "{s:?}");
        assert_eq!(s.bots.as_ref().unwrap().max, 96);

        // Raising the bot count in the settings makes the files out of date until Configure runs.
        let mut tuned = settings.clone();
        tuned.bots.max = 120;
        settings::save(&paths, &tuned).unwrap();
        let s = status(&paths, &recipe);
        assert_eq!(s.state, ConfigState::Outdated);
        let bots = s.files.iter().find(|f| f.id == "aiplayerbot").unwrap();
        assert_eq!(bots.state, FileState::Outdated);
        assert!(bots.stale_keys.contains(&"AiPlayerbot.MaxRandomBots".to_string()));

        std::fs::remove_file(config_dir(&paths).join("realmd.conf")).unwrap();
        assert_eq!(status(&paths, &recipe).files.iter().find(|f| f.id == "realmd").unwrap().state, FileState::Missing);
    }

    #[test]
    fn replacing_a_file_keeps_the_previous_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mangosd.conf");
        replace_file(&path, "one").unwrap();
        assert!(!dir.path().join("mangosd.conf.previous").exists(), "nothing to keep the first time");
        replace_file(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(std::fs::read_to_string(dir.path().join("mangosd.conf.previous")).unwrap(), "one");
        assert!(!dir.path().join("mangosd.conf.partial").exists());
    }
}
