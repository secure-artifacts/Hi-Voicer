mod app_state;
mod config;

use app_state::AppSnapshot;
use config::UserSettings;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs,
    fs::File,
    io::BufWriter,
    io::{self, Read, Write},
    net::TcpStream,
    path::{Component, Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Position, Size, State, WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutEvent, ShortcutState};

const SHERPA_DAEMON_PORT: u16 = 6127;
const SHERPA_WEBSOCKET_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const DAVINCI_TIMECODE_FPS: u64 = 25;
const LONG_AUDIO_CHUNK_SECONDS: u32 = 60;
const LONG_AUDIO_THRESHOLD_SECONDS: f64 = 300.0;
const SHERPA_RUNTIME_TAG: &str = "v1.13.2";
const SHERPA_CPU_RUNTIME_NAME: &str = "sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts";
const SHERPA_CPU_ARCHIVE_NAME: &str =
    "sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts.tar.bz2";
const SHERPA_CPU_RUNTIME_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts.tar.bz2";
const SHERPA_CUDA_RUNTIME_NAME: &str = "sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda";
const SHERPA_CUDA_ARCHIVE_NAME: &str =
    "sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda.tar.bz2";
const SHERPA_CUDA_RUNTIME_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda.tar.bz2";

struct RuntimeState {
    settings: Mutex<UserSettings>,
    recording: Mutex<Option<RecordingSession>>,
    sherpa_daemon: Mutex<Option<SherpaDaemon>>,
    sherpa_runtime_install: Mutex<()>,
    cuda_disabled_reason: Mutex<Option<String>>,
    cuda_startup_checked_runtime: Mutex<Option<String>>,
}

struct RecordingSession {
    stream: cpal::Stream,
    writer: Arc<Mutex<Option<hound::WavWriter<BufWriter<File>>>>>,
    path: PathBuf,
}

struct SherpaDaemon {
    child: Child,
    model_dir: String,
    executable: String,
}

impl Drop for SherpaDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// Hi-Voicer targets Windows desktop. The CPAL WASAPI stream is owned by this
// session and is only taken out to be dropped when recording stops.
unsafe impl Send for RecordingSession {}

#[cfg(windows)]
fn paste_text_to_active_window(text: &str) -> Result<(), String> {
    use std::mem::size_of;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL, VK_V,
    };

    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    clipboard
        .set_text(text.to_string())
        .map_err(|error| error.to_string())?;
    drop(clipboard);
    thread::sleep(Duration::from_millis(80));

    fn key_input(key: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    let inputs = [
        key_input(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_V, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_V, KEYEVENTF_KEYUP),
        key_input(VK_CONTROL, KEYEVENTF_KEYUP),
    ];

    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err("自动粘贴失败。".to_string());
    }

    Ok(())
}

#[cfg(not(windows))]
fn paste_text_to_active_window(_text: &str) -> Result<(), String> {
    Err("自动上屏当前只支持 Windows。".to_string())
}

fn normalize_shortcut(shortcut: &str) -> String {
    let trimmed = shortcut.trim();
    if trimmed.is_empty() {
        return "CapsLock".to_string();
    }

    trimmed
        .split('+')
        .map(|part| match part.trim() {
            "Win" => "Super".to_string(),
            "Esc" => "Escape".to_string(),
            value => value.to_string(),
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelInstallRequest {
    id: String,
    name: String,
    install_kind: String,
    download_url: String,
    archive_root: Option<String>,
    model_files: Vec<ModelFileRequest>,
    sherpa_args: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelFileRequest {
    url: String,
    path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstalledEngineConfig {
    engine: String,
    model_id: String,
    model_name: String,
    model_dir: String,
    executable: String,
    args: String,
    #[serde(default)]
    required_files: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ModelInstallProgress {
    model_id: String,
    message: String,
    completed: usize,
    total: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscribeFileRequest {
    audio_path: String,
    model_dir: String,
    task_id: Option<String>,
    #[serde(default = "default_performance_mode")]
    performance_mode: String,
    #[serde(default = "default_acceleration_mode")]
    acceleration_mode: String,
    #[serde(default = "default_export_format")]
    output_format: String,
    #[serde(default = "default_save_output")]
    save_output: bool,
}

fn default_save_output() -> bool {
    true
}

fn default_export_format() -> String {
    "plainText".to_string()
}

fn default_performance_mode() -> String {
    "balanced".to_string()
}

fn default_acceleration_mode() -> String {
    "cpu".to_string()
}

fn transcription_performance(mode: &str) -> TranscriptionPerformance {
    match mode {
        "stable" => TranscriptionPerformance {
            file_workers: 1,
            chunk_workers: 1,
            sherpa_threads: 4,
        },
        "fast" => TranscriptionPerformance {
            file_workers: 3,
            chunk_workers: 3,
            sherpa_threads: 2,
        },
        _ => TranscriptionPerformance {
            file_workers: 2,
            chunk_workers: 2,
            sherpa_threads: 2,
        },
    }
}

fn performance_for_acceleration(
    performance: TranscriptionPerformance,
    acceleration_mode: &str,
) -> TranscriptionPerformance {
    if acceleration_mode != "cuda" {
        return performance;
    }

    TranscriptionPerformance {
        file_workers: 1,
        chunk_workers: 1,
        sherpa_threads: performance.sherpa_threads.min(2).max(1),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelValidationRequest {
    model_dir: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccelerationStatusRequest {
    #[serde(default = "default_acceleration_mode")]
    acceleration_mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccelerationSmokeTestRequest {
    model_dir: String,
    #[serde(default = "default_acceleration_mode")]
    acceleration_mode: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelValidationResult {
    valid: bool,
    model_name: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccelerationStatus {
    selected_mode: String,
    effective_mode: String,
    cuda_available: bool,
    cuda_device_summary: Option<String>,
    cuda_detection_error: Option<String>,
    cpu_runtime_installed: bool,
    cuda_runtime_installed: bool,
    cuda_disabled_reason: Option<String>,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccelerationSmokeTestResult {
    requested_mode: String,
    used_mode: String,
    fallback_used: bool,
    elapsed_ms: u128,
    transcript_preview: String,
    message: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TranscribeFileResult {
    text: String,
    output_path: String,
    output_paths: Vec<String>,
    output_files: Vec<TranscriptionOutputFile>,
}

#[derive(Debug, Clone, Copy)]
struct TranscriptionPerformance {
    file_workers: usize,
    chunk_workers: usize,
    sherpa_threads: usize,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TranscriptionProgressEvent {
    task_id: String,
    stage: String,
    progress: u8,
    message: String,
    completed_segments: usize,
    total_segments: usize,
    elapsed_ms: u128,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TranscriptionOutputFile {
    format: String,
    label: String,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveTextFileRequest {
    suggested_name: String,
    contents: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveExistingFileRequest {
    source_path: String,
    suggested_name: String,
    destination_dir: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RecordingStateEvent {
    is_recording: bool,
    path: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RecordingLevelEvent {
    level: f32,
}

struct AudioLevelEmitter {
    app: AppHandle,
    last_emit: Mutex<Instant>,
}

fn unix_timestamp_millis() -> Result<u128, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis())
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(&config_dir).map_err(|error| error.to_string())?;
    Ok(config_dir.join("settings.json"))
}

fn installed_model_from_dir(model_dir: &Path) -> Option<(String, String)> {
    let model_dir_text = model_dir.to_string_lossy().to_string();
    if !validate_model_dir_path(&model_dir_text).valid {
        return None;
    }

    let raw_config = fs::read_to_string(model_dir.join("engine.json")).ok()?;
    let engine: InstalledEngineConfig = serde_json::from_str(&raw_config).ok()?;
    let model_id = if engine.model_id.trim().is_empty() {
        model_dir.file_name()?.to_string_lossy().to_string()
    } else {
        engine.model_id
    };

    Some((model_id, model_dir_text))
}

fn discover_installed_model_in_models_dir(
    models_dir: &Path,
    preferred_model_id: &str,
) -> Option<(String, String)> {
    const PREFERRED_MODELS: &[&str] = &[
        "sensevoice-small",
        "qwen3-asr-0.6b",
        "sherpa-funasr-nano",
        "whisper-base",
        "sherpa-paraformer-zh",
        "sherpa-zipformer-zh",
    ];

    let preferred_dir = models_dir.join(preferred_model_id);
    if let Some(model) = installed_model_from_dir(&preferred_dir) {
        return Some(model);
    }

    for model_id in PREFERRED_MODELS {
        if *model_id == preferred_model_id {
            continue;
        }

        let model_dir = models_dir.join(model_id);
        if let Some(model) = installed_model_from_dir(&model_dir) {
            return Some(model);
        }
    }

    let Ok(entries) = fs::read_dir(models_dir) else {
        return None;
    };

    for entry in entries.flatten() {
        let model_dir = entry.path();
        if model_dir.is_dir() {
            if let Some(model) = installed_model_from_dir(&model_dir) {
                return Some(model);
            }
        }
    }

    None
}

fn discover_installed_model(
    app: &AppHandle,
    preferred_model_id: &str,
) -> Result<Option<(String, String)>, String> {
    let models_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?
        .join("models");
    Ok(discover_installed_model_in_models_dir(
        &models_dir,
        preferred_model_id,
    ))
}

fn bind_installed_model_if_available(
    app: &AppHandle,
    mut settings: UserSettings,
) -> Result<UserSettings, String> {
    let configured_model_dir = settings.model_dir.trim();
    if !configured_model_dir.is_empty() && validate_model_dir_path(configured_model_dir).valid {
        if let Some((model_id, model_dir)) =
            installed_model_from_dir(&PathBuf::from(configured_model_dir))
        {
            settings.selected_model_id = model_id;
            settings.model_dir = model_dir;
        }
        return Ok(settings);
    }

    if let Some((model_id, model_dir)) = discover_installed_model(app, &settings.selected_model_id)?
    {
        settings.selected_model_id = model_id;
        settings.model_dir = model_dir;
    }

    Ok(settings)
}

fn read_settings(app: &AppHandle) -> Result<UserSettings, String> {
    let path = settings_path(app)?;
    let loaded_settings = if path.exists() {
        let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
        serde_json::from_str(&raw).map_err(|error| error.to_string())?
    } else {
        UserSettings::default()
    };

    let settings = bind_installed_model_if_available(app, loaded_settings.clone())?;
    if settings != loaded_settings {
        write_settings(app, &settings)?;
    }

    Ok(settings)
}

fn write_settings(app: &AppHandle, settings: &UserSettings) -> Result<(), String> {
    let path = settings_path(app)?;
    let raw = serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(path, raw).map_err(|error| error.to_string())
}

fn apply_mini_window_visibility(app: &AppHandle, visible: bool) {
    if let Some(window) = app.get_webview_window("mini") {
        let _ = window.set_size(Size::Logical(LogicalSize::new(46.0, 46.0)));
        if visible {
            let _ = window.show();
        } else {
            let _ = window.hide();
        }
    }
}

fn position_wave_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("wave") else {
        return;
    };

    let _ = window.set_size(Size::Logical(LogicalSize::new(156.0, 42.0)));
    if let Ok(Some(monitor)) = window.current_monitor() {
        let monitor_position = monitor.position();
        let monitor_size = monitor.size();
        let x = monitor_position.x + (monitor_size.width.saturating_sub(156) / 2) as i32;
        let y = monitor_position.y + monitor_size.height.saturating_sub(96) as i32;
        let _ = window.set_position(Position::Physical(PhysicalPosition::new(x, y)));
    }
}

fn apply_wave_window_visibility(app: &AppHandle, visible: bool) {
    if let Some(window) = app.get_webview_window("wave") {
        if visible {
            position_wave_window(app);
            let _ = window.show();
        } else {
            let _ = window.hide();
        }
    }
}

#[cfg(windows)]
fn apply_launch_at_startup(enabled: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "Hi-Voicer";

    let mut command = Command::new("reg");
    if enabled {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let exe_text = format!("\"{}\"", exe.to_string_lossy());
        command.args([
            "add",
            RUN_KEY,
            "/v",
            VALUE_NAME,
            "/t",
            "REG_SZ",
            "/d",
            exe_text.as_str(),
            "/f",
        ]);
    } else {
        command.args(["delete", RUN_KEY, "/v", VALUE_NAME, "/f"]);
    }
    command.creation_flags(CREATE_NO_WINDOW);

    let output = run_command_with_timeout(
        &mut command,
        Duration::from_secs(15),
        "Windows startup registry update",
    )?;
    if output.status.success() || !enabled {
        return Ok(());
    }

    Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

#[cfg(not(windows))]
fn apply_launch_at_startup(_enabled: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn suppress_command_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    command.creation_flags(0x08000000);
}

#[cfg(not(windows))]
fn suppress_command_window(_command: &mut Command) {}

fn run_command_with_timeout(
    command: &mut Command,
    timeout: Duration,
    description: &str,
) -> Result<Output, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture child stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture child stderr".to_string())?;
    let stdout_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stream = stdout;
        let _ = stream.read_to_end(&mut output);
        output
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stream = stderr;
        let _ = stream.read_to_end(&mut output);
        output
    });
    let started_at = Instant::now();

    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            let stdout = stdout_reader.join().unwrap_or_default();
            let stderr = stderr_reader.join().unwrap_or_default();
            return Ok(Output {
                status,
                stdout,
                stderr,
            });
        }

        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let status = child.wait().map_err(|error| error.to_string())?;
            let stdout = stdout_reader.join().unwrap_or_default();
            let stderr = stderr_reader.join().unwrap_or_default();
            let output = Output {
                status,
                stdout,
                stderr,
            };
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout}\n{stderr}");
            return Err(format!(
                "{description} timed out after {}s: {}",
                timeout.as_secs(),
                combined.trim()
            ));
        }

        thread::sleep(Duration::from_millis(100));
    }
}

fn extract_zip(zip_path: &Path, destination: &Path) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let Some(enclosed_name) = entry.enclosed_name() else {
            continue;
        };
        let out_path = destination.join(enclosed_name);

        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|error| error.to_string())?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }

        let mut out_file = fs::File::create(&out_path).map_err(|error| error.to_string())?;
        io::copy(&mut entry, &mut out_file).map_err(|error| error.to_string())?;
    }

    Ok(())
}

fn ensure_relative_file_path(path: &str) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(path);
    if candidate.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        return Err(format!("invalid model file path: {path}"));
    }

    Ok(candidate)
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("Hi-Voicer/0.1.0")
        .build()
        .map_err(|error| error.to_string())
}

fn download_file_once(url: &str, destination: &Path) -> Result<(), String> {
    let mut response = http_client()?
        .get(url)
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("download failed: {url} ({})", response.status()));
    }

    let temp_path = destination.with_extension("download");
    if temp_path.exists() {
        fs::remove_file(&temp_path).map_err(|error| error.to_string())?;
    }
    let mut file = fs::File::create(&temp_path).map_err(|error| error.to_string())?;
    io::copy(&mut response, &mut file).map_err(|error| error.to_string())?;
    fs::rename(temp_path, destination).map_err(|error| error.to_string())
}

fn download_file(url: &str, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let mut last_error = String::new();
    for attempt in 1..=3 {
        match download_file_once(url, destination) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = error;
                let temp_path = destination.with_extension("download");
                let _ = fs::remove_file(temp_path);
                if attempt < 3 {
                    thread::sleep(Duration::from_secs(attempt));
                }
            }
        }
    }

    Err(format!("download failed after 3 attempts: {last_error}"))
}

fn sherpa_runtime_executable_path(app: &AppHandle, runtime_name: &str) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?;
    Ok(data_dir
        .join("engines")
        .join("sherpa")
        .join(SHERPA_RUNTIME_TAG)
        .join(runtime_name)
        .join("bin")
        .join("sherpa-onnx-offline.exe"))
}

fn sherpa_runtime_dir_from_executable(executable: &Path) -> Result<PathBuf, String> {
    executable
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "invalid Sherpa-ONNX executable path: {}",
                executable.to_string_lossy()
            )
        })
}

