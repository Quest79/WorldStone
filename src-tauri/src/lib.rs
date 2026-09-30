mod accounts;
mod appearance;
mod backups;
mod bots;
mod builder;
mod client;
mod clock;
mod commands;
mod compose;
mod config;
mod connect;
mod dashboard;
mod database;
mod docker;
mod dockerapi;
mod environment;
mod extractor;
mod fonts;
mod git;
mod images;
mod interrupt;
mod logview;
mod models;
mod paths;
mod play;
mod process;
mod progress;
mod recipe;
mod repair;
mod reset;
mod runtime;
mod settings;
mod tasks;
mod update;

use commands::AppState;
use std::sync::Arc;
use tasks::{EventSink, LogEvent, TaskEvent, TaskManager};
use tauri::{AppHandle, Emitter, Manager};

/// Forwards task and log events to the frontend.
struct TauriSink(AppHandle);

impl EventSink for TauriSink {
    fn task(&self, event: &TaskEvent) {
        let _ = self.0.emit("task-update", event);
    }

    fn log(&self, event: &LogEvent) {
        let _ = self.0.emit("task-log", event);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let paths = paths::Paths::resolve()?;
            let recipes = recipe::builtin()?;
            let tasks = TaskManager::new(Arc::new(TauriSink(app.handle().clone())), paths.launcher_log());
            app.manage(AppState { paths, tasks, recipes });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::list_recipes,
            commands::check_environment,
            commands::repo_status,
            commands::setup_repositories,
            commands::build_status,
            commands::build_server,
            commands::client_status,
            commands::set_client,
            commands::choose_client,
            commands::extract_status,
            commands::extract_client_data,
            commands::database_status,
            commands::provision_database,
            commands::config_status,
            commands::configure_server,
            commands::server_status,
            commands::start_server,
            commands::stop_server,
            commands::restart_server,
            commands::pause_server,
            commands::resume_server,
            commands::launch_game,
            commands::connect_status,
            commands::point_client,
            commands::restore_client,
            commands::accounts_status,
            commands::create_account,
            commands::change_password,
            commands::bots_status,
            commands::save_bot_settings,
            commands::images_status,
            commands::remove_old_images,
            commands::backups_status,
            commands::create_backup,
            commands::restore_backup,
            commands::open_backups_folder,
            commands::dashboard_status,
            commands::log_sources,
            commands::read_log,
            commands::update_server,
            commands::repair_status,
            commands::repair_instance,
            commands::reset_preview,
            commands::reset_instance,
            commands::get_appearance,
            commands::save_appearance,
            commands::installed_fonts,
            commands::google_catalogue,
            commands::google_downloaded,
            commands::download_google_font,
            commands::google_font_faces,
            commands::current_task,
            commands::cancel_task,
        ])
        .run(tauri::generate_context!())
        .expect("error while running WorldStone");
}
