use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub readiness: String,
    pub model_name: String,
    pub shortcut: String,
    pub microphone_name: String,
}

impl Default for AppSnapshot {
    fn default() -> Self {
        Self {
            readiness: "model-required".to_string(),
            model_name: "未配置模型".to_string(),
            shortcut: "CapsLock".to_string(),
            microphone_name: "默认麦克风".to_string(),
        }
    }
}