fn install_sherpa_runtime_archive(
    app: &AppHandle,
    runtime_name: &str,
    archive_name: &str,
    runtime_url: &str,
) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?;
    let sherpa_root = data_dir
        .join("engines")
        .join("sherpa")
        .join(SHERPA_RUNTIME_TAG);
    let executable = sherpa_runtime_executable_path(app, runtime_name)?;

    if executable.exists() {
        return Ok(executable);
    }

    let state = app.try_state::<RuntimeState>();
    let _install_guard = state
        .as_ref()
        .map(|state| state.sherpa_runtime_install.lock())
        .transpose()
        .map_err(|error| error.to_string())?;

    if executable.exists() {
        return Ok(executable);
    }

    let runtime_dir = sherpa_runtime_dir_from_executable(&executable)?;
    fs::create_dir_all(&sherpa_root).map_err(|error| error.to_string())?;
    let archive_path = sherpa_root.join(archive_name);
    download_file(runtime_url, &archive_path)?;

    if runtime_dir.exists() {
        fs::remove_dir_all(&runtime_dir).map_err(|error| error.to_string())?;
    }

    let mut command = Command::new("tar");
    command
        .arg("-xjf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&sherpa_root);
    suppress_command_window(&mut command);

    let output = run_command_with_timeout(
        &mut command,
        Duration::from_secs(600),
        "Sherpa runtime extraction",
    )?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");
        let _ = fs::remove_dir_all(&runtime_dir);
        let _ = fs::remove_file(&archive_path);
        return Err(format!(
            "failed to extract Sherpa-ONNX runtime: {}",
            combined.trim()
        ));
    }

    if !executable.exists() {
        let _ = fs::remove_dir_all(&runtime_dir);
        let _ = fs::remove_file(&archive_path);
        return Err(format!(
            "Sherpa-ONNX executable was not found after extraction: {}",
            executable.to_string_lossy()
        ));
    }

    Ok(executable)
}

fn install_sherpa_runtime(app: &AppHandle) -> Result<PathBuf, String> {
    install_sherpa_runtime_archive(
        app,
        SHERPA_CPU_RUNTIME_NAME,
        SHERPA_CPU_ARCHIVE_NAME,
        SHERPA_CPU_RUNTIME_URL,
    )
}

fn install_sherpa_cuda_runtime(app: &AppHandle) -> Result<PathBuf, String> {
    install_sherpa_runtime_archive(
        app,
        SHERPA_CUDA_RUNTIME_NAME,
        SHERPA_CUDA_ARCHIVE_NAME,
        SHERPA_CUDA_RUNTIME_URL,
    )
}

fn emit_model_install_progress(
    app: &AppHandle,
    model_id: &str,
    message: String,
    completed: usize,
    total: usize,
) {
    let _ = app.emit(
        "model-install-progress",
        ModelInstallProgress {
            model_id: model_id.to_string(),
            message,
            completed,
            total,
        },
    );
}

fn find_file_recursive(root: &Path, file_name: &str) -> Result<Option<PathBuf>, String> {
    if !root.exists() {
        return Ok(None);
    }

    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, file_name)? {
                return Ok(Some(found));
            }
            continue;
        }

        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(file_name))
        {
            return Ok(Some(path));
        }
    }

    Ok(None)
}

fn install_ffmpeg_runtime(app: &AppHandle) -> Result<PathBuf, String> {
    const FFMPEG_URL: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";

    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?;
    let ffmpeg_root = data_dir.join("engines").join("ffmpeg");
    if let Some(executable) = find_file_recursive(&ffmpeg_root, "ffmpeg.exe")? {
        return Ok(executable);
    }

    fs::create_dir_all(&ffmpeg_root).map_err(|error| error.to_string())?;
    let archive_path = ffmpeg_root.join("ffmpeg-master-latest-win64-gpl.zip");
    download_file(FFMPEG_URL, &archive_path)?;
    extract_zip(&archive_path, &ffmpeg_root)?;

    find_file_recursive(&ffmpeg_root, "ffmpeg.exe")?
        .ok_or_else(|| "ffmpeg 下载完成，但没有找到 ffmpeg.exe。".to_string())
}

fn install_sherpa_model(app: AppHandle, model: ModelInstallRequest) -> Result<String, String> {
    if model.model_files.is_empty() || model.sherpa_args.trim().is_empty() {
        return Err(format!("{} has no Sherpa install recipe", model.name));
    }

    let total_steps = model.model_files.len() + 3;
    emit_model_install_progress(
        &app,
        &model.id,
        "正在准备 Sherpa-ONNX 运行时...".to_string(),
        0,
        total_steps,
    );
    let executable = install_sherpa_runtime(&app)?;
    emit_model_install_progress(
        &app,
        &model.id,
        "Sherpa-ONNX 运行时已就绪。".to_string(),
        1,
        total_steps,
    );
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?;
    let model_dir = data_dir.join("models").join(&model.id);
    fs::create_dir_all(&model_dir).map_err(|error| error.to_string())?;

    for (index, model_file) in model.model_files.iter().enumerate() {
        emit_model_install_progress(
            &app,
            &model.id,
            format!(
                "正在下载模型文件 {}/{}：{}",
                index + 1,
                model.model_files.len(),
                model_file.path
            ),
            index + 1,
            total_steps,
        );
        let relative_path = ensure_relative_file_path(&model_file.path)?;
        download_file(&model_file.url, &model_dir.join(relative_path))?;
    }

    emit_model_install_progress(
        &app,
        &model.id,
        "正在写入本地模型配置...".to_string(),
        total_steps - 1,
        total_steps,
    );

    let model_dir_text = model_dir.to_string_lossy().to_string();
    let executable_text = executable.to_string_lossy().to_string();
    let args = model
        .sherpa_args
        .replace("{modelDir}", &model_dir_text)
        .replace("{exePath}", &executable_text);
    let config = InstalledEngineConfig {
        engine: "sherpa-onnx".to_string(),
        model_id: model.id.clone(),
        model_name: model.name.clone(),
        model_dir: model_dir_text.clone(),
        executable: executable_text,
        args,
        required_files: model
            .model_files
            .iter()
            .map(|model_file| model_file.path.clone())
            .collect(),
    };

    let raw = serde_json::to_string_pretty(&config).map_err(|error| error.to_string())?;
    fs::write(model_dir.join("engine.json"), raw).map_err(|error| error.to_string())?;

    emit_model_install_progress(
        &app,
        &model.id,
        format!("{} 已安装完成。", model.name),
        total_steps,
        total_steps,
    );

    Ok(model_dir_text)
}

fn install_zip_model(app: AppHandle, model: ModelInstallRequest) -> Result<String, String> {
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?;
    let models_dir = data_dir.join("models");
    let downloads_dir = data_dir.join("downloads");
    let install_dir = models_dir.join(&model.id);

    if install_dir.exists() {
        return Ok(install_dir.to_string_lossy().to_string());
    }

    fs::create_dir_all(&models_dir).map_err(|error| error.to_string())?;
    fs::create_dir_all(&downloads_dir).map_err(|error| error.to_string())?;

    let archive_path = downloads_dir.join(format!("{}.zip", model.id));
    let mut response = http_client()?
        .get(&model.download_url)
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("下载 {} 失败：{}", model.name, response.status()));
    }

    let mut archive_file = fs::File::create(&archive_path).map_err(|error| error.to_string())?;
    io::copy(&mut response, &mut archive_file).map_err(|error| error.to_string())?;

    let temp_dir = models_dir.join(format!("{}.tmp", model.id));
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&temp_dir).map_err(|error| error.to_string())?;
    extract_zip(&archive_path, &temp_dir)?;

    let extracted_root = model
        .archive_root
        .as_ref()
        .map(|root| temp_dir.join(root))
        .filter(|path| path.exists())
        .unwrap_or_else(|| temp_dir.clone());

    if install_dir.exists() {
        fs::remove_dir_all(&install_dir).map_err(|error| error.to_string())?;
    }
    fs::rename(&extracted_root, &install_dir).map_err(|error| error.to_string())?;

    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir).map_err(|error| error.to_string())?;
    }

    Ok(install_dir.to_string_lossy().to_string())
}

fn validate_model_dir_path(model_dir: &str) -> ModelValidationResult {
    let trimmed = model_dir.trim();
    if trimmed.is_empty() {
        return ModelValidationResult {
            valid: false,
            model_name: String::new(),
            message: "尚未配置离线模型。".to_string(),
        };
    }

    let model_dir = PathBuf::from(trimmed);
    if !model_dir.exists() {
        return ModelValidationResult {
            valid: false,
            model_name: String::new(),
            message: "模型目录不存在，请重新下载或重新选择模型目录。".to_string(),
        };
    }

    let engine_path = model_dir.join("engine.json");
    if !engine_path.exists() {
        return ModelValidationResult {
            valid: false,
            model_name: String::new(),
            message: "模型目录里没有 engine.json，请在设置里用“一键下载并配置”。".to_string(),
        };
    }

    let raw_config = match fs::read_to_string(engine_path) {
        Ok(raw_config) => raw_config,
        Err(error) => {
            return ModelValidationResult {
                valid: false,
                model_name: String::new(),
                message: format!("读取模型配置失败：{error}"),
            }
        }
    };
    let engine: InstalledEngineConfig = match serde_json::from_str(&raw_config) {
        Ok(engine) => engine,
        Err(error) => {
            return ModelValidationResult {
                valid: false,
                model_name: String::new(),
                message: format!("模型配置格式不正确：{error}"),
            }
        }
    };

    if engine.engine != "sherpa-onnx" {
        return ModelValidationResult {
            valid: false,
            model_name: engine.model_name,
            message: format!("暂不支持这个引擎：{}", engine.engine),
        };
    }

    if !PathBuf::from(&engine.executable).exists() {
        return ModelValidationResult {
            valid: false,
            model_name: engine.model_name,
            message: "Sherpa-ONNX 程序不存在，请重新下载并配置模型。".to_string(),
        };
    }

    for required_file in &engine.required_files {
        let relative_path = match ensure_relative_file_path(required_file) {
            Ok(relative_path) => relative_path,
            Err(error) => {
                return ModelValidationResult {
                    valid: false,
                    model_name: engine.model_name.clone(),
                    message: format!("模型配置里的文件路径不安全：{error}"),
                }
            }
        };

        if !model_dir.join(relative_path).exists() {
            return ModelValidationResult {
                valid: false,
                model_name: engine.model_name.clone(),
                message: format!("模型文件缺失：{required_file}。请在设置里重新下载并配置模型。"),
            };
        }
    }

    ModelValidationResult {
        valid: true,
        model_name: engine.model_name,
        message: "模型已就绪。".to_string(),
    }
}

fn split_command_args(args: &str) -> Result<Vec<String>, String> {
    let mut parsed = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = args.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '\\' if matches!(chars.peek(), Some('"')) => {
                current.push('"');
                chars.next();
            }
            ch if ch.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    parsed.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(ch),
        }
    }

    if in_quotes {
        return Err("Sherpa 参数里的引号没有闭合。".to_string());
    }

    if !current.is_empty() {
        parsed.push(current);
    }

    Ok(parsed)
}

fn set_sherpa_arg_value(
    parsed: &mut Vec<String>,
    name: &str,
    value: &str,
    append_if_missing: bool,
) {
    let mut replaced = false;
    let prefix = format!("{name}=");
    let mut index = 0;
    while index < parsed.len() {
        if parsed[index] == name {
            if index + 1 < parsed.len() {
                parsed[index + 1] = value.to_string();
            } else {
                parsed.push(value.to_string());
            }
            replaced = true;
            index += 2;
            continue;
        }

        if parsed[index].starts_with(&prefix) {
            parsed[index] = format!("{prefix}{value}");
            replaced = true;
        }
        index += 1;
    }

    if append_if_missing && !replaced {
        parsed.push(format!("{name}={value}"));
    }
}

