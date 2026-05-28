mod app_state;
mod config;

use app_state::AppSnapshot;
use config::UserSettings;
use std::sync::Mutex;
use tauri::State;

struct RuntimeState {
    settings: Mutex<UserSettings>,
}

#[tauri::command]
fn get_app_snapshot(state: State<'_, RuntimeState>) -> AppSnapshot {
    let settings = state.settings.lock().expect("settings mutex poisoned");
    AppSnapshot {
        shortcut: settings.shortcut.clone(),
        ..AppSnapshot::default()
    }
}

#[tauri::command]
fn load_settings(state: State<'_, RuntimeState>) -> UserSettings {
    state.settings.lock().expect("settings mutex poisoned").clone()
}

#[tauri::command]
fn save_settings(settings: UserSettings, state: State<'_, RuntimeState>) -> UserSettings {
    let mut stored = state.settings.lock().expect("settings mutex poisoned");
    *stored = settings.clone();
    settings
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(RuntimeState {
            settings: Mutex::new(UserSettings::default()),
        })
        .invoke_handler(tauri::generate_handler![
            get_app_snapshot,
            load_settings,
            save_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    run();
}
