use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct UserSettings {
    pub shortcut: String,
    pub selected_model_id: String,
    pub model_dir: String,
    pub output_dir: String,
    pub paste_mode: String,
    pub recording_mode: String,
    pub export_format: String,
    pub theme: String,
    pub save_recordings: bool,
    pub launch_at_startup: bool,
    pub show_mini_window: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            shortcut: "CapsLock".to_string(),
            selected_model_id: "sensevoice-small".to_string(),
            model_dir: String::new(),
            output_dir: String::new(),
            paste_mode: "clipboard".to_string(),
            recording_mode: "hold".to_string(),
            export_format: "plainText".to_string(),
            theme: "light".to_string(),
            save_recordings: false,
            launch_at_startup: false,
            show_mini_window: true,
        }
    }
}