fn sherpa_args_for_runtime(
    args: &str,
    threads: Option<usize>,
    runtime_mode: &str,
) -> Result<Vec<String>, String> {
    let mut parsed = split_command_args(args)?;
    if let Some(threads) = threads {
        set_sherpa_arg_value(&mut parsed, "--num-threads", &threads.to_string(), true);
    }

    if runtime_mode.eq_ignore_ascii_case("cuda") {
        set_sherpa_arg_value(&mut parsed, "--provider", "cuda", true);
    }

    Ok(parsed)
}

fn text_from_sherpa_json_value(value: &serde_json::Value) -> Option<String> {
    let text = value.get("text").and_then(|text| text.as_str())?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn extract_sherpa_json_text(trimmed: &str) -> Option<String> {
    if !trimmed.starts_with('{') {
        return None;
    }

    let value = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    text_from_sherpa_json_value(&value)
}

fn extract_sherpa_json_texts(output: &str) -> Vec<String> {
    let mut texts = Vec::new();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in output.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start_index) = start.take() {
                        let end_index = index + ch.len_utf8();
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(
                            &output[start_index..end_index],
                        ) {
                            if let Some(text) = text_from_sherpa_json_value(&value) {
                                texts.push(text);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    texts
}

fn is_sherpa_status_line(trimmed: &str) -> bool {
    let lower = trimmed.to_ascii_lowercase();

    trimmed == "Started"
        || trimmed == "Done!"
        || trimmed == "----"
        || lower.starts_with("num threads:")
        || lower.starts_with("decoding method:")
        || lower.starts_with("elapsed seconds:")
        || lower.starts_with("real time factor")
        || lower.starts_with("creating recognizer")
        || lower.starts_with("recognizer created")
        || lower.starts_with("offlinerecognizerconfig")
        || lower.starts_with("loading model")
        || lower.contains("parse-options.cc:read:")
        || lower.contains("sherpa-onnx")
        || lower.ends_with(".wav")
        || lower.ends_with(".mp3")
        || lower.ends_with(".m4a")
        || lower.ends_with(".flac")
        || lower.ends_with(".ogg")
        || lower.ends_with(".mp4")
}

fn extract_transcription_text(output: &str) -> String {
    let mut best = String::new();

    for text in extract_sherpa_json_texts(output) {
        best = text;
    }

    if !best.is_empty() {
        return best;
    }

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(text) = extract_sherpa_json_text(trimmed) {
            best = text;
            continue;
        }

        if is_sherpa_status_line(trimmed) {
            continue;
        }

        for marker in ["Decoded text:", "Result:", "result:"] {
            if let Some((_, text)) = trimmed.split_once(marker) {
                let text = text.trim();
                if !text.is_empty() {
                    best = text.to_string();
                    continue;
                }
            }
        }
    }

    best
}

fn media_to_sherpa_wav(app: &AppHandle, input_path: &Path) -> Result<PathBuf, String> {
    let ffmpeg = install_ffmpeg_runtime(app)?;
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?;
    let transcode_dir = data_dir.join("transcodes");
    fs::create_dir_all(&transcode_dir).map_err(|error| error.to_string())?;

    let stem = input_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    let output_path = transcode_dir.join(format!("{stem}-16k-mono.wav"));

    let mut command = Command::new(ffmpeg);
    command
        .arg("-y")
        .arg("-i")
        .arg(input_path)
        .arg("-vn")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg("16000")
        .arg("-sample_fmt")
        .arg("s16")
        .arg(&output_path);
    suppress_command_window(&mut command);

    let output = command.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg 转码失败：{}", stderr.trim()));
    }

    Ok(output_path)
}

fn read_sherpa_engine(model_dir: &Path) -> Result<InstalledEngineConfig, String> {
    let engine_path = model_dir.join("engine.json");
    if !engine_path.exists() {
        return Err("模型目录里没有 engine.json，请先在设置里下载并配置 Sherpa 模型。".to_string());
    }

    let raw_config = fs::read_to_string(engine_path).map_err(|error| error.to_string())?;
    let engine: InstalledEngineConfig =
        serde_json::from_str(&raw_config).map_err(|error| error.to_string())?;
    if engine.engine != "sherpa-onnx" {
        return Err(format!("暂不支持这个引擎：{}", engine.engine));
    }

    Ok(engine)
}

#[derive(Debug, Clone)]
struct ResolvedSherpaRuntime {
    executable: PathBuf,
    mode: String,
    fallback_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct NvidiaCudaInfo {
    available: bool,
    device_summary: Option<String>,
    detection_error: Option<String>,
}

fn parse_nvidia_smi_query_output(output: &str) -> Option<String> {
    let devices = output
        .lines()
        .filter_map(|line| {
            let parts = line.split(',').map(str::trim).collect::<Vec<_>>();
            if parts.len() < 3 {
                return None;
            }
            let name = parts[0].trim();
            let driver = parts[1].trim();
            let memory_mb = parts[2].trim();
            if name.is_empty() {
                return None;
            }

            let driver_text = if driver.is_empty() {
                "driver unknown".to_string()
            } else {
                format!("driver {driver}")
            };
            let memory_text = if memory_mb.is_empty() {
                "VRAM unknown".to_string()
            } else {
                format!("VRAM {memory_mb} MB")
            };
            Some(format!("{name} / {driver_text} / {memory_text}"))
        })
        .collect::<Vec<_>>();

    if devices.is_empty() {
        None
    } else {
        Some(devices.join("; "))
    }
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn nvidia_smi_candidate_paths(
    system_root: Option<&str>,
    windir: Option<&str>,
    program_files: Option<&str>,
) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("nvidia-smi")];

    for root in [system_root, windir].into_iter().flatten() {
        push_unique_path(
            &mut paths,
            PathBuf::from(root).join("System32").join("nvidia-smi.exe"),
        );
    }

    if let Some(program_files) = program_files {
        push_unique_path(
            &mut paths,
            PathBuf::from(program_files)
                .join("NVIDIA Corporation")
                .join("NVSMI")
                .join("nvidia-smi.exe"),
        );
    }

    paths
}

fn nvidia_smi_candidates() -> Vec<PathBuf> {
    nvidia_smi_candidate_paths(
        std::env::var("SystemRoot").ok().as_deref(),
        std::env::var("WINDIR").ok().as_deref(),
        std::env::var("ProgramFiles").ok().as_deref(),
    )
}

fn query_nvidia_cuda_info() -> NvidiaCudaInfo {
    query_nvidia_cuda_info_from_candidates(&nvidia_smi_candidates())
}

fn nvidia_cuda_info_from_output(
    candidate: &Path,
    output: Output,
) -> Result<NvidiaCudaInfo, String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    if !output.status.success() {
        return Err(format!(
            "{} exited with {}: {}",
            candidate.to_string_lossy(),
            output.status,
            combined.trim()
        ));
    }

    let Some(device_summary) = parse_nvidia_smi_query_output(&stdout) else {
        return Err(format!(
            "{} returned no parseable GPU rows: {}",
            candidate.to_string_lossy(),
            combined.trim()
        ));
    };

    Ok(NvidiaCudaInfo {
        available: true,
        device_summary: Some(device_summary),
        detection_error: None,
    })
}

