//! Tauri command handlers. Deliberately thin: each one validates its input and delegates
//! to a service module.

use crate::{
    accounts, appearance, backups, bots, builder, client, config, connect, dashboard, database, environment, extractor, fonts, git, images, logview, play, repair, reset,
    models::{AppInfo, Check},
    paths::Paths,
    recipe::{Recipe, RecipeSummary},
    runtime,
    tasks::{TaskEvent, TaskManager},
    update,
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

pub struct AppState {
    pub paths: Paths,
    pub tasks: TaskManager,
    pub recipes: Vec<Recipe>,
}

impl AppState {
    fn recipe(&self, id: &str) -> Result<&Recipe, String> {
        self.recipes
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("Unknown recipe '{id}'"))
    }
}

#[tauri::command]
pub fn app_info(app: AppHandle, state: State<'_, AppState>) -> AppInfo {
    let info = app.package_info();
    AppInfo {
        name: info.name.clone(),
        version: info.version.to_string(),
        root: state.paths.root.to_string_lossy().into_owned(),
    }
}

#[tauri::command]
pub fn list_recipes(state: State<'_, AppState>) -> Vec<RecipeSummary> {
    state.recipes.iter().map(Recipe::summary).collect()
}

#[tauri::command]
pub async fn check_environment() -> Vec<Check> {
    environment::check().await
}

#[tauri::command]
pub async fn repo_status(state: State<'_, AppState>, recipe_id: String) -> Result<Vec<git::RepoStatus>, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(git::inspect_all(&paths, recipe, &recipe.repos).await)
}

/// Starts the download/update in the background and returns its task id straight away.
#[tauri::command]
pub fn setup_repositories(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let present = recipe.repos.iter().all(|r| paths.repos.join(recipe.repo_folder(r)).exists());
    let verb = if present { "Update" } else { "Download" };
    let task = state.tasks.begin(&format!("{verb} {} source", recipe.name), true)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = git::sync_all(&mut task, &paths, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

#[tauri::command]
pub async fn build_status(state: State<'_, AppState>, recipe_id: String) -> Result<builder::BuildStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(builder::status(&paths, &recipe.id, recipe).await)
}

/// Starts the container build in the background and returns its task id straight away.
/// `force` rebuilds from scratch even when the image is current.
#[tauri::command]
pub fn build_server(state: State<'_, AppState>, recipe_id: String, force: bool) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let verb = if force { "Rebuild" } else { "Build" };
    let task = state.tasks.begin(&format!("{verb} {} server", recipe.name), true)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = builder::build(&mut task, &paths, &recipe.id, &recipe, force).await;
        task.finish(result);
    });
    Ok(id)
}

/// The remembered game client folder and whether it still passes the checks.
#[tauri::command]
pub fn client_status(state: State<'_, AppState>, recipe_id: String) -> Result<client::ClientStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(client::status(&paths, &recipe.client))
}

/// Checks a typed or pasted folder and remembers it only if it is a usable client.
#[tauri::command]
pub fn set_client(state: State<'_, AppState>, recipe_id: String, path: String) -> Result<client::ClientInfo, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    client::select(&paths, &recipe.client, &path)
}

/// Opens the system folder picker. `None` when the person closed it without choosing.
#[tauri::command]
pub async fn choose_client(
    app: AppHandle,
    state: State<'_, AppState>,
    recipe_id: String,
) -> Result<Option<client::ClientInfo>, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let title = format!("Choose your {} game client folder", recipe.game);
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().set_title(title).blocking_pick_folder())
        .await
        .map_err(|e| e.to_string())?;
    match picked {
        None => Ok(None),
        Some(folder) => {
            let path = folder.into_path().map_err(|e| e.to_string())?;
            client::select(&paths, &recipe.client, &path.to_string_lossy()).map(Some)
        }
    }
}

#[tauri::command]
pub async fn extract_status(state: State<'_, AppState>, recipe_id: String) -> Result<extractor::ExtractStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(extractor::status(&paths, &recipe.id, recipe).await)
}

