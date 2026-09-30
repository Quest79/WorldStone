//! Update (Step 9): pull the newest source, rebuild what changed, apply new database scripts and
//! restart, as one operation that touches only what has to change.
//!
//! The order is chosen so that a failure leaves things as safe as possible:
//! 1. everything that would stop a later step is found out first (Docker, the source, the image,
//!    the database, and whether the servers could be started again);
//! 2. the source is fast-forwarded (never reset), so an unrelated local edit is never lost;
//! 3. the images are rebuilt only when the source or recipe no longer matches them, while the
//!    servers keep running on the old ones;
//! 4. when database scripts are waiting, the servers are stopped (the world server must not run
//!    while its tables change), **every database is backed up**, and only then are the scripts applied;
//! 5. servers that were running are started again, on the new images. Servers that were stopped stay
//!    stopped, and nothing is restarted when nothing changed.

use crate::{
    backups,
    builder::{self, BuildState},
    database::{self, Credentials, DatabaseState},
    git::{self, RepoState},
    paths::InstancePaths,
    recipe::Recipe,
    runtime::{self, ServerState},
    tasks::Task,
};

/// What happens to the servers, decided from what the update found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServersPlan {
    /// They were stopped and stay stopped.
    WereStopped,
    /// They run, and nothing changed that they use.
    LeaveRunning,
    /// They run and a new image is waiting: restart them.
    Restart,
    /// They run and the database is about to change: stop them first, start them after.
    StopForDatabase,
}

pub fn servers_plan(was_up: bool, rebuilt: bool, database_update: bool) -> ServersPlan {
    match (was_up, database_update, rebuilt) {
        (false, _, _) => ServersPlan::WereStopped,
        (true, true, _) => ServersPlan::StopForDatabase,
        (true, false, true) => ServersPlan::Restart,
        (true, false, false) => ServersPlan::LeaveRunning,
    }
}

/// `(repository id, checked-out commit)` for every repository of the recipe.
async fn commits(paths: &InstancePaths, recipe: &Recipe) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for repo in &recipe.repos {
        out.push((repo.id.clone(), git::inspect(paths, recipe, repo).await.commit));
    }
    out
}

fn short(commit: &str) -> String {
    commit.chars().take(12).collect()
}

/// One line per repository whose checkout moved.
pub fn source_changes(before: &[(String, Option<String>)], after: &[(String, Option<String>)]) -> Vec<String> {
    after
        .iter()
        .filter_map(|(id, now)| {
            let was = before.iter().find(|(b, _)| b == id).and_then(|(_, commit)| commit.as_deref());
            match (was, now.as_deref()) {
                (Some(was), Some(now)) if was != now => Some(format!("{id} moved from {} to {}", short(was), short(now))),
                (None, Some(now)) => Some(format!("{id} is now at {}", short(now))),
                _ => None,
            }
        })
        .collect()
}

/// What the update did, in one sentence.
pub fn summary(source_moved: bool, rebuilt: bool, database: Option<u32>, plan: ServersPlan) -> String {
    if !source_moved && !rebuilt && database.is_none() {
        return "Everything was already up to date, so nothing was changed and nothing was restarted.".to_string();
    }
    let mut parts = vec![
        if source_moved { "pulled new source" } else { "the source was already current" }.to_string(),
        if rebuilt { "rebuilt the server images" } else { "the server images already matched" }.to_string(),
    ];
    parts.push(match database {
        Some(0) => "completed the database setup, after a backup".to_string(),
        Some(n) => format!("applied {n} database update{}, after a backup", if n == 1 { "" } else { "s" }),
        None => "the database was already current".to_string(),
    });
    parts.push(
        match plan {
            ServersPlan::WereStopped => "the servers were stopped and stay stopped",
            ServersPlan::LeaveRunning => "the servers were left running",
            ServersPlan::Restart | ServersPlan::StopForDatabase => "restarted the servers on the new version",
        }
        .to_string(),
    );
    let text = parts.join("; ");
    let (first, rest) = text.split_at(text.chars().next().map_or(0, char::len_utf8));
    format!("{}{rest}.", first.to_uppercase())
}