fn query_nvidia_cuda_info_from_candidates(candidates: &[PathBuf]) -> NvidiaCudaInfo {
    let mut errors = Vec::new();

    for candidate in candidates {
        let mut command = Command::new(candidate);
        command.args([
            "--query-gpu=name,driver_version,memory.total",
            "--format=csv,noheader,nounits",
        ]);
        suppress_command_window(&mut command);

        match run_command_with_timeout(
            &mut command,
            Duration::from_secs(5),
            "NVIDIA CUDA detection",
        ) {
            Ok(output) => match nvidia_cuda_info_from_output(candidate, output) {
                Ok(info) => return info,
                Err(error) => errors.push(error),
            },
            Err(error) => errors.push(format!("{}: {error}", candidate.to_string_lossy())),
        }
    }

    let tried = candidates
        .iter()
        .map(|candidate| candidate.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    NvidiaCudaInfo {
        available: false,
        device_summary: None,
        detection_error: Some(format!(
            "未找到可用的 nvidia-smi；已尝试：{tried}。{}",
            errors.join("；")
        )),
    }
}

fn smoke_test_sherpa_runtime(executable: &Path) -> Result<(), String> {
    if !executable.exists() {
        return Err(format!(
            "runtime executable not found: {}",
            executable.to_string_lossy()
        ));
    }

    let mut command = Command::new(executable);
    command.arg("--help");
    if let Some(parent) = executable.parent() {
        command.current_dir(parent);
    }
    suppress_command_window(&mut command);

    let output = run_command_with_timeout(
        &mut command,
        Duration::from_secs(15),
        "Sherpa runtime startup check",
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    let combined_lower = combined.to_ascii_lowercase();

    if output.status.success()
        || combined_lower.contains("usage")
        || combined_lower.contains("sherpa-onnx")
    {
        return Ok(());
    }

    Err(format!("runtime startup check failed: {}", combined.trim()))
}

fn cuda_runtime_check_matches(checked_runtime: Option<&str>, executable: &Path) -> bool {
    let executable_text = executable.to_string_lossy();
    match checked_runtime {
        Some(checked_runtime) => checked_runtime == executable_text.as_ref(),
        None => false,
    }
}

fn is_cuda_runtime_startup_checked(app: &AppHandle, executable: &Path) -> bool {
    let Some(state) = app.try_state::<RuntimeState>() else {
        return false;
    };
    state
        .cuda_startup_checked_runtime
        .lock()
        .ok()
        .map(|checked_runtime| cuda_runtime_check_matches(checked_runtime.as_deref(), executable))
        .unwrap_or(false)
}

fn mark_cuda_runtime_startup_checked(app: &AppHandle, executable: &Path) {
    if let Some(state) = app.try_state::<RuntimeState>() {
        if let Ok(mut checked_runtime) = state.cuda_startup_checked_runtime.lock() {
            *checked_runtime = Some(executable.to_string_lossy().to_string());
        }
    }
}

fn clear_cuda_runtime_startup_checked(app: &AppHandle) {
    if let Some(state) = app.try_state::<RuntimeState>() {
        if let Ok(mut checked_runtime) = state.cuda_startup_checked_runtime.lock() {
            *checked_runtime = None;
        }
    }
}

fn cuda_circuit_reason(app: &AppHandle) -> Option<String> {
    let state = app.try_state::<RuntimeState>()?;
    state
        .cuda_disabled_reason
        .lock()
        .ok()
        .and_then(|reason| reason.clone())
}

fn trip_cuda_circuit(app: &AppHandle, reason: String) {
    if let Some(state) = app.try_state::<RuntimeState>() {
        if let Ok(mut disabled_reason) = state.cuda_disabled_reason.lock() {
            *disabled_reason = Some(reason);
        }
    }
    clear_cuda_runtime_startup_checked(app);
}

fn clear_cuda_circuit(app: &AppHandle) {
    if let Some(state) = app.try_state::<RuntimeState>() {
        if let Ok(mut disabled_reason) = state.cuda_disabled_reason.lock() {
            *disabled_reason = None;
        }
    }
}

fn acceleration_status_from_parts(
    selected_mode: &str,
    cuda_available: bool,
    cuda_device_summary: Option<String>,
    cuda_detection_error: Option<String>,
    cpu_runtime_installed: bool,
    cuda_runtime_installed: bool,
    cuda_disabled_reason: Option<String>,
    prepare_error: Option<String>,
) -> AccelerationStatus {
    let (effective_mode, message) = if selected_mode == "cuda" {
        if let Some(reason) = cuda_disabled_reason.as_ref() {
            (
                "cpu",
                format!("CUDA 本次会话已停用，转录会直接使用 CPU：{reason}"),
            )
        } else if let Some(error) = prepare_error {
            (
                "cpu",
                format!("CUDA runtime 准备失败；转录时会回退 CPU：{error}"),
            )
        } else {
            match (cuda_available, cuda_runtime_installed) {
                (false, _) => (
                    "cpu",
                    "已选择 CUDA，但未检测到 NVIDIA CUDA 环境；转录时会自动回退 CPU。".to_string(),
                ),
                (true, false) => (
                    "cuda",
                    "已检测到 NVIDIA 环境；首次 CUDA 转录会下载 CUDA runtime，失败会回退 CPU。"
                        .to_string(),
                ),
                (true, true) => (
                    "cuda",
                    "CUDA 环境和 CUDA runtime 已就绪；识别失败时仍会自动回退 CPU。".to_string(),
                ),
            }
        }
    } else if let Some(error) = prepare_error {
        (
            "cpu",
            format!("CUDA runtime 准备失败；转录时会回退 CPU：{error}"),
        )
    } else {
        (
            "cpu",
            "当前选择 CPU，兼容性最高；不会尝试加载 CUDA runtime。".to_string(),
        )
    };

    AccelerationStatus {
        selected_mode: selected_mode.to_string(),
        effective_mode: effective_mode.to_string(),
        cuda_available,
        cuda_device_summary,
        cuda_detection_error,
        cpu_runtime_installed,
        cuda_runtime_installed,
        cuda_disabled_reason,
        message,
    }
}

fn acceleration_status_for_app(
    app: &AppHandle,
    requested_mode: &str,
) -> Result<AccelerationStatus, String> {
    let selected_mode = if requested_mode.trim().eq_ignore_ascii_case("cuda") {
        "cuda"
    } else {
        "cpu"
    };
    let cuda_info = query_nvidia_cuda_info();
    let cpu_runtime_installed =
        sherpa_runtime_executable_path(app, SHERPA_CPU_RUNTIME_NAME)?.exists();
    let cuda_runtime_installed =
        sherpa_runtime_executable_path(app, SHERPA_CUDA_RUNTIME_NAME)?.exists();

    Ok(acceleration_status_from_parts(
        selected_mode,
        cuda_info.available,
        cuda_info.device_summary,
        cuda_info.detection_error,
        cpu_runtime_installed,
        cuda_runtime_installed,
        cuda_circuit_reason(app),
        None,
    ))
}

fn prepare_acceleration_runtime_for_app(
    app: &AppHandle,
    requested_mode: &str,
) -> Result<AccelerationStatus, String> {
    let selected_mode = if requested_mode.trim().eq_ignore_ascii_case("cuda") {
        "cuda"
    } else {
        "cpu"
    };

    if selected_mode == "cpu" {
        return acceleration_status_for_app(app, requested_mode);
    }

    let cuda_info = query_nvidia_cuda_info();
    if !cuda_info.available {
        return acceleration_status_for_app(app, requested_mode);
    }

    let prepare_error = match install_sherpa_cuda_runtime(app) {
        Ok(executable) => match smoke_test_sherpa_runtime(&executable) {
            Ok(()) => {
                mark_cuda_runtime_startup_checked(app, &executable);
                None
            }
            Err(error) => {
                clear_cuda_runtime_startup_checked(app);
                Some(error)
            }
        },
        Err(error) => Some(error),
    };
    if let Some(error) = prepare_error.as_ref() {
        trip_cuda_circuit(app, error.clone());
    } else {
        clear_cuda_circuit(app);
    }
    let cpu_runtime_installed =
        sherpa_runtime_executable_path(app, SHERPA_CPU_RUNTIME_NAME)?.exists();
    let cuda_runtime_installed =
        sherpa_runtime_executable_path(app, SHERPA_CUDA_RUNTIME_NAME)?.exists();

    let mut status = acceleration_status_from_parts(
        selected_mode,
        cuda_info.available,
        cuda_info.device_summary,
        cuda_info.detection_error,
        cpu_runtime_installed,
        cuda_runtime_installed,
        cuda_circuit_reason(app),
        prepare_error,
    );
    if status.effective_mode == "cuda" {
        status.message =
            "CUDA runtime 已下载并通过启动检查；识别失败时仍会自动回退 CPU。".to_string();
    }

    Ok(status)
}

fn resolve_sherpa_runtime(
    app: &AppHandle,
    engine: &InstalledEngineConfig,
    requested_mode: &str,
) -> ResolvedSherpaRuntime {
    let cpu_executable = PathBuf::from(&engine.executable);
    let mode = requested_mode.trim().to_ascii_lowercase();
    if mode != "cuda" {
        return ResolvedSherpaRuntime {
            executable: cpu_executable,
            mode: "cpu".to_string(),
            fallback_reason: None,
        };
    }

    if let Some(reason) = cuda_circuit_reason(app) {
        return ResolvedSherpaRuntime {
            executable: cpu_executable,
            mode: "cpu".to_string(),
            fallback_reason: Some(format!("CUDA 本次会话已停用，已直接使用 CPU：{reason}")),
        };
    }

    let cuda_info = query_nvidia_cuda_info();
    if !cuda_info.available {
        return ResolvedSherpaRuntime {
            executable: cpu_executable,
            mode: "cpu".to_string(),
            fallback_reason: Some("未检测到可用的 NVIDIA CUDA 环境，已回退到 CPU。".to_string()),
        };
    }

    match install_sherpa_cuda_runtime(app) {
        Ok(executable) => {
            if !is_cuda_runtime_startup_checked(app, &executable) {
                if let Err(error) = smoke_test_sherpa_runtime(&executable) {
                    trip_cuda_circuit(app, error.clone());
                    return ResolvedSherpaRuntime {
                        executable: cpu_executable,
                        mode: "cpu".to_string(),
                        fallback_reason: Some(format!(
                            "CUDA runtime 启动检查失败，已回退到 CPU：{error}"
                        )),
                    };
                }
                mark_cuda_runtime_startup_checked(app, &executable);
            }

            ResolvedSherpaRuntime {
                executable,
                mode: "cuda".to_string(),
                fallback_reason: None,
            }
        }
        Err(error) => {
            trip_cuda_circuit(app, error.clone());
            ResolvedSherpaRuntime {
                executable: cpu_executable,
                mode: "cpu".to_string(),
                fallback_reason: Some(format!("CUDA 运行时准备失败，已回退到 CPU：{error}")),
            }
        }
    }
}

fn sherpa_websocket_server_path(executable: &Path) -> PathBuf {
    executable.with_file_name("sherpa-onnx-offline-websocket-server.exe")
}

fn ensure_sherpa_daemon_running(
    app: &AppHandle,
    engine: &InstalledEngineConfig,
    executable: &Path,
    runtime_mode: &str,
) -> Result<(), String> {
    let server_exe = sherpa_websocket_server_path(executable);
    if !server_exe.exists() {
        return Err("Sherpa WebSocket 服务程序不存在，无法启用极速模式。".to_string());
    }

    let state = app.state::<RuntimeState>();
    let mut daemon = state
        .sherpa_daemon
        .lock()
        .map_err(|error| error.to_string())?;

    if let Some(existing) = daemon.as_mut() {
        let still_running = existing
            .child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none();
        let server_exe_text = server_exe.to_string_lossy().to_string();
        if still_running
            && existing.model_dir == engine.model_dir
            && existing.executable == server_exe_text
        {
            return Ok(());
        }
    }

    *daemon = None;

    let mut command = Command::new(&server_exe);
    command.args(sherpa_args_for_runtime(&engine.args, None, runtime_mode)?);
    command.arg(format!("--port={SHERPA_DAEMON_PORT}"));
    if let Some(parent) = server_exe.parent() {
        command.current_dir(parent);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    suppress_command_window(&mut command);

    let child = command.spawn().map_err(|error| error.to_string())?;
    *daemon = Some(SherpaDaemon {
        child,
        model_dir: engine.model_dir.clone(),
        executable: server_exe.to_string_lossy().to_string(),
    });

    Ok(())
}

fn warm_sherpa_daemon(app: AppHandle, model_dir: String) {
    if model_dir.trim().is_empty() {
        return;
    }

    let model_dir = PathBuf::from(model_dir);
    if let Ok(engine) = read_sherpa_engine(&model_dir) {
        let executable = PathBuf::from(&engine.executable);
        let _ = ensure_sherpa_daemon_running(&app, &engine, &executable, "cpu");
    }
}

fn wav_to_sherpa_websocket_payload(wav_path: &Path) -> Result<Vec<u8>, String> {
    let mut reader = hound::WavReader::open(wav_path).map_err(|error| error.to_string())?;
    let spec = reader.spec();
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Err("Sherpa 极速模式只支持 16-bit PCM WAV。".to_string());
    }

    let samples = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let audio_bytes = samples
        .len()
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or_else(|| "音频太大，无法发送到 Sherpa 服务。".to_string())?;

    let mut payload = Vec::with_capacity(8 + audio_bytes);
    payload.extend_from_slice(&(spec.sample_rate as i32).to_le_bytes());
    payload.extend_from_slice(&(audio_bytes as i32).to_le_bytes());
    for sample in samples {
        payload.extend_from_slice(&((sample as f32) / 32768.0).to_le_bytes());
    }

    Ok(payload)
}

fn websocket_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    const MASK: [u8; 4] = [0x12, 0x34, 0x56, 0x78];
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | (opcode & 0x0f));

    if payload.len() <= 125 {
        frame.push(0x80 | payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }

    frame.extend_from_slice(&MASK);
    for (index, byte) in payload.iter().enumerate() {
        frame.push(byte ^ MASK[index % MASK.len()]);
    }

    frame
}

fn read_websocket_message(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut message = Vec::new();

    loop {
        let mut header = [0u8; 2];
        stream
            .read_exact(&mut header)
            .map_err(|error| error.to_string())?;
        let fin = header[0] & 0x80 != 0;
        let opcode = header[0] & 0x0f;
        let masked = header[1] & 0x80 != 0;
        let mut len = (header[1] & 0x7f) as u64;

        if len == 126 {
            let mut bytes = [0u8; 2];
            stream
                .read_exact(&mut bytes)
                .map_err(|error| error.to_string())?;
            len = u16::from_be_bytes(bytes) as u64;
        } else if len == 127 {
            let mut bytes = [0u8; 8];
            stream
                .read_exact(&mut bytes)
                .map_err(|error| error.to_string())?;
            len = u64::from_be_bytes(bytes);
        }

        let mut mask = [0u8; 4];
        if masked {
            stream
                .read_exact(&mut mask)
                .map_err(|error| error.to_string())?;
        }

        let mut payload = vec![0u8; len as usize];
        stream
            .read_exact(&mut payload)
            .map_err(|error| error.to_string())?;
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % mask.len()];
            }
        }

        match opcode {
            0x0 | 0x1 | 0x2 => message.extend_from_slice(&payload),
            0x8 => return Err("Sherpa WebSocket 服务提前关闭连接。".to_string()),
            _ => {}
        }

        if fin && !message.is_empty() {
            return Ok(message);
        }
    }
}

fn transcribe_sherpa_wav_websocket(wav_path: &Path) -> Result<String, String> {
    let duration = wav_duration_seconds(wav_path).unwrap_or(60.0);
    let read_timeout = Duration::from_secs(((duration * 2.0) as u64 + 120).clamp(120, 14_400));
    let write_timeout = Duration::from_secs(((duration / 5.0) as u64 + 120).clamp(120, 1_800));
    let payload = wav_to_sherpa_websocket_payload(wav_path)?;
    let mut last_error = String::new();

    for _ in 0..60 {
        match TcpStream::connect(("127.0.0.1", SHERPA_DAEMON_PORT)) {
            Ok(mut stream) => {
                stream
                    .set_read_timeout(Some(read_timeout))
                    .map_err(|error| error.to_string())?;
                stream
                    .set_write_timeout(Some(write_timeout))
                    .map_err(|error| error.to_string())?;

                let handshake = format!(
                    "GET / HTTP/1.1\r\nHost: 127.0.0.1:{SHERPA_DAEMON_PORT}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {SHERPA_WEBSOCKET_KEY}\r\nSec-WebSocket-Version: 13\r\n\r\n"
                );
                stream
                    .write_all(handshake.as_bytes())
                    .map_err(|error| error.to_string())?;

                let mut response = Vec::new();
                let mut buffer = [0u8; 512];
                loop {
                    let read = stream
                        .read(&mut buffer)
                        .map_err(|error| error.to_string())?;
                    if read == 0 {
                        return Err("Sherpa WebSocket 握手失败。".to_string());
                    }
                    response.extend_from_slice(&buffer[..read]);
                    if response.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }

                let response_text = String::from_utf8_lossy(&response);
                if !response_text.starts_with("HTTP/1.1 101")
                    && !response_text.starts_with("HTTP/1.0 101")
                {
                    return Err(format!("Sherpa WebSocket 握手失败：{response_text}"));
                }

                stream
                    .write_all(&websocket_frame(0x2, &payload))
                    .map_err(|error| error.to_string())?;
                stream
                    .write_all(&websocket_frame(0x1, b"Done"))
                    .map_err(|error| error.to_string())?;
                let response = read_websocket_message(&mut stream)?;
                let text = extract_transcription_text(&String::from_utf8_lossy(&response));
                return Ok(text);
            }
            Err(error) => {
                last_error = error.to_string();
                thread::sleep(Duration::from_millis(100));
            }
        }
    }

    Err(format!("Sherpa WebSocket 服务未就绪：{last_error}"))
}

fn wav_duration_seconds(wav_path: &Path) -> Result<f64, String> {
    let reader = hound::WavReader::open(wav_path).map_err(|error| error.to_string())?;
    let spec = reader.spec();
    if spec.sample_rate == 0 {
        return Ok(1.0);
    }

    let channel_count = u32::from(spec.channels).max(1);
    Ok(reader.duration() as f64 / spec.sample_rate as f64 / channel_count as f64)
}

fn split_wav_into_chunks_in_dir(
    wav_path: &Path,
    chunk_dir: &Path,
    chunk_seconds: u32,
) -> Result<Vec<PathBuf>, String> {
    let mut reader = hound::WavReader::open(wav_path).map_err(|error| error.to_string())?;
    let spec = reader.spec();
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Err("长音频分段只支持 16-bit PCM WAV。".to_string());
    }

    fs::create_dir_all(chunk_dir).map_err(|error| error.to_string())?;
    let samples_per_chunk = (spec.sample_rate as usize)
        .saturating_mul(spec.channels as usize)
        .saturating_mul(chunk_seconds.max(1) as usize)
        .max(1);
    let stem = wav_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    let mut chunk_paths = Vec::new();
    let mut writer: Option<hound::WavWriter<BufWriter<File>>> = None;
    let mut samples_in_chunk = 0usize;
    let mut chunk_index = 0usize;

    for sample in reader.samples::<i16>() {
        if writer.is_none() {
            let chunk_path = chunk_dir.join(format!("{stem}-part-{chunk_index:04}.wav"));
            writer = Some(
                hound::WavWriter::create(&chunk_path, spec).map_err(|error| error.to_string())?,
            );
            chunk_paths.push(chunk_path);
            samples_in_chunk = 0;
        }

        if let Some(writer) = writer.as_mut() {
            writer
                .write_sample(sample.map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        }
        samples_in_chunk += 1;

        if samples_in_chunk >= samples_per_chunk {
            if let Some(writer) = writer.take() {
                writer.finalize().map_err(|error| error.to_string())?;
            }
            chunk_index += 1;
        }
    }

    if let Some(writer) = writer.take() {
        writer.finalize().map_err(|error| error.to_string())?;
    }

    Ok(chunk_paths)
}

