use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct UserSettings {
    pub shortcut: String,
    pub selected_model_id: String,
    pub model_dir: String,
    pub input_model_id: String,
    pub input_model_dir: String,
    pub transcription_model_id: String,
    pub transcription_model_dir: String,
    pub output_dir: String,
    pub paste_mode: String,
    pub recording_mode: String,
    pub recording_source: String,
    pub acceleration_mode: String,
    pub directml_verified: bool,
    pub directml_verified_at: Option<String>,
    pub hotwords: Vec<HotwordRule>,
    pub term_categories: Vec<TermCategory>,
    pub export_format: String,
    pub theme: String,
    pub save_recordings: bool,
    pub launch_at_startup: bool,
    pub show_mini_window: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct HotwordRule {
    pub id: String,
    pub source: String,
    pub target: String,
    pub enabled: bool,
    pub category_id: Option<String>,
    pub hit_count: Option<u32>,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct TermCategory {
    pub id: String,
    pub name: String,
    pub order: u32,
}

impl Default for HotwordRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            source: String::new(),
            target: String::new(),
            enabled: true,
            category_id: None,
            hit_count: Some(0),
            last_used_at: None,
        }
    }
}

impl Default for TermCategory {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            order: 0,
        }
    }
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            shortcut: "CapsLock".to_string(),
            selected_model_id: "sensevoice-small".to_string(),
            model_dir: String::new(),
            input_model_id: "sensevoice-small".to_string(),
            input_model_dir: String::new(),
            transcription_model_id: "qwen3-asr-0.6b".to_string(),
            transcription_model_dir: String::new(),
            output_dir: String::new(),
            paste_mode: "clipboard".to_string(),
            recording_mode: "hold".to_string(),
            recording_source: "microphone".to_string(),
            acceleration_mode: "cpu".to_string(),
            directml_verified: false,
            directml_verified_at: None,
            hotwords: Vec::new(),
            term_categories: vec![
                TermCategory {
                    id: "people".to_string(),
                    name: "人名".to_string(),
                    order: 0,
                },
                TermCategory {
                    id: "organizations".to_string(),
                    name: "机构".to_string(),
                    order: 1,
                },
                TermCategory {
                    id: "projects".to_string(),
                    name: "项目".to_string(),
                    order: 2,
                },
                TermCategory {
                    id: "technical".to_string(),
                    name: "技术词".to_string(),
                    order: 3,
                },
                TermCategory {
                    id: "replacements".to_string(),
                    name: "常用替换".to_string(),
                    order: 4,
                },
            ],
            export_format: "plainText".to_string(),
            theme: "light".to_string(),
            save_recordings: false,
            launch_at_startup: false,
            show_mini_window: true,
        }
    }
}

impl UserSettings {
    pub fn normalized(mut self) -> Self {
        let defaults = Self::default();

        if self.shortcut.trim().is_empty() {
            self.shortcut = defaults.shortcut;
        }
        if self.selected_model_id.trim().is_empty() {
            self.selected_model_id = defaults.selected_model_id;
        }
        if self.input_model_id.trim().is_empty() {
            self.input_model_id = self.selected_model_id.clone();
        }
        if self.transcription_model_id.trim().is_empty() {
            self.transcription_model_id = if self.input_model_id == self.selected_model_id {
                self.selected_model_id.clone()
            } else {
                defaults.transcription_model_id
            };
        }
        if self.input_model_dir.trim().is_empty() && !self.model_dir.trim().is_empty() {
            self.input_model_dir = self.model_dir.clone();
        }
        if self.transcription_model_dir.trim().is_empty() && !self.model_dir.trim().is_empty() {
            self.transcription_model_dir = self.model_dir.clone();
        }
        if !matches!(self.paste_mode.as_str(), "direct" | "clipboard") {
            self.paste_mode = defaults.paste_mode;
        }
        if !matches!(
            self.recording_mode.as_str(),
            "hold" | "toggle" | "audioOnly"
        ) {
            self.recording_mode = defaults.recording_mode;
        }
        if !matches!(
            self.recording_source.as_str(),
            "microphone" | "system" | "microphoneAndSystem"
        ) {
            self.recording_source = defaults.recording_source;
        }
        if !matches!(self.acceleration_mode.as_str(), "cpu" | "directml") {
            self.acceleration_mode = defaults.acceleration_mode;
        }
        if !self.directml_verified {
            self.directml_verified_at = None;
        }
        if !matches!(
            self.export_format.as_str(),
            "plainText" | "timelineText" | "timelineTxt" | "srt" | "resolveMarkers"
        ) {
            self.export_format = defaults.export_format;
        }
        if !matches!(self.theme.as_str(), "light" | "dark") {
            self.theme = defaults.theme;
        }
        if self.term_categories.is_empty() {
            self.term_categories = defaults.term_categories;
        }

        self
    }
}
