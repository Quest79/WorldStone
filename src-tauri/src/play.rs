//! Playing: starting the game client so it logs in to this server.
//!
//! The client is the user's own copy, chosen in Step 3. It is started only while the servers are
//! answering, and only after its realm list file sends it here (the step keeps a backup of the
//! address it had and does nothing when the file is already right, see `connect.rs`). The game runs
//! on its own afterwards: closing this app does not close it.

use crate::{
    client, connect,
    paths::InstancePaths,
    process,
    recipe::Recipe,
    runtime::{self, ServerState},
    tasks::Task,
};
use std::{path::Path, time::Duration};

/// How long the game gets to prove it did not fall over at once (a missing library, a blocked
/// executable) before it is reported as started.
const SETTLE: Duration = Duration::from_millis(2500);

/// Why the game cannot be launched with the servers in this state, or `None` when it can.
pub fn blocked_by(server: ServerState) -> Option<&'static str> {
    match server {
        ServerState::Running => None,
        ServerState::Paused => Some("The servers are paused. Resume them first."),
        ServerState::Starting => Some("The servers are still starting."),
        ServerState::Unknown => Some("Docker could not be asked whether the servers are running."),
        ServerState::Stopped | ServerState::Partial | ServerState::Failing => Some("Start the servers first."),
    }
}

/// Starts the game client. Refuses, with the reason, when it could not log in.
pub async fn launch(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    task.step("Checking the servers");
    let server = runtime::status(paths, instance, recipe).await;
    if let Some(why) = blocked_by(server.state) {
        return Err(why.to_string());
    }

    task.step("Checking the game client");
    let known = client::status(paths, &recipe.client);
    let info = match (known.info, known.problem) {
        (Some(info), _) => info,
        (None, Some(problem)) => return Err(problem),
        (None, None) => return Err("Choose your game client first.".to_string()),
    };
    let folder = Path::new(&info.path);
    let exe = folder.join(&recipe.client.executable);

    connect::point(task, paths, recipe)?;

    task.step(&format!("Starting {}", recipe.client.executable));
    let mut child = process::spawn_detached(&exe, folder)?;
    tokio::time::sleep(SETTLE).await;
    match child.try_wait() {
        Ok(Some(status)) => Err(format!(
            "{} closed straight after starting (exit code {}). Start it from its own folder once to see why.",
            recipe.client.executable,
            status.code().map_or_else(|| "unknown".to_string(), |c| c.to_string())
        )),
        _ => {
            task.info(&format!("Started {} from {}", recipe.client.executable, folder.display()));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_starts_only_when_the_servers_are_running() {
        assert_eq!(blocked_by(ServerState::Running), None);
        for state in [ServerState::Stopped, ServerState::Partial, ServerState::Failing, ServerState::Starting, ServerState::Paused, ServerState::Unknown] {
            assert!(blocked_by(state).is_some(), "{state:?} must not allow a launch");
        }
        assert!(blocked_by(ServerState::Paused).unwrap().contains("Resume"));
    }
}