fn split_wav_into_chunks(
    app: &AppHandle,
    wav_path: &Path,
) -> Result<(Vec<PathBuf>, PathBuf), String> {
    let chunk_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("chunks")
        .join(format!("audio-{}", unix_timestamp_millis()?));
    let chunks = split_wav_into_chunks_in_dir(wav_path, &chunk_dir, LONG_AUDIO_CHUNK_SECONDS)?;
    Ok((chunks, chunk_dir))
}

fn format_srt_timestamp(seconds: f64) -> String {
    let millis = (seconds.max(0.0) * 1000.0).round() as u64;
    let hours = millis / 3_600_000;
    let minutes = (millis % 3_600_000) / 60_000;
    let seconds = (millis % 60_000) / 1000;
    let milliseconds = millis % 1000;
    format!("{hours:02}:{minutes:02}:{seconds:02},{milliseconds:03}")
}

fn format_timeline_timestamp(seconds: f64) -> String {
    let total_frames = (seconds.max(0.0) * DAVINCI_TIMECODE_FPS as f64).floor() as u64;
    let frames = total_frames % DAVINCI_TIMECODE_FPS;
    let total_seconds = total_frames / DAVINCI_TIMECODE_FPS;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}:{frames:02}")
}

#[derive(Debug, Clone)]
struct TranscriptSegment {
    start: f64,
    end: f64,
    text: String,
}

fn split_text_into_chunks(text: &str) -> Vec<String> {
    const MAX_CHARS: usize = 42;
    let mut chunks = Vec::new();
    let mut current = String::new();

    for character in text.chars() {
        current.push(character);
        let char_count = current.chars().count();
        let is_break = matches!(
            character,
            '。' | '！' | '？' | '；' | '.' | '!' | '?' | ';' | '\n'
        );
        if is_break || char_count >= MAX_CHARS {
            let chunk = current.trim();
            if !chunk.is_empty() {
                chunks.push(chunk.to_string());
            }
            current.clear();
        }
    }

    let chunk = current.trim();
    if !chunk.is_empty() {
        chunks.push(chunk.to_string());
    }

    if chunks.is_empty() && !text.trim().is_empty() {
        chunks.push(text.trim().to_string());
    }

    chunks
}

fn build_transcript_segments(text: &str, duration: f64) -> Vec<TranscriptSegment> {
    let chunks = split_text_into_chunks(text);
    if chunks.is_empty() {
        return Vec::new();
    }

    let duration = duration.max(chunks.len() as f64 * 1.2);
    let total_chars: usize = chunks
        .iter()
        .map(|chunk| {
            chunk
                .chars()
                .filter(|character| !character.is_whitespace())
                .count()
                .max(1)
        })
        .sum();
    let mut cursor = 0.0;
    let mut segments = Vec::with_capacity(chunks.len());

    for (index, chunk) in chunks.iter().enumerate() {
        let end = if index + 1 == chunks.len() {
            duration
        } else {
            let char_count = chunk
                .chars()
                .filter(|character| !character.is_whitespace())
                .count()
                .max(1);
            (cursor + duration * char_count as f64 / total_chars as f64).min(duration)
        };
        segments.push(TranscriptSegment {
            start: cursor,
            end: end.max(cursor + 0.5),
            text: chunk.to_string(),
        });
        cursor = end;
    }

    segments
}

fn timeline_text_from_segments(segments: &[TranscriptSegment]) -> String {
    segments
        .iter()
        .map(|segment| {
            format!(
                "[{} --> {}]\n{}",
                format_timeline_timestamp(segment.start),
                format_timeline_timestamp(segment.end),
                segment.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn srt_text_from_segments(segments: &[TranscriptSegment]) -> String {
    let mut output = String::new();
    for (index, segment) in segments.iter().enumerate() {
        output.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            index + 1,
            format_srt_timestamp(segment.start),
            format_srt_timestamp(segment.end),
            segment.text
        ));
    }
    output
}

fn emit_transcription_progress(
    app: &AppHandle,
    task_id: Option<&str>,
    started_at: Instant,
    stage: &str,
    progress: u8,
    message: String,
    completed_segments: usize,
    total_segments: usize,
) {
    let Some(task_id) = task_id else {
        return;
    };

    let _ = app.emit(
        "transcription-progress",
        TranscriptionProgressEvent {
            task_id: task_id.to_string(),
            stage: stage.to_string(),
            progress: progress.min(100),
            message,
            completed_segments,
            total_segments,
            elapsed_ms: started_at.elapsed().as_millis(),
        },
    );
}

fn transcribe_sherpa_wav_cli(
    executable: &Path,
    engine: &InstalledEngineConfig,
    wav_path: &Path,
    sherpa_threads: usize,
    runtime_mode: &str,
) -> Result<String, String> {
    let mut command = Command::new(executable);
    command.args(sherpa_args_for_runtime(
        &engine.args,
        Some(sherpa_threads),
        runtime_mode,
    )?);
    command.arg(wav_path);
    suppress_command_window(&mut command);

    let duration = wav_duration_seconds(wav_path).unwrap_or(60.0);
    let timeout = Duration::from_secs(((duration * 4.0) as u64 + 120).clamp(120, 14_400));
    let output = run_command_with_timeout(&mut command, timeout, "Sherpa CLI transcription")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    if !output.status.success() {
        return Err(format!("Sherpa 转录失败：{}", combined.trim()));
    }

    Ok(extract_transcription_text(&combined))
}

fn transcribe_sherpa_wav_once(
    app: &AppHandle,
    engine: &InstalledEngineConfig,
    executable: &Path,
    wav_path: &Path,
    performance: TranscriptionPerformance,
    allow_daemon: bool,
    runtime_mode: &str,
) -> Result<String, String> {
    if allow_daemon {
        match ensure_sherpa_daemon_running(app, engine, executable, runtime_mode)
            .and_then(|_| transcribe_sherpa_wav_websocket(wav_path))
        {
            Ok(text) if !text.trim().is_empty() => return Ok(text),
            Ok(_) | Err(_) => {}
        }
    }

    transcribe_sherpa_wav_cli(
        executable,
        engine,
        wav_path,
        performance.sherpa_threads,
        runtime_mode,
    )
}

fn transcribe_sherpa_wav(
    app: &AppHandle,
    engine: &InstalledEngineConfig,
    executable: &Path,
    wav_path: &Path,
    performance: TranscriptionPerformance,
    task_id: Option<&str>,
    started_at: Instant,
    runtime_mode: &str,
) -> Result<String, String> {
    let duration = wav_duration_seconds(wav_path)?;
    if duration <= LONG_AUDIO_THRESHOLD_SECONDS {
        emit_transcription_progress(
            app,
            task_id,
            started_at,
            "transcribing",
            35,
            "正在调用本地 Sherpa 模型".to_string(),
            0,
            1,
        );
        let text = transcribe_sherpa_wav_once(
            app,
            engine,
            executable,
            wav_path,
            performance,
            true,
            runtime_mode,
        )?;
        emit_transcription_progress(
            app,
            task_id,
            started_at,
            "transcribing",
            92,
            "转录完成，正在生成结果文件".to_string(),
            1,
            1,
        );
        return Ok(text);
    }

    let (chunks, chunk_dir) = split_wav_into_chunks(app, wav_path)?;
    let total_segments = chunks.len();
    emit_transcription_progress(
        app,
        task_id,
        started_at,
        "splitting",
        15,
        format!("长音频已切分为 {total_segments} 段"),
        0,
        total_segments,
    );

    let worker_count = performance.chunk_workers.max(1).min(total_segments.max(1));
    let queue = Arc::new(Mutex::new(
        chunks
            .iter()
            .cloned()
            .enumerate()
            .collect::<VecDeque<(usize, PathBuf)>>(),
    ));
    let (sender, receiver) = mpsc::channel::<(usize, Result<String, String>)>();

    let mut handles = Vec::new();
    for _ in 0..worker_count {
        let queue = Arc::clone(&queue);
        let sender = sender.clone();
        let engine = engine.clone();
        let executable = executable.to_path_buf();
        let runtime_mode = runtime_mode.to_string();
        handles.push(thread::spawn(move || loop {
            let next = {
                let Ok(mut queue) = queue.lock() else {
                    return;
                };
                queue.pop_front()
            };
            let Some((index, chunk)) = next else {
                return;
            };
            let result = transcribe_sherpa_wav_cli(
                &executable,
                &engine,
                &chunk,
                performance.sherpa_threads,
                &runtime_mode,
            );
            let _ = sender.send((index, result));
        }));
    }
    drop(sender);

    let mut results = vec![None; total_segments];
    let mut completed = 0usize;
    for (index, result) in receiver {
        completed += 1;
        results[index] = Some(result);
        let progress =
            15 + ((completed as f64 / total_segments.max(1) as f64) * 78.0).round() as u8;
        emit_transcription_progress(
            app,
            task_id,
            started_at,
            "transcribing",
            progress,
            format!("正在转录分段 {completed}/{total_segments}"),
            completed,
            total_segments,
        );
    }
    for handle in handles {
        let _ = handle.join();
    }
    let _ = fs::remove_dir_all(&chunk_dir);

    let mut texts = Vec::new();
    let mut errors = Vec::new();
    for (index, result) in results.into_iter().enumerate() {
        match result {
            Some(Ok(text)) if !text.trim().is_empty() => texts.push(text.trim().to_string()),
            Some(Ok(_)) => errors.push(format!("第 {} 段：没有识别到文字", index + 1)),
            Some(Err(error)) => errors.push(format!("第 {} 段：{error}", index + 1)),
            None => errors.push(format!("第 {} 段：没有返回结果", index + 1)),
        }
    }

    if !texts.is_empty() {
        return Ok(texts.join("\n"));
    }

    if !errors.is_empty() {
        return Err(format!("长音频分段转录失败：{}", errors.join("；")));
    }

    Err("Sherpa 已运行，但长音频分段后仍未解析到转录文字。".to_string())
}

fn write_smoke_test_wav(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|error| error.to_string())?;
    for _ in 0..8_000 {
        writer
            .write_sample(0i16)
            .map_err(|error| error.to_string())?;
    }
    writer.finalize().map_err(|error| error.to_string())
}

fn run_acceleration_smoke_test_inner(
    app: AppHandle,
    request: AccelerationSmokeTestRequest,
) -> Result<AccelerationSmokeTestResult, String> {
    let started_at = Instant::now();
    let requested_mode = if request
        .acceleration_mode
        .trim()
        .eq_ignore_ascii_case("cuda")
    {
        "cuda"
    } else {
        "cpu"
    };
    let model_dir = PathBuf::from(&request.model_dir);
    let engine = read_sherpa_engine(&model_dir)?;
    let cpu_executable = PathBuf::from(&engine.executable);
    if !cpu_executable.exists() {
        return Err("Sherpa-ONNX 程序不存在，请重新配置模型。".to_string());
    }

    let smoke_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("gpu-smoke-tests");
    let smoke_wav = smoke_dir.join(format!("smoke-{}.wav", unix_timestamp_millis()?));
    write_smoke_test_wav(&smoke_wav)?;

    let runtime = resolve_sherpa_runtime(&app, &engine, requested_mode);
    let fallback_reason = runtime.fallback_reason.clone();
    let performance = transcription_performance("stable");
    let first_result = transcribe_sherpa_wav_once(
        &app,
        &engine,
        &runtime.executable,
        &smoke_wav,
        performance,
        false,
        &runtime.mode,
    );

    let (used_mode, fallback_used, text, message) = match first_result {
        Ok(text) => {
            let message = if runtime.mode == "cuda" {
                "CUDA smoke test 已完成；静音音频不要求识别出文字。".to_string()
            } else if let Some(reason) = fallback_reason {
                format!("CUDA 不可用，已使用 CPU 完成 smoke test：{reason}")
            } else {
                "CPU smoke test 已完成；静音音频不要求识别出文字。".to_string()
            };
            let fallback_used = requested_mode == "cuda" && runtime.mode != "cuda";
            (runtime.mode, fallback_used, text, message)
        }
        Err(error) if runtime.mode != "cpu" => {
            trip_cuda_circuit(&app, format!("CUDA smoke test 失败：{error}"));
            let cpu_text = transcribe_sherpa_wav_once(
                &app,
                &engine,
                &cpu_executable,
                &smoke_wav,
                performance,
                false,
                "cpu",
            )?;
            (
                "cpu".to_string(),
                true,
                cpu_text,
                format!("CUDA smoke test 失败，CPU 回退成功：{error}"),
            )
        }
        Err(error) => {
            let _ = fs::remove_file(&smoke_wav);
            return Err(error);
        }
    };

    let _ = fs::remove_file(&smoke_wav);
    Ok(AccelerationSmokeTestResult {
        requested_mode: requested_mode.to_string(),
        used_mode,
        fallback_used,
        elapsed_ms: started_at.elapsed().as_millis(),
        transcript_preview: text.trim().chars().take(80).collect(),
        message,
    })
}

fn write_transcription_outputs(
    output_dir: &Path,
    preferred_format: &str,
    source_audio_path: &Path,
    sherpa_audio_path: &Path,
    text: &str,
) -> Result<(String, Vec<String>, Vec<TranscriptionOutputFile>), String> {
    fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;

    let stem = source_audio_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("transcript");
    let suffix = unix_timestamp_millis()?;
    let plain_path = output_dir.join(format!("{stem}-{suffix}.txt"));
    let timeline_path = output_dir.join(format!("{stem}-{suffix}-timeline.txt"));
    let srt_path = output_dir.join(format!("{stem}-{suffix}.srt"));
    let duration = wav_duration_seconds(sherpa_audio_path)?.max(1.0);
    let segments = build_transcript_segments(text, duration);

    fs::write(&plain_path, text).map_err(|error| error.to_string())?;
    fs::write(&timeline_path, timeline_text_from_segments(&segments))
        .map_err(|error| error.to_string())?;
    fs::write(&srt_path, srt_text_from_segments(&segments)).map_err(|error| error.to_string())?;

    let files = vec![
        TranscriptionOutputFile {
            format: "plainText".to_string(),
            label: "无时间码纯文字".to_string(),
            path: plain_path.to_string_lossy().to_string(),
        },
        TranscriptionOutputFile {
            format: "timelineText".to_string(),
            label: "带时间码 TXT".to_string(),
            path: timeline_path.to_string_lossy().to_string(),
        },
        TranscriptionOutputFile {
            format: "srt".to_string(),
            label: "SRT 字幕".to_string(),
            path: srt_path.to_string_lossy().to_string(),
        },
    ];
    let output_paths = files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let primary = if preferred_format == "timelineText" {
        timeline_path
    } else if preferred_format == "srt" {
        srt_path
    } else {
        plain_path
    };

    Ok((primary.to_string_lossy().to_string(), output_paths, files))
}

fn transcribe_file_with_sherpa(
    app: AppHandle,
    request: TranscribeFileRequest,
) -> Result<TranscribeFileResult, String> {
    let started_at = Instant::now();
    let performance = transcription_performance(&request.performance_mode);
    let _ = performance.file_workers;
    emit_transcription_progress(
        &app,
        request.task_id.as_deref(),
        started_at,
        "transcoding",
        5,
        "正在转为 16kHz 单声道 WAV".to_string(),
        0,
        0,
    );
    let audio_path = PathBuf::from(&request.audio_path);
    if !audio_path.exists() {
        return Err("音频文件不存在。".to_string());
    }
    let sherpa_audio_path = media_to_sherpa_wav(&app, &audio_path)?;
    emit_transcription_progress(
        &app,
        request.task_id.as_deref(),
        started_at,
        "transcoding",
        12,
        "音频转码完成，正在准备识别".to_string(),
        0,
        0,
    );

    let model_dir = PathBuf::from(&request.model_dir);
    let engine = read_sherpa_engine(&model_dir)?;

    let cpu_executable = PathBuf::from(&engine.executable);
    if !cpu_executable.exists() {
        return Err("Sherpa-ONNX 程序不存在，请重新配置模型。".to_string());
    }

    if request
        .acceleration_mode
        .trim()
        .eq_ignore_ascii_case("cuda")
    {
        emit_transcription_progress(
            &app,
            request.task_id.as_deref(),
            started_at,
            "transcribing",
            14,
            "正在准备 CUDA 加速运行时；不可用时会自动回退到 CPU。".to_string(),
            0,
            0,
        );
    }

    let runtime = resolve_sherpa_runtime(&app, &engine, &request.acceleration_mode);
    let runtime_performance = performance_for_acceleration(performance, &runtime.mode);
    if let Some(reason) = runtime.fallback_reason.as_ref() {
        emit_transcription_progress(
            &app,
            request.task_id.as_deref(),
            started_at,
            "transcribing",
            18,
            reason.clone(),
            0,
            0,
        );
    }

    let text_result = transcribe_sherpa_wav(
        &app,
        &engine,
        &runtime.executable,
        &sherpa_audio_path,
        runtime_performance,
        request.task_id.as_deref(),
        started_at,
        &runtime.mode,
    );
    let text = match text_result {
        Ok(text) => text,
        Err(error) if runtime.mode != "cpu" => {
            trip_cuda_circuit(&app, format!("CUDA 识别失败：{error}"));
            emit_transcription_progress(
                &app,
                request.task_id.as_deref(),
                started_at,
                "transcribing",
                20,
                format!("CUDA 识别失败，正在回退到 CPU：{error}"),
                0,
                0,
            );
            transcribe_sherpa_wav(
                &app,
                &engine,
                &cpu_executable,
                &sherpa_audio_path,
                performance,
                request.task_id.as_deref(),
                started_at,
                "cpu",
            )?
        }
        Err(error) => return Err(error),
    };

    if text.is_empty() {
        return Err("Sherpa 已运行，但没有解析到转录文字。".to_string());
    }

    let (output_path, output_paths, output_files) = if request.save_output {
        emit_transcription_progress(
            &app,
            request.task_id.as_deref(),
            started_at,
            "exporting",
            95,
            "正在生成三种导出文件".to_string(),
            0,
            0,
        );
        let output_dir = app
            .path()
            .app_cache_dir()
            .map_err(|error| error.to_string())?
            .join("transcripts");
        write_transcription_outputs(
            &output_dir,
            &request.output_format,
            &audio_path,
            &sherpa_audio_path,
            &text,
        )?
    } else {
        (String::new(), Vec::new(), Vec::new())
    };
    if sherpa_audio_path != audio_path {
        let _ = fs::remove_file(&sherpa_audio_path);
    }
    emit_transcription_progress(
        &app,
        request.task_id.as_deref(),
        started_at,
        "done",
        100,
        "转录完成".to_string(),
        0,
        0,
    );

    Ok(TranscribeFileResult {
        text,
        output_path,
        output_paths,
        output_files,
    })
}
fn write_i16_samples(
    writer: &Arc<Mutex<Option<hound::WavWriter<BufWriter<File>>>>>,
    level_emitter: &Arc<AudioLevelEmitter>,
    data: &[i16],
) {
    let mut sum = 0.0f64;
    if let Ok(mut guard) = writer.lock() {
        if let Some(writer) = guard.as_mut() {
            for sample in data {
                let normalized = *sample as f64 / i16::MAX as f64;
                sum += normalized * normalized;
                let _ = writer.write_sample(*sample);
            }
        }
    }
    if !data.is_empty() {
        emit_recording_level(
            level_emitter,
            ((sum / data.len() as f64).sqrt() as f32) * 4.0,
        );
    }
}

fn write_f32_samples(
    writer: &Arc<Mutex<Option<hound::WavWriter<BufWriter<File>>>>>,
    level_emitter: &Arc<AudioLevelEmitter>,
    data: &[f32],
) {
    let mut sum = 0.0f64;
    if let Ok(mut guard) = writer.lock() {
        if let Some(writer) = guard.as_mut() {
            for sample in data {
                let sample = sample.clamp(-1.0, 1.0);
                sum += f64::from(sample) * f64::from(sample);
                let _ = writer.write_sample((sample * i16::MAX as f32) as i16);
            }
        }
    }
    if !data.is_empty() {
        emit_recording_level(
            level_emitter,
            ((sum / data.len() as f64).sqrt() as f32) * 4.0,
        );
    }
}

fn write_u16_samples(
    writer: &Arc<Mutex<Option<hound::WavWriter<BufWriter<File>>>>>,
    level_emitter: &Arc<AudioLevelEmitter>,
    data: &[u16],
) {
    let mut sum = 0.0f64;
    if let Ok(mut guard) = writer.lock() {
        if let Some(writer) = guard.as_mut() {
            for sample in data {
                let centered = *sample as i32 - i16::MAX as i32 - 1;
                let sample = centered.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                let normalized = sample as f64 / i16::MAX as f64;
                sum += normalized * normalized;
                let _ = writer.write_sample(sample);
            }
        }
    }
    if !data.is_empty() {
        emit_recording_level(
            level_emitter,
            ((sum / data.len() as f64).sqrt() as f32) * 4.0,
        );
    }
}

fn app_recordings_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?;
    let recordings_dir = data_dir.join("recordings");
    fs::create_dir_all(&recordings_dir).map_err(|error| error.to_string())?;
    Ok(recordings_dir)
}

