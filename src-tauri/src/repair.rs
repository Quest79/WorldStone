//! Repair (Step 9): look at every part of the instance, name what is wrong, and put right what the
//! app can put right, using the same steps as the buttons of the Setup page.
//!
//! What it is **not**: it never deletes or overwrites data, so a broken database is completed
//! (never reset), the configuration only gets its managed settings back, and a stale-but-working
//! build is left to *Update*. Only things this app made itself and that are useless as they are (a
//! backup that never finished) are removed. What only the person can do (choose the game client)
//! is reported, not guessed.
//!
//! The diagnosis is worked out from the real state each time, both for the list shown on the page
//! and again inside the task, and the task ends by checking again: a repair that did not help says so.

use crate::{
    backups, builder, client,
    config::{self, ConfigState},
    database::{self, DatabaseState},
    docker,
    extractor::{self, ExtractState},
    git::{self, RepoState},
    paths::InstancePaths,
    recipe::Recipe,
    runtime::{self, ServerState},
    tasks::Task,
};
use serde::Serialize;

/// What a fix does, in the order the fixes run: each one may need the one before it.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Fix {
    Source,
    Build,
    Extract,
    Database,
    Configure,
    Leftovers,
    Servers,
}

impl Fix {
    fn label(self) -> &'static str {
        match self {
            Fix::Source => "Download the source",
            Fix::Build => "Build the server",
            Fix::Extract => "Extract the game data",
            Fix::Database => "Set up the database",
            Fix::Configure => "Generate the configuration",
            Fix::Leftovers => "Remove unfinished backup files",
            Fix::Servers => "Restart the servers",
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub title: String,
    pub detail: String,
    /// What repairs it; `None` when only the person can.
    pub fix: Option<Fix>,
    /// What repairing does, in words, for the list.
    pub action: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RepairStatus {
    pub findings: Vec<Finding>,
    /// Something can be repaired by the app.
    pub can_repair: bool,
}

/// What was read from the instance. Kept apart from the reading so the judgement can be tested.
#[derive(Clone, Debug)]
pub struct Facts {
    /// Why Docker cannot be asked, if it cannot.
    pub docker: Option<String>,
    pub repos: Vec<(String, RepoState, String)>,
    pub images_missing: Vec<String>,
    pub database_image_missing: bool,
    pub client_ready: bool,
    pub extract: ExtractState,
    pub database: DatabaseState,
    pub database_note: Option<String>,
    pub config: ConfigState,
    pub config_note: Option<String>,
    pub servers: ServerState,
    pub partial_backups: usize,
}

fn found(title: &str, detail: impl Into<String>, fix: Fix) -> Finding {
    Finding { title: title.to_string(), detail: detail.into(), fix: Some(fix), action: Some(fix.label().to_string()) }
}

fn manual(title: &str, detail: impl Into<String>) -> Finding {
    Finding { title: title.to_string(), detail: detail.into(), fix: None, action: None }
}

/// What is wrong, from what was read. A part that is merely out of date is not wrong: *Update* is
/// for that.
pub fn plan(f: &Facts) -> Vec<Finding> {
    if let Some(why) = &f.docker {
        return vec![manual("Docker is not available", why.as_str())];
    }
    let mut out = Vec::new();
    for (id, state, detail) in &f.repos {
        if *state != RepoState::Ready {
            let what = if *state == RepoState::Missing { "missing" } else { "incomplete" };
            out.push(found(&format!("The {id} source is {what}"), detail.as_str(), Fix::Source));
        }
    }
    if !f.images_missing.is_empty() {
        out.push(found("The server image is missing", format!("Docker has no {}.", f.images_missing.join(" or ")), Fix::Build));
    }
    if f.extract == ExtractState::NotExtracted {
        if f.client_ready {
            out.push(found("The game data has not been extracted", "The world server cannot run without it.", Fix::Extract));
        } else {
            out.push(manual("The game data has not been extracted", "Choose your game client on the Setup page first; the data is extracted from it."));
        }
    }
    match f.database {
        DatabaseState::NotProvisioned | DatabaseState::Incomplete => out.push(found(
            "The database is not complete",
            f.database_note.clone().unwrap_or_else(|| "Some of its tables or data are missing. Setting up completes it without resetting anything.".to_string()),
            Fix::Database,
        )),
        DatabaseState::Unknown => out.push(manual("The database cannot be read", f.database_note.clone().unwrap_or_else(|| "Docker or the database scripts could not be asked.".to_string()))),
        DatabaseState::Stopped | DatabaseState::Starting | DatabaseState::Outdated | DatabaseState::Ready => {
            if f.database_image_missing {
                out.push(found("The database image has not been named for this server", "Setting up the database names it; its data is not touched.", Fix::Database));
            }
        }
    }
    match f.config {
        ConfigState::NotConfigured => out.push(found("The configuration has not been generated", "The servers cannot start without it.", Fix::Configure)),
        ConfigState::Outdated => out.push(found(
            "The configuration is out of date",
            f.config_note.clone().unwrap_or_else(|| "A setting this app manages does not have its intended value.".to_string()),
            Fix::Configure,
        )),
        ConfigState::Unknown => out.push(manual("The configuration cannot be read", f.config_note.clone().unwrap_or_else(|| "The configuration files could not be read.".to_string()))),
        ConfigState::Ready => {}
    }
    if f.partial_backups > 0 {
        out.push(found(
            "Unfinished backup files",
            format!("{} backup{} never finished (a backup was interrupted). They cannot be restored and only take room.", f.partial_backups, if f.partial_backups == 1 { "" } else { "s" }),
            Fix::Leftovers,
        ));
    }
    match f.servers {
        ServerState::Failing => out.push(found("A server crashed or stopped answering", "Restarting stops everything cleanly and starts it again.", Fix::Servers)),
        ServerState::Partial => out.push(found("Only some of the servers are running", "Restarting brings all of them up together.", Fix::Servers)),
        _ => {}
    }
    out
}

/// The fixes to run, once each, in order.
pub fn fixes(findings: &[Finding]) -> Vec<Fix> {
    let mut all: Vec<Fix> = findings.iter().filter_map(|f| f.fix).collect();
    all.sort();
    all.dedup();
    all
}

fn describe(findings: &[Finding]) -> String {
    findings.iter().map(|f| f.title.to_lowercase()).collect::<Vec<_>>().join("; ")
}

/// Reads the instance. Starts and changes nothing.
pub async fn gather(paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Facts {
    let mut facts = Facts {
        docker: None,
        repos: Vec::new(),
        images_missing: Vec::new(),
        database_image_missing: false,
        client_ready: false,
        extract: ExtractState::Unknown,
        database: DatabaseState::Unknown,
        database_note: None,
        config: ConfigState::Unknown,
        config_note: None,
        servers: ServerState::Unknown,
        partial_backups: backups::partial_files(paths).len(),
    };
    if let Err(reason) = builder::engine().await {
        facts.docker = Some(reason);
        return facts;
    }
    for (repo, seen) in recipe.repos.iter().zip(git::inspect_all(paths, recipe, &recipe.repos).await) {
        facts.repos.push((repo.id.clone(), seen.state, seen.detail));
    }
    for service in &recipe.run.services {
        let image = builder::image_ref(instance, recipe, service);
        if docker::image(&image).await.is_none() {
            facts.images_missing.push(image);
        }
    }
    facts.database_image_missing = docker::image(&builder::database_image(instance, recipe)).await.is_none();
    facts.client_ready = client::status(paths, &recipe.client).info.is_some();
    facts.extract = extractor::status(paths, instance, recipe).await.state;
    let db = database::status(paths, instance, recipe).await;
    facts.database = db.state;
    facts.database_note = db.note;
    let configured = config::status(paths, recipe);
    facts.config = configured.state;
    facts.config_note = configured.note;
    facts.servers = runtime::status(paths, instance, recipe).await.state;
    facts
}

pub async fn status(paths: &InstancePaths, instance: &str, recipe: &Recipe) -> RepairStatus {
    let findings = plan(&gather(paths, instance, recipe).await);
    let can_repair = findings.iter().any(|f| f.fix.is_some());
    RepairStatus { findings, can_repair }
}

async fn run_fix(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe, fix: Fix) -> Result<(), String> {
    task.step(fix.label());
    match fix {
        Fix::Source => git::sync_all(task, paths, recipe).await,
        Fix::Build => builder::build(task, paths, instance, recipe, false).await,
        Fix::Extract => extractor::extract(task, paths, instance, recipe).await,
        Fix::Database => database::provision(task, paths, instance, recipe).await,
        Fix::Configure => config::configure(task, paths, instance, recipe).await,
        Fix::Leftovers => backups::remove_partial(task, paths),
        Fix::Servers => runtime::restart(task, paths, instance, recipe).await,
    }
}

/// Repairs what [`plan`] finds, then checks again.
pub async fn repair(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    task.step("Checking every part of the server");
    let before = plan(&gather(paths, instance, recipe).await);
    if before.is_empty() {
        task.outcome("Nothing needs repairing: every part checks out.");
        return Ok(());
    }
    for finding in &before {
        task.info(&format!("Found: {}. {}", finding.title, finding.detail));
    }
    let to_run = fixes(&before);
    if to_run.is_empty() {
        return Err(format!("The app cannot repair this by itself: {}.", describe(&before)));
    }
    for fix in to_run {
        run_fix(task, paths, instance, recipe, fix).await.map_err(|e| format!("Repair stopped at \"{}\": {e}", fix.label()))?;
    }

    task.step("Checking the result");
    let after = plan(&gather(paths, instance, recipe).await);
    if after.is_empty() {
        task.outcome("Repaired: every part checks out.");
        Ok(())
    } else {
        Err(format!("Repair did what it could, but this still needs attention: {}.", describe(&after)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A healthy instance with the servers running.
    fn healthy() -> Facts {
        Facts {
            docker: None,
            repos: vec![("server".into(), RepoState::Ready, "Ready".into()), ("database".into(), RepoState::Ready, "Ready".into())],
            images_missing: Vec::new(),
            database_image_missing: false,
            client_ready: true,
            extract: ExtractState::Ready,
            database: DatabaseState::Ready,
            database_note: None,
            config: ConfigState::Ready,
            config_note: None,
            servers: ServerState::Running,
            partial_backups: 0,
        }
    }

    fn titles(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.title.as_str()).collect()
    }

    #[test]
    fn a_healthy_instance_has_nothing_to_repair() {
        assert!(plan(&healthy()).is_empty());
    }

    #[test]
    fn stopped_servers_and_waiting_updates_are_not_faults() {
        let mut f = healthy();
        f.servers = ServerState::Stopped;
        f.database = DatabaseState::Outdated;
        assert!(plan(&f).is_empty(), "Update is for a database with new scripts, Start for stopped servers");
        f.database = DatabaseState::Stopped;
        assert!(plan(&f).is_empty(), "a stopped database cannot be read, which is not a fault");
    }

    #[test]
    fn a_missing_image_is_a_build_and_a_crashed_server_is_a_restart() {
        let mut f = healthy();
        f.images_missing = vec!["worldstone/vanilla-world:latest".into()];
        f.servers = ServerState::Failing;
        let found = plan(&f);
        assert_eq!(titles(&found), ["The server image is missing", "A server crashed or stopped answering"]);
        assert_eq!(fixes(&found), [Fix::Build, Fix::Servers]);
        assert!(found[0].detail.contains("worldstone/vanilla-world:latest"));
    }

    #[test]
    fn game_data_is_extracted_only_when_a_client_is_chosen() {
        let mut f = healthy();
        f.extract = ExtractState::NotExtracted;
        assert_eq!(fixes(&plan(&f)), [Fix::Extract]);
        f.client_ready = false;
        let found = plan(&f);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fix, None, "only the person can choose the client");
        assert!(fixes(&found).is_empty());
    }

    #[test]
    fn a_database_that_is_incomplete_is_completed_not_reset() {
        let mut f = healthy();
        f.database = DatabaseState::Incomplete;
        let found = plan(&f);
        assert_eq!(fixes(&found), [Fix::Database]);
        assert!(found[0].detail.contains("without resetting"), "{}", found[0].detail);
        f.database = DatabaseState::Unknown;
        assert_eq!(plan(&f)[0].fix, None, "unknown is reported as unknown, not repaired blindly");
    }

    #[test]
    fn an_unnamed_database_image_is_fixed_by_setting_up_the_database_once() {
        let mut f = healthy();
        f.database_image_missing = true;
        f.database = DatabaseState::Incomplete;
        assert_eq!(plan(&f).len(), 1, "one finding, not two");
        f.database = DatabaseState::Stopped;
        assert_eq!(fixes(&plan(&f)), [Fix::Database]);
    }

    #[test]
    fn the_configuration_is_generated_when_missing_or_out_of_date() {
        let mut f = healthy();
        f.config = ConfigState::NotConfigured;
        assert_eq!(fixes(&plan(&f)), [Fix::Configure]);
        f.config = ConfigState::Outdated;
        f.config_note = Some("PlayerLimit differs".into());
        assert!(plan(&f)[0].detail.contains("PlayerLimit"));
        f.config = ConfigState::Unknown;
        assert_eq!(plan(&f)[0].fix, None);
    }

    #[test]
    fn some_servers_running_is_a_fault_and_a_starting_one_is_not() {
        let mut f = healthy();
        f.servers = ServerState::Partial;
        assert_eq!(fixes(&plan(&f)), [Fix::Servers]);
        f.servers = ServerState::Starting;
        assert!(plan(&f).is_empty());
        f.servers = ServerState::Paused;
        assert!(plan(&f).is_empty(), "paused is a choice");
    }

    #[test]
    fn unfinished_backups_are_counted_in_words() {
        let mut f = healthy();
        f.partial_backups = 1;
        assert!(plan(&f)[0].detail.starts_with("1 backup never finished"));
        f.partial_backups = 3;
        assert!(plan(&f)[0].detail.starts_with("3 backups never finished"));
    }

    #[test]
    fn a_missing_source_is_downloaded_before_anything_is_built_from_it() {
        let mut f = healthy();
        f.repos[0].1 = RepoState::Missing;
        f.repos[1].1 = RepoState::Incomplete;
        f.images_missing = vec!["x".into()];
        f.database = DatabaseState::Incomplete;
        f.config = ConfigState::NotConfigured;
        f.servers = ServerState::Stopped;
        let found = plan(&f);
        assert_eq!(found[0].title, "The server source is missing");
        assert_eq!(found[1].title, "The database source is incomplete");
        assert_eq!(fixes(&found), [Fix::Source, Fix::Build, Fix::Database, Fix::Configure], "each fix once, in the order they depend on each other");
    }

    #[test]
    fn without_docker_nothing_else_is_judged() {
        let mut f = healthy();
        f.docker = Some("Docker Desktop is not running".into());
        f.images_missing = vec!["x".into()];
        let found = plan(&f);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fix, None);
        assert!(found[0].detail.contains("not running"));
    }
}