struct Done {
    plan: ServersPlan,
    /// Update scripts applied (`Some(0)` when the database needed completing rather than updating).
    database: Option<u32>,
}

/// The database and server part of an update. The database must be up.
async fn finish(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe, creds: &Credentials, was_up: bool, rebuilt: bool) -> Result<Done, String> {
    task.step("Checking the database for updates");
    let db = database::status(paths, instance, recipe).await;
    if !matches!(db.state, DatabaseState::Ready | DatabaseState::Outdated | DatabaseState::Incomplete) {
        return Err(db.note.or(db.blocked).unwrap_or_else(|| "The database cannot be read, so it was not updated. Use Set up database.".to_string()));
    }
    let pending: u32 = db.schemas.iter().map(|s| s.updates_pending).sum();
    let database_update = matches!(db.state, DatabaseState::Outdated | DatabaseState::Incomplete);
    let plan = servers_plan(was_up, rebuilt, database_update);

    let mut stopped = false;
    if plan == ServersPlan::StopForDatabase {
        task.info("The servers are stopped while the database changes, and started again afterwards.");
        runtime::stop(task, paths, instance, recipe).await.map_err(|e| format!("The update was interrupted before the database was touched. {e}"))?;
        stopped = true;
        backups::open_database(task, paths, instance, recipe).await?;
    }

    if let Err(e) = change(task, paths, instance, recipe, creds, plan, database_update).await {
        return Err(if stopped { format!("{e} The servers were stopped for the update and are still stopped.") } else { e });
    }
    Ok(Done { plan, database: database_update.then_some(pending) })
}

/// Backs up and updates the database when it needs it, then does what the plan says with the servers.
async fn change(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe, creds: &Credentials, plan: ServersPlan, database_update: bool) -> Result<(), String> {
    if database_update {
        let files = backups::backup_everything(task, instance, recipe, creds, backups::BEFORE_UPDATE).await?;
        if files.is_empty() {
            task.info("The database holds nothing yet, so there is nothing to back up.");
        }
        database::provision_after_backup(task, paths, instance, recipe, true).await?;
    } else {
        task.info("The database is already current.");
    }
    match plan {
        ServersPlan::Restart => runtime::restart(task, paths, instance, recipe).await,
        ServersPlan::StopForDatabase => runtime::start(task, paths, instance, recipe).await,
        ServersPlan::LeaveRunning => {
            task.info("Nothing the servers use changed, so they were left running.");
            Ok(())
        }
        ServersPlan::WereStopped => Ok(()),
    }
}