fn start_microphone_recording(app: &AppHandle) -> Result<RecordingSession, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "没有找到可用麦克风。".to_string())?;
    let supported_config = device
        .default_input_config()
        .map_err(|error| error.to_string())?;
    let sample_format = supported_config.sample_format();
    let stream_config: cpal::StreamConfig = supported_config.into();

    let recordings_dir = app_recordings_dir(app)?;
    let path = recordings_dir.join(format!("voice-{}.wav", unix_timestamp_millis()?));

    let spec = hound::WavSpec {
        channels: stream_config.channels,
        sample_rate: stream_config.sample_rate.0,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let writer = hound::WavWriter::create(&path, spec).map_err(|error| error.to_string())?;
    let writer = Arc::new(Mutex::new(Some(writer)));
    let level_emitter = Arc::new(AudioLevelEmitter {
        app: app.clone(),
        last_emit: Mutex::new(Instant::now() - Duration::from_millis(100)),
    });
    let err_fn = |error| eprintln!("audio input stream error: {error}");

    let stream = match sample_format {
        cpal::SampleFormat::I16 => {
            let writer = Arc::clone(&writer);
            let level_emitter = Arc::clone(&level_emitter);
            device.build_input_stream(
                &stream_config,
                move |data: &[i16], _| write_i16_samples(&writer, &level_emitter, data),
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::F32 => {
            let writer = Arc::clone(&writer);
            let level_emitter = Arc::clone(&level_emitter);
            device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| write_f32_samples(&writer, &level_emitter, data),
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let writer = Arc::clone(&writer);
            let level_emitter = Arc::clone(&level_emitter);
            device.build_input_stream(
                &stream_config,
                move |data: &[u16], _| write_u16_samples(&writer, &level_emitter, data),
                err_fn,
                None,
            )
        }
        other => return Err(format!("涓嶆敮鎸佺殑楹﹀厠椋庨噰鏍锋牸寮忥細{other:?}")),
    }
    .map_err(|error| error.to_string())?;

    stream.play().map_err(|error| error.to_string())?;
    Ok(RecordingSession {
        stream,
        writer,
        path,
    })
}

fn stop_microphone_recording(session: RecordingSession) -> Result<PathBuf, String> {
    let RecordingSession {
        stream,
        writer,
        path,
    } = session;
    drop(stream);

    let mut guard = writer.lock().map_err(|error| error.to_string())?;
    if let Some(writer) = guard.take() {
        writer.finalize().map_err(|error| error.to_string())?;
    }

    Ok(path)
}

fn start_recording_from_runtime(app: &AppHandle, state: &RuntimeState) -> Result<String, String> {
    let mut recording = state.recording.lock().map_err(|error| error.to_string())?;
    if recording.is_some() {
        return Err("录音已经在进行中。".to_string());
    }

    let session = start_microphone_recording(app)?;
    let path = session.path.to_string_lossy().to_string();
    *recording = Some(session);
    emit_recording_state(app, true, Some(path.clone()));
    Ok(path)
}

fn emit_recording_state(app: &AppHandle, is_recording: bool, path: Option<String>) {
    apply_wave_window_visibility(app, is_recording);
    let _ = app.emit(
        "recording-state",
        RecordingStateEvent { is_recording, path },
    );
}

fn emit_recording_level(emitter: &Arc<AudioLevelEmitter>, level: f32) {
    let Ok(mut last_emit) = emitter.last_emit.lock() else {
        return;
    };
    if last_emit.elapsed() < Duration::from_millis(70) {
        return;
    }
    *last_emit = Instant::now();
    let _ = emitter.app.emit(
        "recording-level",
        RecordingLevelEvent {
            level: level.clamp(0.0, 1.0),
        },
    );
}

fn take_recording_and_settings(
    state: &RuntimeState,
) -> Result<(RecordingSession, UserSettings), String> {
    let session = {
        let mut recording = state.recording.lock().map_err(|error| error.to_string())?;
        recording
            .take()
            .ok_or_else(|| "当前没有正在进行的录音。".to_string())?
    };
    let settings = {
        let settings = state.settings.lock().map_err(|error| error.to_string())?;
        settings.clone()
    };

    Ok((session, settings))
}

fn finish_recording_with_settings(
    app: AppHandle,
    session: RecordingSession,
    settings: UserSettings,
    paste: bool,
) -> Result<TranscribeFileResult, String> {
    let audio_path = stop_microphone_recording(session)?;
    if settings.recording_mode == "audioOnly" {
        let audio_path_text = audio_path.to_string_lossy().to_string();
        return Ok(TranscribeFileResult {
            text: format!("录音已保存：{audio_path_text}"),
            output_path: audio_path_text.clone(),
            output_paths: vec![audio_path_text],
            output_files: Vec::new(),
        });
    }

    if settings.model_dir.trim().is_empty() {
        return Err("请先在设置里下载并配置离线模型。".to_string());
    }

    let result = transcribe_file_with_sherpa(
        app,
        TranscribeFileRequest {
            audio_path: audio_path.to_string_lossy().to_string(),
            model_dir: settings.model_dir,
            task_id: None,
            performance_mode: "stable".to_string(),
            acceleration_mode: settings.acceleration_mode,
            output_format: settings.export_format,
            save_output: settings.save_recordings,
        },
    )?;

    if paste {
        paste_text_to_active_window(&result.text)?;
    }

    if !settings.save_recordings {
        let _ = fs::remove_file(audio_path);
    }

    Ok(result)
}

fn finish_recording_async(app: AppHandle, session: RecordingSession, settings: UserSettings) {
    tauri::async_runtime::spawn(async move {
        let app_for_work = app.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            finish_recording_with_settings(app_for_work, session, settings, true)
        })
        .await
        {
            Ok(Ok(result)) => {
                let _ = app.emit("transcription-result", result);
            }
            Ok(Err(error)) => eprintln!("global shortcut stop failed: {error}"),
            Err(error) => eprintln!("global shortcut stop task failed: {error}"),
        }
    });
}

