use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSettings {
    pub shortcut: String,
    pub model_dir: String,
    pub output_dir: String,
    pub paste_mode: String,
    pub save_recordings: bool,
    pub launch_at_startup: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            shortcut: "CapsLock".to_string(),
            model_dir: String::new(),
            output_dir: String::new(),
            paste_mode: "clipboard".to_string(),
            save_recordings: false,
            launch_at_startup: false,
        }
    }
}