/// Updates the installed server. Everything is checked before anything is changed; see the module
/// notes for the order.
pub async fn update(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    task.step("Checking what is installed");
    builder::engine().await?;
    for repo in &recipe.repos {
        let seen = git::inspect(paths, recipe, repo).await;
        if seen.state != RepoState::Ready {
            return Err(format!("{} is not ready ({}). Update refreshes a server that is already set up; use Set up first.", repo.id, seen.detail));
        }
    }
    let built = builder::status(paths, instance, recipe).await;
    match built.state {
        BuildState::Unknown => return Err(built.blocked.unwrap_or_else(|| "Docker cannot be asked about the server build.".to_string())),
        BuildState::NotBuilt => return Err("The server has not been built yet. Update refreshes a server that is already set up; use Set up first.".to_string()),
        BuildState::Ready | BuildState::Outdated => {}
    }
    let creds = Credentials::load(paths)?.ok_or_else(|| "There is no database yet. Update refreshes a server that is already set up; use Set up first.".to_string())?;
    for secret in creds.secrets() {
        task.add_secret(secret);
    }
    let servers = runtime::status(paths, instance, recipe).await;
    let was_up = match servers.state {
        ServerState::Stopped => false,
        ServerState::Unknown => return Err(servers.note.unwrap_or_else(|| "Docker cannot be asked whether the servers are running.".to_string())),
        _ => true,
    };
    if was_up {
        // Servers that are stopped for the update must be able to start again, so that is found out now.
        runtime::check_ready(task, paths, instance, recipe).await?;
    }

    task.step("Downloading source changes");
    let before = commits(paths, recipe).await;
    git::sync_all(task, paths, recipe).await?;
    let moved = source_changes(&before, &commits(paths, recipe).await);
    if moved.is_empty() {
        task.info("The source is already up to date.");
    }
    for line in &moved {
        task.info(line);
    }

    let rebuild = builder::status(paths, instance, recipe).await.state != BuildState::Ready;
    if rebuild {
        builder::build(task, paths, instance, recipe, false).await?;
    } else {
        task.info("The server images already match the source, so nothing is rebuilt.");
    }

    let opened = backups::open_database(task, paths, instance, recipe).await?;
    let result = finish(task, paths, instance, recipe, &creds, was_up, rebuild).await;
    let restarted = matches!(&result, Ok(done) if matches!(done.plan, ServersPlan::Restart | ServersPlan::StopForDatabase));
    if !restarted {
        // A database this update had to start for its own work is stopped again; one that was
        // already running for the servers is left running.
        backups::close_database(task, paths, instance, recipe, opened && !was_up).await;
    }
    let done = result?;
    task.outcome(&summary(!moved.is_empty(), rebuild, done.database, done.plan));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str, commit: Option<&str>) -> (String, Option<String>) {
        (text.to_string(), commit.map(str::to_string))
    }

    #[test]
    fn servers_that_were_stopped_stay_stopped_whatever_changed() {
        for (rebuilt, database) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(servers_plan(false, rebuilt, database), ServersPlan::WereStopped);
        }
    }

    #[test]
    fn running_servers_are_touched_only_when_something_they_use_changed() {
        assert_eq!(servers_plan(true, false, false), ServersPlan::LeaveRunning);
        assert_eq!(servers_plan(true, true, false), ServersPlan::Restart);
        assert_eq!(servers_plan(true, false, true), ServersPlan::StopForDatabase, "the world must not run while its tables change");
        assert_eq!(servers_plan(true, true, true), ServersPlan::StopForDatabase);
    }

    #[test]
    fn a_moved_checkout_is_reported_and_an_unchanged_one_is_not() {
        let before = vec![id("server", Some("aaaaaaaaaaaaaaaa")), id("database", Some("bbbb"))];
        let after = vec![id("server", Some("cccccccccccccccc")), id("database", Some("bbbb"))];
        assert_eq!(source_changes(&before, &after), ["server moved from aaaaaaaaaaaa to cccccccccccc"]);
        assert!(source_changes(&before, &before).is_empty());
    }

    #[test]
    fn a_repository_that_appeared_is_reported_and_one_that_cannot_be_read_is_not() {
        let after = vec![id("server", Some("abc")), id("database", None)];
        assert_eq!(source_changes(&[], &after), ["server is now at abc"]);
    }

    #[test]
    fn nothing_changing_says_so_and_claims_no_restart() {
        let text = summary(false, false, None, ServersPlan::LeaveRunning);
        assert!(text.contains("already up to date") && text.contains("nothing was restarted"), "{text}");
    }

    #[test]
    fn the_summary_says_what_was_done_and_mentions_the_backup_only_with_database_changes() {
        let text = summary(true, true, Some(3), ServersPlan::StopForDatabase);
        assert_eq!(text, "Pulled new source; rebuilt the server images; applied 3 database updates, after a backup; restarted the servers on the new version.");
        let one = summary(true, false, Some(1), ServersPlan::WereStopped);
        assert!(one.contains("applied 1 database update, after a backup") && one.contains("stay stopped"), "{one}");
        let none = summary(true, false, None, ServersPlan::Restart);
        assert!(!none.contains("backup") && none.contains("database was already current"), "{none}");
        let completed = summary(false, false, Some(0), ServersPlan::LeaveRunning);
        assert!(completed.contains("completed the database setup"), "{completed}");
    }
}