fn handle_global_shortcut_event(app: &AppHandle, event: ShortcutEvent) {
    let state = app.state::<RuntimeState>();
    let recording_mode = state
        .settings
        .lock()
        .map(|settings| settings.recording_mode.clone())
        .unwrap_or_else(|_| "hold".to_string());

    match (recording_mode.as_str(), event.state) {
        ("hold", ShortcutState::Pressed) => {
            if let Err(error) = start_recording_from_runtime(app, state.inner()) {
                eprintln!("global shortcut start failed: {error}");
            }
        }
        ("hold", ShortcutState::Released) => {
            let Ok((session, settings)) = take_recording_and_settings(state.inner()) else {
                return;
            };
            emit_recording_state(app, false, None);
            finish_recording_async(app.clone(), session, settings);
        }
        (_, ShortcutState::Pressed) => match take_recording_and_settings(state.inner()) {
            Ok((session, settings)) => {
                emit_recording_state(app, false, None);
                finish_recording_async(app.clone(), session, settings);
            }
            Err(_) => {
                if let Err(error) = start_recording_from_runtime(app, state.inner()) {
                    eprintln!("global shortcut toggle start failed: {error}");
                }
            }
        },
        (_, ShortcutState::Released) => {}
    }
}

fn register_global_recording_shortcut(app: &AppHandle, shortcut: &str) -> Result<(), String> {
    let shortcut = normalize_shortcut(shortcut);
    let global_shortcut = app.global_shortcut();
    global_shortcut
        .unregister_all()
        .map_err(|error| error.to_string())?;
    global_shortcut
        .on_shortcut(shortcut.as_str(), |app, _shortcut, event| {
            handle_global_shortcut_event(app, event);
        })
        .map_err(|error| error.to_string())
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "show", "打开 Hi-Voicer", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;
    let Some(icon) = app.default_window_icon() else {
        return Ok(());
    };

    TrayIconBuilder::new()
        .tooltip("Hi-Voicer")
        .icon(icon.clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "quit" => {
                if let Some(state) = app.try_state::<RuntimeState>() {
                    if let Ok(mut daemon) = state.sherpa_daemon.lock() {
                        *daemon = None;
                    }
                }
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(&tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
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
fn load_settings(app: AppHandle, state: State<'_, RuntimeState>) -> Result<UserSettings, String> {
    let settings = read_settings(&app)?;
    register_global_recording_shortcut(&app, &settings.shortcut)?;
    let mut stored = state.settings.lock().expect("settings mutex poisoned");
    *stored = settings.clone();
    let app_handle = app.clone();
    let model_dir = settings.model_dir.clone();
    thread::spawn(move || warm_sherpa_daemon(app_handle, model_dir));
    Ok(settings)
}

#[tauri::command]
fn save_settings(
    settings: UserSettings,
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<UserSettings, String> {
    apply_launch_at_startup(settings.launch_at_startup)?;
    apply_mini_window_visibility(&app, settings.show_mini_window);
    write_settings(&app, &settings)?;
    register_global_recording_shortcut(&app, &settings.shortcut)?;
    let mut stored = state.settings.lock().expect("settings mutex poisoned");
    *stored = settings.clone();
    let app_handle = app.clone();
    let model_dir = settings.model_dir.clone();
    thread::spawn(move || warm_sherpa_daemon(app_handle, model_dir));
    Ok(settings)
}

#[tauri::command]
async fn install_model(app: AppHandle, model: ModelInstallRequest) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if model.install_kind == "sherpaOnnx" {
            install_sherpa_model(app, model)
        } else {
            install_zip_model(app, model)
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn select_directory() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        Ok(rfd::FileDialog::new()
            .pick_folder()
            .map(|path| path.to_string_lossy().to_string()))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn select_audio_files() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        Ok(rfd::FileDialog::new()
            .add_filter(
                "Audio and Video",
                &[
                    "wav", "mp3", "m4a", "aac", "flac", "ogg", "mp4", "mkv", "mov", "webm",
                ],
            )
            .pick_files()
            .unwrap_or_default()
            .into_iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn open_recordings_dir(app: AppHandle) -> Result<String, String> {
    let recordings_dir = app_recordings_dir(&app)?;
    let dir_text = recordings_dir.to_string_lossy().to_string();

    #[cfg(windows)]
    {
        Command::new("explorer")
            .arg(&recordings_dir)
            .spawn()
            .map_err(|error| error.to_string())?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&recordings_dir)
            .spawn()
            .map_err(|error| error.to_string())?;
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(&recordings_dir)
            .spawn()
            .map_err(|error| error.to_string())?;
    }

    Ok(dir_text)
}

#[tauri::command]
async fn save_text_file(request: SaveTextFileRequest) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&request.suggested_name)
            .save_file()
        else {
            return Ok(None);
        };

        fs::write(&path, request.contents).map_err(|error| error.to_string())?;
        Ok(Some(path.to_string_lossy().to_string()))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn save_existing_file(request: SaveExistingFileRequest) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source_path = PathBuf::from(&request.source_path);
        if !source_path.exists() {
            return Err("要导出的文件不存在。".to_string());
        }

        if let Some(destination_dir) = request.destination_dir.as_deref() {
            if !destination_dir.trim().is_empty() {
                let destination_dir = PathBuf::from(destination_dir);
                fs::create_dir_all(&destination_dir).map_err(|error| error.to_string())?;
                let destination_path = destination_dir.join(&request.suggested_name);
                fs::copy(&source_path, &destination_path).map_err(|error| error.to_string())?;
                return Ok(Some(destination_path.to_string_lossy().to_string()));
            }
        }

        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&request.suggested_name)
            .save_file()
        else {
            return Ok(None);
        };

        fs::copy(&source_path, &path).map_err(|error| error.to_string())?;
        Ok(Some(path.to_string_lossy().to_string()))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn transcribe_file(
    app: AppHandle,
    request: TranscribeFileRequest,
) -> Result<TranscribeFileResult, String> {
    tauri::async_runtime::spawn_blocking(move || transcribe_file_with_sherpa(app, request))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn validate_model_dir(request: ModelValidationRequest) -> ModelValidationResult {
    validate_model_dir_path(&request.model_dir)
}

#[tauri::command]
fn get_acceleration_status(
    app: AppHandle,
    request: AccelerationStatusRequest,
) -> Result<AccelerationStatus, String> {
    acceleration_status_for_app(&app, &request.acceleration_mode)
}

#[tauri::command]
async fn prepare_acceleration_runtime(
    app: AppHandle,
    request: AccelerationStatusRequest,
) -> Result<AccelerationStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_acceleration_runtime_for_app(&app, &request.acceleration_mode)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn run_acceleration_smoke_test(
    app: AppHandle,
    request: AccelerationSmokeTestRequest,
) -> Result<AccelerationSmokeTestResult, String> {
    tauri::async_runtime::spawn_blocking(move || run_acceleration_smoke_test_inner(app, request))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn start_recording(app: AppHandle, state: State<'_, RuntimeState>) -> Result<String, String> {
    start_recording_from_runtime(&app, state.inner())
}

#[tauri::command]
async fn stop_recording(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<TranscribeFileResult, String> {
    let (session, settings) = take_recording_and_settings(state.inner())?;
    emit_recording_state(&app, false, None);

    tauri::async_runtime::spawn_blocking(move || {
        finish_recording_with_settings(app, session, settings, true)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .manage(RuntimeState {
            settings: Mutex::new(UserSettings::default()),
            recording: Mutex::new(None),
            sherpa_daemon: Mutex::new(None),
            sherpa_runtime_install: Mutex::new(()),
            cuda_disabled_reason: Mutex::new(None),
            cuda_startup_checked_runtime: Mutex::new(None),
        })
        .setup(|app| {
            let settings = read_settings(&app.handle()).unwrap_or_else(|_| UserSettings::default());
            if let Some(state) = app.try_state::<RuntimeState>() {
                if let Ok(mut stored) = state.settings.lock() {
                    *stored = settings.clone();
                }
            }
            apply_launch_at_startup(settings.launch_at_startup)?;
            apply_mini_window_visibility(&app.handle(), settings.show_mini_window);
            apply_wave_window_visibility(&app.handle(), false);
            setup_tray(app)?;
            register_global_recording_shortcut(&app.handle(), &settings.shortcut)?;
            let app_handle = app.handle().clone();
            let model_dir = settings.model_dir.clone();
            thread::spawn(move || warm_sherpa_daemon(app_handle, model_dir));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_app_snapshot,
            load_settings,
            save_settings,
            install_model,
            select_directory,
            select_audio_files,
            open_recordings_dir,
            save_text_file,
            save_existing_file,
            transcribe_file,
            validate_model_dir,
            get_acceleration_status,
            prepare_acceleration_runtime,
            run_acceleration_smoke_test,
            start_recording,
            stop_recording
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::current_dir()
            .expect("current dir")
            .join("target")
            .join("hi-voicer-tests")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test root");
        root
    }

    #[cfg(windows)]
    fn test_exit_status(code: u32) -> std::process::ExitStatus {
        use std::os::windows::process::ExitStatusExt;

        std::process::ExitStatus::from_raw(code)
    }

    #[cfg(unix)]
    fn test_exit_status(code: u32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;

        std::process::ExitStatus::from_raw((code as i32) << 8)
    }

    fn write_installed_model(models_dir: &Path, model_id: &str, executable: &Path) -> PathBuf {
        let model_dir = models_dir.join(model_id);
        fs::create_dir_all(&model_dir).expect("create model dir");
        fs::write(model_dir.join("model.int8.onnx"), b"model").expect("write model file");

        let config = InstalledEngineConfig {
            engine: "sherpa-onnx".to_string(),
            model_id: model_id.to_string(),
            model_name: model_id.to_string(),
            model_dir: model_dir.to_string_lossy().to_string(),
            executable: executable.to_string_lossy().to_string(),
            args: "--dummy".to_string(),
            required_files: vec!["model.int8.onnx".to_string()],
        };
        fs::write(
            model_dir.join("engine.json"),
            serde_json::to_string(&config).expect("serialize engine config"),
        )
        .expect("write engine config");

        model_dir
    }

    #[test]
    fn discovers_preferred_installed_model() {
        let root = test_root("discovers-preferred-installed-model");
        let executable = root.join("sherpa-onnx-offline.exe");
        fs::write(&executable, b"exe").expect("write executable");
        let models_dir = root.join("models");
        let model_dir = write_installed_model(&models_dir, "sensevoice-small", &executable);

        let discovered =
            discover_installed_model_in_models_dir(&models_dir, "sensevoice-small").expect("model");

        assert_eq!(discovered.0, "sensevoice-small");
        assert_eq!(discovered.1, model_dir.to_string_lossy());
    }

    #[test]
    fn skips_incomplete_preferred_model_and_falls_back() {
        let root = test_root("skips-incomplete-preferred-model-and-falls-back");
        let executable = root.join("sherpa-onnx-offline.exe");
        fs::write(&executable, b"exe").expect("write executable");
        let models_dir = root.join("models");
        fs::create_dir_all(models_dir.join("sensevoice-small")).expect("create incomplete model");
        write_installed_model(&models_dir, "sherpa-paraformer-zh", &executable);

        let discovered =
            discover_installed_model_in_models_dir(&models_dir, "sensevoice-small").expect("model");

        assert_eq!(discovered.0, "sherpa-paraformer-zh");
    }

    #[test]
    fn older_user_settings_default_to_cpu_acceleration() {
        let settings: UserSettings =
            serde_json::from_str(r#"{"shortcut":"Mouse4"}"#).expect("settings");

        assert_eq!(settings.acceleration_mode, "cpu");
    }

    #[test]
    fn transcribe_request_defaults_to_cpu_acceleration() {
        let request: TranscribeFileRequest =
            serde_json::from_str(r#"{"audioPath":"sample.wav","modelDir":"models/demo"}"#)
                .expect("request");

        assert_eq!(request.acceleration_mode, "cpu");
    }

    #[test]
    fn cuda_acceleration_uses_single_worker_performance() {
        let fast = transcription_performance("fast");
        let cuda = performance_for_acceleration(fast, "cuda");
        let cpu = performance_for_acceleration(fast, "cpu");

        assert_eq!(cuda.file_workers, 1);
        assert_eq!(cuda.chunk_workers, 1);
        assert_eq!(cuda.sherpa_threads, 2);
        assert_eq!(cpu.chunk_workers, 3);
    }

    #[test]
    fn acceleration_status_keeps_cpu_effective_when_cuda_is_unavailable() {
        let status =
            acceleration_status_from_parts("cuda", false, None, None, true, false, None, None);

        assert_eq!(status.selected_mode, "cuda");
        assert_eq!(status.effective_mode, "cpu");
        assert!(status.message.contains("回退 CPU"));
    }

    #[test]
    fn acceleration_status_reports_cuda_ready_when_runtime_is_installed() {
        let status = acceleration_status_from_parts(
            "cuda",
            true,
            Some("RTX 4090 / driver 555.85 / VRAM 24564 MB".to_string()),
            None,
            true,
            true,
            None,
            None,
        );

        assert_eq!(status.effective_mode, "cuda");
        assert!(status.cuda_runtime_installed);
        assert!(status.cuda_device_summary.is_some());
        assert!(status.message.contains("CUDA"));
    }

    #[test]
    fn acceleration_status_reports_prepare_failure_as_cpu_fallback() {
        let status = acceleration_status_from_parts(
            "cuda",
            true,
            None,
            None,
            true,
            false,
            None,
            Some("network error".to_string()),
        );

        assert_eq!(status.effective_mode, "cpu");
        assert!(status.message.contains("network error"));
    }

    #[test]
    fn acceleration_status_reports_cuda_circuit_breaker_as_cpu_fallback() {
        let status = acceleration_status_from_parts(
            "cuda",
            true,
            None,
            None,
            true,
            true,
            Some("driver mismatch".to_string()),
            None,
        );

        assert_eq!(status.effective_mode, "cpu");
        assert_eq!(
            status.cuda_disabled_reason.as_deref(),
            Some("driver mismatch")
        );
        assert!(status.message.contains("本次会话已停用"));
    }

    #[test]
    fn parses_nvidia_smi_query_output_for_diagnostics() {
        let summary = parse_nvidia_smi_query_output(
            "NVIDIA GeForce RTX 4070, 552.44, 12282\nNVIDIA RTX A2000, 551.86, 6144\n",
        )
        .expect("summary");

        assert!(summary.contains("RTX 4070"));
        assert!(summary.contains("driver 552.44"));
        assert!(summary.contains("VRAM 12282 MB"));
        assert!(summary.contains("RTX A2000"));
    }

    #[test]
    fn builds_nvidia_smi_candidate_paths_for_common_windows_installs() {
        let paths = nvidia_smi_candidate_paths(
            Some(r"C:\Windows"),
            Some(r"C:\Windows"),
            Some(r"C:\Program Files"),
        );

        assert_eq!(paths[0], PathBuf::from("nvidia-smi"));
        assert!(paths.contains(&PathBuf::from(r"C:\Windows\System32\nvidia-smi.exe")));
        assert!(paths.contains(&PathBuf::from(
            r"C:\Program Files\NVIDIA Corporation\NVSMI\nvidia-smi.exe"
        )));
        assert_eq!(
            paths
                .iter()
                .filter(|path| path.to_string_lossy().contains("System32"))
                .count(),
            1
        );
    }

    #[test]
    fn nvidia_cuda_info_requires_parseable_successful_output() {
        let candidate = PathBuf::from("nvidia-smi");
        let info = nvidia_cuda_info_from_output(
            &candidate,
            Output {
                status: test_exit_status(0),
                stdout: b"NVIDIA GeForce RTX 4070, 552.44, 12282\n".to_vec(),
                stderr: Vec::new(),
            },
        )
        .expect("cuda info");

        assert!(info.available);
        assert!(info
            .device_summary
            .as_deref()
            .unwrap_or_default()
            .contains("RTX 4070"));

        let error = nvidia_cuda_info_from_output(
            &candidate,
            Output {
                status: test_exit_status(1),
                stdout: Vec::new(),
                stderr: b"driver unavailable".to_vec(),
            },
        )
        .expect_err("non-zero output");
        assert!(error.contains("driver unavailable"));

        let error = nvidia_cuda_info_from_output(
            &candidate,
            Output {
                status: test_exit_status(0),
                stdout: b"not csv\n".to_vec(),
                stderr: Vec::new(),
            },
        )
        .expect_err("unparseable output");
        assert!(error.contains("no parseable GPU rows"));
    }

    #[test]
    fn derives_runtime_dir_from_executable_path() {
        let executable = PathBuf::from(
            r"C:\HiVoicer\engines\sherpa\v1.13.2\sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda\bin\sherpa-onnx-offline.exe",
        );

        let runtime_dir = sherpa_runtime_dir_from_executable(&executable).expect("runtime dir");

        assert_eq!(
            runtime_dir,
            PathBuf::from(
                r"C:\HiVoicer\engines\sherpa\v1.13.2\sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda"
            )
        );
    }

    #[test]
    fn websocket_server_path_is_derived_from_selected_runtime() {
        let cpu_executable = PathBuf::from(
            r"C:\HiVoicer\runtimes\sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts\bin\sherpa-onnx-offline.exe",
        );
        let cuda_executable = PathBuf::from(
            r"C:\HiVoicer\runtimes\sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda\bin\sherpa-onnx-offline.exe",
        );

        let cpu_server = sherpa_websocket_server_path(&cpu_executable);
        let cuda_server = sherpa_websocket_server_path(&cuda_executable);

        assert_ne!(cpu_server, cuda_server);
        assert!(cpu_server.to_string_lossy().contains("static-MT"));
        assert!(cuda_server.to_string_lossy().contains("cuda-12.x"));
    }

    #[test]
    fn cuda_startup_check_cache_is_bound_to_exact_runtime() {
        let cuda_executable = PathBuf::from(
            r"C:\HiVoicer\runtimes\sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda\bin\sherpa-onnx-offline.exe",
        );
        let cpu_executable = PathBuf::from(
            r"C:\HiVoicer\runtimes\sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts\bin\sherpa-onnx-offline.exe",
        );
        let checked_runtime = cuda_executable.to_string_lossy().to_string();

        assert!(cuda_runtime_check_matches(
            Some(&checked_runtime),
            &cuda_executable
        ));
        assert!(!cuda_runtime_check_matches(
            Some(&checked_runtime),
            &cpu_executable
        ));
        assert!(!cuda_runtime_check_matches(None, &cuda_executable));
    }

    #[test]
    fn smoke_test_runtime_reports_missing_executable() {
        let root = test_root("smoke-test-runtime-reports-missing-executable");
        let error = smoke_test_sherpa_runtime(&root.join("missing.exe")).expect_err("missing exe");

        assert!(error.contains("runtime executable not found"));
    }

    #[test]
    fn command_timeout_kills_hung_processes() {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-Command",
            "Start-Sleep -Seconds 5; Write-Output done",
        ]);

        let error =
            run_command_with_timeout(&mut command, Duration::from_millis(200), "timeout-test")
                .expect_err("timeout");

        assert!(error.contains("timeout-test timed out"));
    }

    #[test]
    fn command_timeout_captures_large_stdout_without_blocking() {
        let mut command = Command::new("powershell");
        command.args(["-NoProfile", "-Command", "'x' * 200000"]);

        let output =
            run_command_with_timeout(&mut command, Duration::from_secs(10), "large-output")
                .expect("large output");

        assert!(output.status.success());
        assert!(output.stdout.len() > 100_000);
    }

    #[test]
    fn writes_smoke_test_wav_as_16khz_mono_pcm() {
        let root = test_root("writes-smoke-test-wav-as-16khz-mono-pcm");
        let wav_path = root.join("smoke.wav");

        write_smoke_test_wav(&wav_path).expect("write smoke wav");

        let reader = hound::WavReader::open(&wav_path).expect("read smoke wav");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(reader.duration(), 8_000);
    }

    #[test]
    fn extracts_text_from_sherpa_json_output() {
        let output = r#"{"lang":"<|zh|>","text":"你好，测试成功。"}"#;

        assert_eq!(extract_transcription_text(output), "你好，测试成功。");
    }

    #[test]
    fn ignores_sherpa_status_lines_without_text() {
        let output = r#"
Started
Done!
C:\Users\TOM\AppData\Local\com.local.hivoicer\transcodes\sample-16k-mono.wav
----
num threads: 4
decoding method: greedy_search
Elapsed seconds: 0.16
Real time factor: 0.04
"#;

        assert_eq!(extract_transcription_text(output), "");
    }

    #[test]
    fn prefers_sherpa_json_text_over_status_lines() {
        let output = r#"
Started
decoding method: greedy_search
{"lang":"<|zh|>","text":"hello from audio"}
Elapsed seconds: 0.16
"#;

        assert_eq!(extract_transcription_text(output), "hello from audio");
    }

    #[test]
    fn extracts_text_from_multiline_sherpa_json_output() {
        let output = r#"
Started
{
  "language": "zh",
  "text": "这是音频里的文字",
  "tokens": ["这是", "音频", "里", "的", "文字"]
}
Elapsed seconds: 0.16
"#;

        assert_eq!(extract_transcription_text(output), "这是音频里的文字");
    }

    #[test]
    fn ignores_multiline_json_without_text() {
        let output = r#"
{
  "language": "zh",
  "duration": 2.1
}
"#;

        assert_eq!(extract_transcription_text(output), "");
    }

    #[test]
    fn ignores_empty_text_json_with_metadata() {
        let output = r#"{"language":"zh","text":"","timestamps":[],"durations":[],"tokens":[],"ys_log_probs":[],"words":[]}"#;

        assert_eq!(extract_transcription_text(output), "");
    }

    #[test]
    fn ignores_split_json_metadata_lines_without_complete_text() {
        let output = r#"
"language": "zh",
"text": "",
"timestamps": [],
"durations": [],
"tokens": [],
"words": []
"#;

        assert_eq!(extract_transcription_text(output), "");
    }

    #[test]
    fn builds_masked_websocket_binary_frame() {
        let frame = websocket_frame(0x2, b"abc");

        assert_eq!(frame[0], 0x82);
        assert_eq!(frame[1], 0x83);
        assert_eq!(&frame[2..6], &[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(frame[6], b'a' ^ 0x12);
        assert_eq!(frame[7], b'b' ^ 0x34);
        assert_eq!(frame[8], b'c' ^ 0x56);
    }

    #[test]
    fn builds_sherpa_websocket_payload_from_wav() {
        let root = test_root("builds-sherpa-websocket-payload-from-wav");
        let wav_path = root.join("sample.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&wav_path, spec).expect("create wav");
        writer.write_sample(0i16).expect("write first sample");
        writer.write_sample(16384i16).expect("write second sample");
        writer.finalize().expect("finalize wav");

        let payload = wav_to_sherpa_websocket_payload(&wav_path).expect("payload");

        assert_eq!(&payload[0..4], &16000i32.to_le_bytes());
        assert_eq!(&payload[4..8], &8i32.to_le_bytes());
        assert_eq!(f32::from_le_bytes(payload[8..12].try_into().unwrap()), 0.0);
        assert_eq!(f32::from_le_bytes(payload[12..16].try_into().unwrap()), 0.5);
    }

    #[test]
    fn writes_plain_text_transcription_output() {
        let root = test_root("writes-plain-text-transcription-output");
        let source_audio = root.join("sample.wav");
        let sherpa_audio = root.join("sample-16k.wav");
        fs::write(&source_audio, b"source").expect("write source");
        write_test_wav(&sherpa_audio);

        let (primary, outputs, files) =
            write_transcription_outputs(&root, "plainText", &source_audio, &sherpa_audio, "hello")
                .expect("outputs");

        assert_eq!(outputs.len(), 3);
        assert_eq!(files.len(), 3);
        assert!(PathBuf::from(&primary).starts_with(&root));
        assert!(primary.ends_with(".txt"));
        assert_eq!(fs::read_to_string(&files[0].path).expect("txt"), "hello");
    }

    #[test]
    fn writes_timeline_text_and_srt_outputs() {
        let root = test_root("writes-timeline-text-and-srt-outputs");
        let source_audio = root.join("sample.wav");
        let sherpa_audio = root.join("sample-16k.wav");
        fs::write(&source_audio, b"source").expect("write source");
        write_test_wav(&sherpa_audio);

        let (primary, outputs, files) = write_transcription_outputs(
            &root,
            "timelineText",
            &source_audio,
            &sherpa_audio,
            "hello",
        )
        .expect("outputs");

        assert!(PathBuf::from(&primary).starts_with(&root));
        assert!(primary.ends_with("-timeline.txt"));
        assert_eq!(outputs.len(), 3);
        assert_eq!(
            files
                .iter()
                .map(|file| file.format.as_str())
                .collect::<Vec<_>>(),
            vec!["plainText", "timelineText", "srt"]
        );
        assert!(fs::read_to_string(&files[1].path)
            .expect("timeline txt")
            .contains("[00:00:00:00 -->"));
        assert!(fs::read_to_string(&files[2].path)
            .expect("srt")
            .contains("00:00:00,000 -->"));
    }

    #[test]
    fn formats_timeline_timestamp_as_davinci_timecode() {
        assert_eq!(format_timeline_timestamp(80.5), "00:01:20:12");
    }

    #[test]
    fn rewrites_sherpa_num_threads_for_concurrent_workers() {
        assert_eq!(
            sherpa_args_for_runtime("--tokens=a --num-threads=4 --model=b", Some(2), "cpu")
                .expect("args"),
            vec![
                "--tokens=a".to_string(),
                "--num-threads=2".to_string(),
                "--model=b".to_string()
            ]
        );
        assert_eq!(
            sherpa_args_for_runtime("--tokens=a --num-threads 4", Some(1), "cpu").expect("args"),
            vec![
                "--tokens=a".to_string(),
                "--num-threads".to_string(),
                "1".to_string()
            ]
        );
    }

    #[test]
    fn cuda_runtime_args_force_cuda_provider() {
        assert_eq!(
            sherpa_args_for_runtime("--tokens=a --model=b", Some(2), "cuda").expect("args"),
            vec![
                "--tokens=a".to_string(),
                "--model=b".to_string(),
                "--num-threads=2".to_string(),
                "--provider=cuda".to_string()
            ]
        );
    }

    #[test]
    fn cuda_runtime_args_replace_existing_provider_forms() {
        assert_eq!(
            sherpa_args_for_runtime("--provider=cpu --tokens=a", None, "cuda").expect("args"),
            vec!["--provider=cuda".to_string(), "--tokens=a".to_string()]
        );
        assert_eq!(
            sherpa_args_for_runtime("--provider cpu --tokens=a", None, "cuda").expect("args"),
            vec![
                "--provider".to_string(),
                "cuda".to_string(),
                "--tokens=a".to_string()
            ]
        );
    }

    #[test]
    fn cpu_runtime_args_preserve_default_provider_behavior() {
        assert_eq!(
            sherpa_args_for_runtime("--tokens=a --model=b", Some(1), "cpu").expect("args"),
            vec![
                "--tokens=a".to_string(),
                "--model=b".to_string(),
                "--num-threads=1".to_string()
            ]
        );
    }

    #[test]
    fn splits_long_wav_into_fixed_chunks() {
        let root = test_root("splits-long-wav-into-fixed-chunks");
        let wav_path = root.join("long.wav");
        write_test_wav_seconds(&wav_path, 3);

        let chunks =
            split_wav_into_chunks_in_dir(&wav_path, &root.join("chunks"), 1).expect("chunks");

        assert_eq!(chunks.len(), 3);
        for chunk in chunks {
            let duration = wav_duration_seconds(&chunk).expect("duration");
            assert!((duration - 1.0).abs() < 0.01);
        }
    }

    fn write_test_wav(path: &Path) {
        write_test_wav_seconds(path, 1);
    }

    fn write_test_wav_seconds(path: &Path, seconds: usize) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        for _ in 0..(16000 * seconds) {
            writer.write_sample(0i16).expect("write sample");
        }
        writer.finalize().expect("finalize wav");
    }
}