/// Starts extracting the client's data in the background and returns its task id straight away.
#[tauri::command]
pub fn extract_client_data(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin(&format!("Extract {} client data", recipe.name), true)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = extractor::extract(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

#[tauri::command]
pub async fn database_status(state: State<'_, AppState>, recipe_id: String) -> Result<database::DatabaseStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(database::status(&paths, &recipe.id, recipe).await)
}

/// Starts setting up (or updating) the database in the background and returns its task id straight
/// away. It is safe to run again after an interruption. Only an emulator with its own importer
/// (a separate throwaway container, removed on cancel) can be cancelled: the SQL scripts of the
/// other method run inside the database container and would keep running after a cancel.
#[tauri::command]
pub fn provision_database(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let cancellable = recipe.database.method == crate::recipe::DatabaseMethod::ImportTool;
    let task = state.tasks.begin(&format!("Set up {} database", recipe.name), cancellable)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = database::provision(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

/// What the configuration files hold compared with what the instance's settings call for.
#[tauri::command]
pub fn config_status(state: State<'_, AppState>, recipe_id: String) -> Result<config::ConfigStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(config::status(&paths, recipe))
}

/// Creates the configuration files, or puts the settings the app manages back in existing ones,
/// in the background. Returns the task id straight away.
#[tauri::command]
pub fn configure_server(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin(&format!("Configure {} server", recipe.name), false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = config::configure(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

#[tauri::command]
pub async fn server_status(state: State<'_, AppState>, recipe_id: String) -> Result<runtime::ServerStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(runtime::status(&paths, &recipe.id, recipe).await)
}

/// Starts the servers and waits until they answer. Cancelling stops what was started.
#[tauri::command]
pub fn start_server(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin(&format!("Start {} server", recipe.name), true)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = runtime::start(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

/// Stops the servers and the database. Not cancellable: the world server is saving the game.
#[tauri::command]
pub fn stop_server(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin(&format!("Stop {} server", recipe.name), false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = runtime::stop(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

#[tauri::command]
pub fn restart_server(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin(&format!("Restart {} server", recipe.name), false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = runtime::restart(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

/// Freezes the servers where they are. Instant and safe to repeat; not cancellable.
#[tauri::command]
pub fn pause_server(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin(&format!("Pause {} server", recipe.name), false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = runtime::pause(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

/// Lets paused servers carry on and waits until they answer.
#[tauri::command]
pub fn resume_server(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin(&format!("Resume {} server", recipe.name), true)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = runtime::resume(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

/// Starts the game client (pointing it at this server first) while the servers are running.
#[tauri::command]
pub fn launch_game(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin(&format!("Launch the {} game client", recipe.game), false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = play::launch(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

/// Where the remembered game client logs in, read from its realm list file.
#[tauri::command]
pub fn connect_status(state: State<'_, AppState>, recipe_id: String) -> Result<connect::ConnectStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(connect::status(&paths, recipe))
}

/// Points the game client at this server, keeping the address it had.
#[tauri::command]
pub fn point_client(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin("Point the game client at this server", false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = connect::point(&mut task, &paths, &recipe);
        task.finish(result);
    });
    Ok(id)
}

/// Puts the address the game client had back.
#[tauri::command]
pub fn restore_client(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin("Restore the game client's address", false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = connect::restore(&mut task, &paths, &recipe);
        task.finish(result);
    });
    Ok(id)
}

/// The accounts in the realm database.
#[tauri::command]
pub async fn accounts_status(state: State<'_, AppState>, recipe_id: String) -> Result<accounts::AccountsStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(accounts::status(&paths, &recipe.id, recipe).await)
}

/// Creates an account through the world server's console. The name and password are checked before
/// anything starts, so a refusal costs no task; the password is never logged.
#[tauri::command]
pub fn create_account(state: State<'_, AppState>, recipe_id: String, name: String, password: String, gm_level: u8) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let name = accounts::check_name(&recipe.accounts, &name)?;
    accounts::password_for_new_account(&recipe.accounts, &name, &password)?;
    accounts::check_level(&recipe.accounts, gm_level)?;
    let task = state.tasks.begin(&format!("Create account {name}"), false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = accounts::create(&mut task, &paths, &recipe.id, &recipe, &name, &password, gm_level).await;
        task.finish(result);
    });
    Ok(id)
}

/// Changes an account's password through the world server's console.
#[tauri::command]
pub fn change_password(state: State<'_, AppState>, recipe_id: String, name: String, password: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let name = accounts::check_name(&recipe.accounts, &name)?;
    accounts::check_password(&recipe.accounts, &password)?;
    let task = state.tasks.begin(&format!("Change the password of {name}"), false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = accounts::change_password(&mut task, &paths, &recipe.id, &recipe, &name, &password).await;
        task.finish(result);
    });
    Ok(id)
}

/// Whether the playerbots are really in the world, from the database.
#[tauri::command]
pub async fn bots_status(state: State<'_, AppState>, recipe_id: String) -> Result<bots::BotsStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(bots::status(&paths, &recipe.id, recipe).await)
}

/// Saves how many bots there should be and regenerates the configuration from it.
#[tauri::command]
pub fn save_bot_settings(state: State<'_, AppState>, recipe_id: String, choice: bots::BotChoice) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin("Save the playerbot settings", false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = bots::save_settings(&mut task, &paths, &recipe.id, &recipe, choice).await;
        task.finish(result);
    });
    Ok(id)
}

/// The Docker images of this instance, named for what they are, and whether anything uses them.
#[tauri::command]
pub async fn images_status(state: State<'_, AppState>, recipe_id: String) -> Result<images::ImagesStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    Ok(images::status(&recipe.id, recipe).await)
}

/// Removes the server builds that were replaced and that no container uses.
#[tauri::command]
pub fn remove_old_images(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let task = state.tasks.begin("Remove old server builds", false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = images::remove_old(&mut task, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

/// The backups on disk, and whether one can be made or restored right now.
#[tauri::command]
pub async fn backups_status(state: State<'_, AppState>, recipe_id: String) -> Result<backups::BackupsStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(backups::status(&paths, &recipe.id, recipe).await)
}

/// Backs up every database of the instance. Not cancellable: a dump cut short is a file that must
/// not be trusted, and the backup step cleans up after itself only when it runs to the end.
#[tauri::command]
pub fn create_backup(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin("Back up the databases", false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = backups::create(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

/// Replaces the databases with a backup. The servers must be stopped; the backup's files are checked
/// and a safety backup is taken before anything is replaced.
#[tauri::command]
pub fn restore_backup(state: State<'_, AppState>, recipe_id: String, set_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin("Restore a backup", false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = backups::restore(&mut task, &paths, &recipe.id, &recipe, &set_id).await;
        task.finish(result);
    });
    Ok(id)
}

/// Opens the Backups folder in the file manager, creating it first if nothing was ever backed up.
#[tauri::command]
pub fn open_backups_folder(app: AppHandle, state: State<'_, AppState>, recipe_id: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    paths.ensure()?;
    app.opener()
        .open_path(paths.backups.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("Cannot open {}: {e}", paths.backups.display()))
}

/// Per-container state, port, uptime and processor/memory use, read from Docker.
#[tauri::command]
pub async fn dashboard_status(state: State<'_, AppState>, recipe_id: String) -> Result<dashboard::Dashboard, String> {
    let recipe = state.recipe(&recipe_id)?;
    Ok(dashboard::status(&recipe.id, recipe).await)
}

/// The logs that can be read: the launcher's history, each container, and the servers' log files.
#[tauri::command]
pub async fn log_sources(state: State<'_, AppState>, recipe_id: String) -> Result<Vec<logview::LogSource>, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(logview::sources(&state.paths, &paths, &recipe.id, recipe).await)
}

/// The last `lines` lines of one log source.
#[tauri::command]
pub async fn read_log(state: State<'_, AppState>, recipe_id: String, source: String, lines: u32) -> Result<logview::LogView, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    logview::read(&state.paths, &paths, &recipe.id, recipe, &source, lines).await
}

/// Pulls the newest source, rebuilds what changed, applies new database scripts (after a backup)
/// and restarts what was running. Not cancellable: it is a chain of steps that each leave things in
/// a valid state, but stopping in the middle of one is not.
#[tauri::command]
pub fn update_server(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin(&format!("Update {}", recipe.name), false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = update::update(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

/// What is wrong with the instance and what repairing it would do. Changes nothing.
#[tauri::command]
pub async fn repair_status(state: State<'_, AppState>, recipe_id: String) -> Result<repair::RepairStatus, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(repair::status(&paths, &recipe.id, recipe).await)
}

/// Repairs what the diagnosis finds, using the same steps as the Setup buttons, then checks again.
#[tauri::command]
pub fn repair_instance(state: State<'_, AppState>, recipe_id: String) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin(&format!("Repair {}", recipe.name), false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = repair::repair(&mut task, &paths, &recipe.id, &recipe).await;
        task.finish(result);
    });
    Ok(id)
}

/// What a reset would delete and keep, and what has to be typed to confirm it.
#[tauri::command]
pub async fn reset_preview(state: State<'_, AppState>, recipe_id: String) -> Result<reset::ResetPreview, String> {
    let recipe = state.recipe(&recipe_id)?;
    let paths = state.paths.instance(&recipe.id)?;
    Ok(reset::preview(&paths, &recipe.id, recipe).await)
}

/// Resets the instance: the only operation that deletes data. `confirmation` is checked here again
/// and a wrong one is refused before any task starts.
#[tauri::command]
pub fn reset_instance(state: State<'_, AppState>, recipe_id: String, confirmation: String, game_data: bool) -> Result<String, String> {
    let recipe = state.recipe(&recipe_id)?.clone();
    if !reset::confirmed(&recipe.id, &confirmation) {
        return Err(format!("Not confirmed: type {} exactly. Nothing was deleted.", reset::phrase(&recipe.id)));
    }
    let paths = state.paths.instance(&recipe.id)?;
    let task = state.tasks.begin(&format!("Reset {}", recipe.name), false)?;
    let id = task.id().to_string();
    tauri::async_runtime::spawn(async move {
        let mut task = task;
        let result = reset::reset(&mut task, &paths, &recipe.id, &recipe, &confirmation, game_data).await;
        task.finish(result);
    });
    Ok(id)
}

/// What the person chose for fonts, the defaults to reset to, and, if the saved file could not be
/// read, why (the defaults are then in use and the file is kept aside when something is saved).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceState {
    current: appearance::Appearance,
    defaults: appearance::Appearance,
    problem: Option<String>,
}

#[tauri::command]
pub fn get_appearance(state: State<'_, AppState>) -> AppearanceState {
    let defaults = appearance::Appearance::default();
    match appearance::load(&state.paths) {
        Ok(current) => AppearanceState { current, defaults, problem: None },
        Err(problem) => AppearanceState { current: defaults.clone(), defaults, problem: Some(problem) },
    }
}

#[tauri::command]
pub fn save_appearance(state: State<'_, AppState>, appearance: appearance::Appearance) -> Result<(), String> {
    appearance::save(&state.paths, &appearance)
}

/// Font families installed on this Windows.
#[tauri::command]
pub async fn installed_fonts(refresh: bool) -> Result<Vec<String>, String> {
    fonts::installed(refresh).await
}

/// The Google Fonts list, from the saved copy unless `refresh` asks for a new one.
#[tauri::command]
pub async fn google_catalogue(state: State<'_, AppState>, refresh: bool) -> Result<fonts::Catalogue, String> {
    fonts::catalogue(&state.paths, refresh).await
}

/// The Google families already downloaded.
#[tauri::command]
pub fn google_downloaded(state: State<'_, AppState>) -> Vec<String> {
    fonts::downloaded(&state.paths)
}

/// Downloads a Google family; returns its exact name.
#[tauri::command]
pub async fn download_google_font(state: State<'_, AppState>, family: String) -> Result<String, String> {
    fonts::download(&state.paths, &family).await
}

/// The font files of a downloaded family, for the page to register.
#[tauri::command]
pub async fn google_font_faces(state: State<'_, AppState>, family: String) -> Result<Vec<fonts::FaceData>, String> {
    fonts::faces(&state.paths, &family)
}

/// The operation running right now, if any. A UI that reloaded while one was running uses this
/// to show it again instead of claiming nothing is happening.
#[tauri::command]
pub fn current_task(state: State<'_, AppState>) -> Option<TaskEvent> {
    state.tasks.current()
}

/// Returns whether a running, cancellable operation was asked to stop.
#[tauri::command]
pub fn cancel_task(state: State<'_, AppState>) -> bool {
    state.tasks.cancel()
}
