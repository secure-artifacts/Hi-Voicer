mod app_state;
mod config;

use app_state::AppSnapshot;
use config::{HotwordRule, UserSettings};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use std::{
    collections::{hash_map::DefaultHasher, VecDeque},
    fs,
    fs::File,
    hash::{Hash, Hasher},
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
const SHERPA_WEBSOCKET_DAEMON_ENABLED: bool = false;
const SHERPA_WEBSOCKET_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const DAVINCI_TIMECODE_FPS: u64 = 25;
const LONG_AUDIO_CHUNK_SECONDS: u32 = 60;
const LONG_AUDIO_THRESHOLD_SECONDS: f64 = 300.0;
const LLM_ASR_CHUNK_SECONDS: u32 = 20;
const MIN_RECORDING_SECONDS: f64 = 0.05;
const SHERPA_RUNTIME_TAG: &str = "v1.13.2";
const SHERPA_CPU_RUNTIME_NAME: &str = "sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts";
const SHERPA_CPU_ARCHIVE_NAME: &str =
    "sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts.tar.bz2";
const SHERPA_CPU_RUNTIME_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts.tar.bz2";
const SHERPA_CUDA_RUNTIME_NAME: &str = "sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda";
const SHERPA_CUDA_REQUIRED_DLLS: &[&str] = &[
    "cudart64_12.dll",
    "cublas64_12.dll",
    "cublasLt64_12.dll",
    "cufft64_11.dll",
    "cudnn64_9.dll",
    "cudnn_adv64_9.dll",
    "cudnn_cnn64_9.dll",
    "cudnn_engines_precompiled64_9.dll",
    "cudnn_engines_runtime_compiled64_9.dll",
    "cudnn_graph64_9.dll",
    "cudnn_heuristic64_9.dll",
    "cudnn_ops64_9.dll",
];

struct RuntimeState {
    settings: Mutex<UserSettings>,
    recording: Mutex<Option<RecordingSession>>,
    sherpa_daemon: Mutex<Option<SherpaDaemon>>,
    sherpa_runtime_install: Mutex<()>,
    cuda_disabled_reason: Mutex<Option<String>>,
    cuda_startup_checked_runtime: Mutex<Option<String>>,
}

struct RecordingSession {
    source: String,
    tracks: Vec<RecordingTrack>,
    path: PathBuf,
    paste_target_window: Option<PasteTargetWindow>,
}

struct RecordingTrack {
    stream: cpal::Stream,
    writer: Arc<Mutex<Option<hound::WavWriter<BufWriter<File>>>>>,
    path: PathBuf,
}

struct StoppedRecording {
    source: String,
    primary_path: PathBuf,
    output_paths: Vec<PathBuf>,
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
type PasteTargetWindow = isize;

#[cfg(not(windows))]
type PasteTargetWindow = ();

fn clipboard_settle_delay(text: &str) -> Duration {
    let extra = (text.chars().count() as u64 / 400).saturating_mul(40);
    Duration::from_millis(80 + extra.min(900))
}

#[cfg(windows)]
fn capture_paste_target_window() -> Option<PasteTargetWindow> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0 == 0 {
        None
    } else {
        Some(hwnd.0)
    }
}

#[cfg(not(windows))]
fn capture_paste_target_window() -> Option<PasteTargetWindow> {
    None
}

#[cfg(windows)]
fn focus_paste_target_window(target_window: Option<PasteTargetWindow>) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SetForegroundWindow};

    let Some(raw_hwnd) = target_window else {
        return;
    };
    let hwnd = HWND(raw_hwnd);
    if unsafe { IsWindow(hwnd).as_bool() } {
        let _ = unsafe { SetForegroundWindow(hwnd) };
        thread::sleep(Duration::from_millis(140));
    }
}

#[cfg(windows)]
fn paste_text_to_target_window(
    text: &str,
    target_window: Option<PasteTargetWindow>,
) -> Result<(), String> {
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
    thread::sleep(clipboard_settle_delay(text));
    focus_paste_target_window(target_window);

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
        return Err("Automatic paste failed.".to_string());
    }

    Ok(())
}

#[cfg(not(windows))]
fn paste_text_to_target_window(
    _text: &str,
    _target_window: Option<PasteTargetWindow>,
) -> Result<(), String> {
    Err("Automatic paste is only supported on Windows.".to_string())
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
    #[serde(default)]
    hotwords: Vec<HotwordRule>,
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

fn apply_hotwords(text: &str, hotwords: &[HotwordRule]) -> String {
    let mut rules = hotwords
        .iter()
        .filter(|rule| rule.enabled && !rule.source.trim().is_empty())
        .collect::<Vec<_>>();
    rules.sort_by(|left, right| {
        right
            .source
            .chars()
            .count()
            .cmp(&left.source.chars().count())
    });

    let mut next = text.to_string();
    for rule in rules {
        next = next.replace(rule.source.trim(), rule.target.as_str());
    }
    next
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
    _acceleration_mode: &str,
) -> TranscriptionPerformance {
    performance
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirectMlProbeRequest {
    model_dir: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DirectMlAdapterInfo {
    name: String,
    driver_version: Option<String>,
    adapter_ram_mb: Option<u64>,
    status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DirectMlSessionProbeCliResult {
    ok: bool,
    message: String,
    model_inputs: Vec<String>,
    model_outputs: Vec<String>,
    onnx_runtime_build: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectMlProbeResult {
    directml_candidate: bool,
    provider_session_ready: bool,
    provider_session_error: Option<String>,
    split_model_ready: bool,
    split_model_dir: Option<String>,
    split_model_missing_files: Vec<String>,
    split_model_session_ready: bool,
    split_model_session_error: Option<String>,
    split_model_inputs: Vec<String>,
    split_model_outputs: Vec<String>,
    model_ready: bool,
    directml_session_ready: bool,
    directml_session_error: Option<String>,
    onnx_runtime_build: Option<String>,
    model_inputs: Vec<String>,
    model_outputs: Vec<String>,
    model_id: Option<String>,
    model_name: Option<String>,
    model_dir: String,
    missing_files: Vec<String>,
    adapters: Vec<DirectMlAdapterInfo>,
    elapsed_ms: u128,
    message: String,
    next_step: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeAudioDiagnostics {
    microphone_available: bool,
    microphone_name: Option<String>,
    microphone_detail: Option<String>,
    system_audio_available: bool,
    system_audio_name: Option<String>,
    system_audio_detail: Option<String>,
    ffmpeg_installed: bool,
    ffmpeg_path: Option<String>,
    ffmpeg_detail: Option<String>,
    message: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TranscribeFileResult {
    text: String,
    output_path: String,
    output_paths: Vec<String>,
    output_files: Vec<TranscriptionOutputFile>,
    segments: Vec<TranscriptSegment>,
    timeline_kind: String,
    source_audio_path: String,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportAudioSegmentRequest {
    source_audio_path: String,
    start_seconds: f64,
    end_seconds: f64,
    destination_dir: Option<String>,
    suggested_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessAudioFileRequest {
    audio_path: String,
    options: AudioProcessingOptions,
    destination_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAudioFilesInDirectoryRequest {
    directory_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioPreviewRequest {
    audio_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenPathDirRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbeMediaFrameRateRequest {
    media_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeMediaFrameRateResult {
    fps: f64,
    source: String,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioWaveformRequest {
    media_path: String,
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioWaveformResult {
    waveform_path: String,
    duration_seconds: f64,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConvertAudioFileRequest {
    audio_path: String,
    output_format: String,
    destination_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClipAudioSegmentRequest {
    source_audio_path: String,
    start_seconds: f64,
    end_seconds: f64,
    output_format: String,
    destination_dir: Option<String>,
    suggested_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClipAudioSegmentSpec {
    start_seconds: f64,
    end_seconds: f64,
    suggested_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClipAudioSegmentsRequest {
    source_audio_path: String,
    segments: Vec<ClipAudioSegmentSpec>,
    output_format: String,
    destination_dir: Option<String>,
    merge_segments: bool,
    suggested_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SplitAudioFileRequest {
    source_audio_path: String,
    segment_seconds: f64,
    output_format: String,
    destination_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeAudioFilesRequest {
    audio_paths: Vec<String>,
    mode: String,
    output_format: String,
    destination_dir: Option<String>,
    suggested_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioProcessingOptions {
    preset: String,
    normalize: bool,
    trim_silence: bool,
    hum_reduction: bool,
    voice_filter: bool,
    noise_reduction: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioProcessingResult {
    output_path: String,
    message: String,
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

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RecordingErrorEvent {
    message: String,
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
    let raw_settings = if path.exists() {
        let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
        serde_json::from_str(&raw).map_err(|error| error.to_string())?
    } else {
        UserSettings::default()
    };
    let loaded_settings = raw_settings.clone().normalized();

    let settings = bind_installed_model_if_available(app, loaded_settings.clone())?;
    if settings != raw_settings {
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
fn suppress_windows_fault_dialogs() {
    use windows::Win32::System::Diagnostics::Debug::{
        SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX,
    };

    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
}

#[cfg(not(windows))]
fn suppress_windows_fault_dialogs() {}

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

fn sherpa_runtime_relative_executable(runtime_name: &str) -> PathBuf {
    PathBuf::from("engines")
        .join("sherpa")
        .join(SHERPA_RUNTIME_TAG)
        .join(runtime_name)
        .join("bin")
        .join("sherpa-onnx-offline.exe")
}

fn sherpa_runtime_executable_path(app: &AppHandle, runtime_name: &str) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?;
    Ok(data_dir.join(sherpa_runtime_relative_executable(runtime_name)))
}

fn sherpa_runtime_search_roots(
    data_dir: &Path,
    resource_dir: Option<&Path>,
    executable_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let mut roots = vec![data_dir.to_path_buf()];
    if let Some(resource_dir) = resource_dir {
        push_unique_path(&mut roots, resource_dir.to_path_buf());
    }
    if let Some(executable_dir) = executable_dir {
        push_unique_path(&mut roots, executable_dir.to_path_buf());
    }
    roots
}

fn sherpa_runtime_search_roots_for_app(app: &AppHandle) -> Result<Vec<PathBuf>, String> {
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?;
    let resource_dir = app.path().resource_dir().ok();
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));

    Ok(sherpa_runtime_search_roots(
        &data_dir,
        resource_dir.as_deref(),
        executable_dir.as_deref(),
    ))
}

fn sherpa_runtime_executable_candidates(roots: &[PathBuf], runtime_name: &str) -> Vec<PathBuf> {
    let relative = sherpa_runtime_relative_executable(runtime_name);
    let mut candidates = Vec::new();
    for root in roots {
        push_unique_path(&mut candidates, root.join(&relative));
    }
    candidates
}

fn find_sherpa_runtime_executable(
    app: &AppHandle,
    runtime_name: &str,
) -> Result<Option<PathBuf>, String> {
    let roots = sherpa_runtime_search_roots_for_app(app)?;
    Ok(sherpa_runtime_executable_candidates(&roots, runtime_name)
        .into_iter()
        .find(|path| path.exists()))
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

fn ffmpeg_runtime_search_roots(
    data_dir: &Path,
    resource_dir: Option<&Path>,
    executable_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let mut roots = vec![data_dir.join("engines").join("ffmpeg")];
    if let Some(resource_dir) = resource_dir {
        roots.push(resource_dir.join("engines").join("ffmpeg"));
        roots.push(resource_dir.join("ffmpeg"));
    }
    if let Some(executable_dir) = executable_dir {
        roots.push(executable_dir.join("engines").join("ffmpeg"));
        roots.push(executable_dir.join("ffmpeg"));
    }
    roots
}

fn ffmpeg_runtime_search_roots_for_app(app: &AppHandle) -> Result<Vec<PathBuf>, String> {
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?;
    let resource_dir = app.path().resource_dir().ok();
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));

    Ok(ffmpeg_runtime_search_roots(
        &data_dir,
        resource_dir.as_deref(),
        executable_dir.as_deref(),
    ))
}

fn find_ffmpeg_in_roots(roots: &[PathBuf]) -> Result<Option<PathBuf>, String> {
    for root in roots {
        if let Some(executable) = find_file_recursive(root, "ffmpeg.exe")? {
            return Ok(Some(executable));
        }
    }
    Ok(None)
}

fn system_ffmpeg_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("where");
        command.arg("ffmpeg");
        command
    };

    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new("which");
        command.arg("ffmpeg");
        command
    };

    let output =
        run_command_with_timeout(&mut command, Duration::from_secs(5), "ffmpeg lookup").ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .find(|path| path.exists())
}

fn ffmpeg_missing_detail(roots: &[PathBuf]) -> String {
    let mut locations = roots
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    locations.push("system PATH".to_string());

    format!(
        "ffmpeg.exe was not found. Place ffmpeg.exe under one of these folders, or add ffmpeg to PATH: {}. Hi-Voicer will not download ffmpeg automatically.",
        locations.join(" | ")
    )
}

fn ffmpeg_missing_message() -> String {
    "ffmpeg is required for offline audio transcoding, subtitle segment export, recording mixdown, and audio processing. Install ffmpeg locally or place ffmpeg.exe under the app data engines\\ffmpeg folder or next to the application in an engines\\ffmpeg folder; Hi-Voicer will not download it automatically.".to_string()
}

fn resolve_ffmpeg_runtime(app: &AppHandle) -> Result<PathBuf, String> {
    let roots = ffmpeg_runtime_search_roots_for_app(app)?;
    if let Some(executable) = find_ffmpeg_in_roots(&roots)? {
        return Ok(executable);
    }
    system_ffmpeg_path().ok_or_else(|| {
        format!(
            "{} {}",
            ffmpeg_missing_message(),
            ffmpeg_missing_detail(&roots)
        )
    })
}

fn installed_ffmpeg_runtime(app: &AppHandle) -> Result<Option<PathBuf>, String> {
    let roots = ffmpeg_runtime_search_roots_for_app(app)?;

    if let Some(executable) = find_ffmpeg_in_roots(&roots)? {
        return Ok(Some(executable));
    }

    Ok(system_ffmpeg_path())
}

fn system_ffprobe_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("where");
        command.arg("ffprobe");
        command
    };

    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new("which");
        command.arg("ffprobe");
        command
    };

    let output =
        run_command_with_timeout(&mut command, Duration::from_secs(5), "ffprobe lookup").ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .find(|path| path.exists())
}

fn resolve_ffprobe_runtime(app: &AppHandle) -> Result<PathBuf, String> {
    let ffmpeg = resolve_ffmpeg_runtime(app)?;
    if let Some(parent) = ffmpeg.parent() {
        let adjacent = parent.join(if cfg!(windows) {
            "ffprobe.exe"
        } else {
            "ffprobe"
        });
        if adjacent.exists() {
            return Ok(adjacent);
        }
    }

    system_ffprobe_path().ok_or_else(|| {
        "ffprobe was not found. Frame-rate detection will fall back to 25fps unless ffprobe is installed next to ffmpeg or on PATH.".to_string()
    })
}

fn install_sherpa_model(app: AppHandle, model: ModelInstallRequest) -> Result<String, String> {
    if model.model_files.is_empty() || model.sherpa_args.trim().is_empty() {
        return Err(format!("{} has no Sherpa install recipe", model.name));
    }

    let total_steps = model.model_files.len() + 3;
    emit_model_install_progress(
        &app,
        &model.id,
        "Preparing Sherpa-ONNX runtime...".to_string(),
        0,
        total_steps,
    );
    let executable = install_sherpa_runtime(&app)?;
    emit_model_install_progress(
        &app,
        &model.id,
        "Sherpa-ONNX runtime is ready.".to_string(),
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
                "Downloading model file {}/{}: {}",
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
        "Writing local model config...".to_string(),
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
        format!("{} has been installed.", model.name),
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
        return Err(format!(
            "Failed to download {}: {}",
            model.name,
            response.status()
        ));
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
            message: "No offline model has been configured.".to_string(),
        };
    }

    let model_dir = PathBuf::from(trimmed);
    if !model_dir.exists() {
        return ModelValidationResult {
            valid: false,
            model_name: String::new(),
            message: "The model directory does not exist. Download it again or select another model directory.".to_string(),
        };
    }

    let engine_path = model_dir.join("engine.json");
    if !engine_path.exists() {
        return ModelValidationResult {
            valid: false,
            model_name: String::new(),
            message: "engine.json was not found in the model directory. Use one-click download in Settings.".to_string(),
        };
    }

    let raw_config = match fs::read_to_string(engine_path) {
        Ok(raw_config) => raw_config,
        Err(error) => {
            return ModelValidationResult {
                valid: false,
                model_name: String::new(),
                message: format!("Failed to read model config: {error}"),
            }
        }
    };
    let engine: InstalledEngineConfig = match serde_json::from_str(&raw_config) {
        Ok(engine) => engine,
        Err(error) => {
            return ModelValidationResult {
                valid: false,
                model_name: String::new(),
                message: format!("Invalid model config format: {error}"),
            }
        }
    };

    if engine.engine != "sherpa-onnx" {
        return ModelValidationResult {
            valid: false,
            model_name: engine.model_name,
            message: format!("Unsupported engine: {}", engine.engine),
        };
    }

    if !PathBuf::from(&engine.executable).exists() {
        return ModelValidationResult {
            valid: false,
            model_name: engine.model_name,
            message:
                "The Sherpa-ONNX executable does not exist. Download and configure the model again."
                    .to_string(),
        };
    }

    for required_file in &engine.required_files {
        let relative_path = match ensure_relative_file_path(required_file) {
            Ok(relative_path) => relative_path,
            Err(error) => {
                return ModelValidationResult {
                    valid: false,
                    model_name: engine.model_name.clone(),
                    message: format!("Unsafe file path in model config: {error}"),
                }
            }
        };

        if !model_dir.join(relative_path).exists() {
            return ModelValidationResult {
                valid: false,
                model_name: engine.model_name.clone(),
                message: format!(
                    "Missing model file: {required_file}. Download and configure the model again."
                ),
            };
        }
    }

    ModelValidationResult {
        valid: true,
        model_name: engine.model_name,
        message: "Model is ready.".to_string(),
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
        return Err("Sherpa arguments contain an unclosed quote.".to_string());
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
    _runtime_mode: &str,
) -> Result<Vec<String>, String> {
    let mut parsed = split_command_args(args)?;
    if let Some(threads) = threads {
        set_sherpa_arg_value(&mut parsed, "--num-threads", &threads.to_string(), true);
    }

    Ok(parsed)
}

fn text_from_sherpa_json_value(value: &serde_json::Value) -> Option<String> {
    let text = value.get("text").and_then(|text| text.as_str())?.trim();
    if text.is_empty() {
        None
    } else if text == "language"
        && value
            .get("timestamps")
            .and_then(|timestamps| timestamps.as_array())
            .is_some_and(|timestamps| timestamps.is_empty())
        && value
            .get("tokens")
            .and_then(|tokens| tokens.as_array())
            .is_some_and(|tokens| {
                tokens.len() == 1
                    && tokens
                        .first()
                        .and_then(|token| token.as_str())
                        .is_some_and(|token| token == "language")
            })
    {
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
    let ffmpeg = resolve_ffmpeg_runtime(app)?;
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
        return Err(format!("ffmpeg transcode failed: {}", stderr.trim()));
    }

    Ok(output_path)
}

fn read_sherpa_engine(model_dir: &Path) -> Result<InstalledEngineConfig, String> {
    let engine_path = model_dir.join("engine.json");
    if !engine_path.exists() {
        return Err("engine.json was not found in the model directory. Configure a Sherpa model in Settings first.".to_string());
    }

    let raw_config = fs::read_to_string(engine_path).map_err(|error| error.to_string())?;
    let engine: InstalledEngineConfig =
        serde_json::from_str(&raw_config).map_err(|error| error.to_string())?;
    if engine.engine != "sherpa-onnx" {
        return Err(format!("Unsupported engine: {}", engine.engine));
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

fn push_existing_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if path.exists() {
        push_unique_path(paths, path);
    }
}

fn push_cuda_bin_candidates(paths: &mut Vec<PathBuf>, bin: PathBuf) {
    push_existing_unique_path(paths, bin.clone());
    if let Ok(entries) = fs::read_dir(&bin) {
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                push_existing_unique_path(paths, child);
            }
        }
    }
}

fn push_cuda_root_candidates(paths: &mut Vec<PathBuf>, root: PathBuf) {
    push_cuda_bin_candidates(paths, root.join("bin"));
    push_existing_unique_path(paths, root.clone());
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                push_cuda_bin_candidates(paths, child.join("bin"));
                push_cuda_bin_candidates(paths, child.join("cuda").join("bin"));
            }
        }
    }
}

fn cuda_dependency_search_dirs(runtime_executable: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(parent) = runtime_executable.parent() {
        push_existing_unique_path(&mut dirs, parent.to_path_buf());
    }

    if let Some(path_value) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path_value) {
            push_existing_unique_path(&mut dirs, path);
        }
    }

    for (key, value) in std::env::vars_os() {
        let key = key.to_string_lossy().to_ascii_uppercase();
        if key == "CUDA_PATH" || key.starts_with("CUDA_PATH_V") || key == "CUDNN_PATH" {
            push_cuda_root_candidates(&mut dirs, PathBuf::from(value));
        }
    }

    for env_name in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(env_name) {
            let root = PathBuf::from(root);
            push_cuda_root_candidates(
                &mut dirs,
                root.join("NVIDIA GPU Computing Toolkit").join("CUDA"),
            );
            push_cuda_root_candidates(&mut dirs, root.join("NVIDIA").join("CUDNN"));
            push_cuda_root_candidates(&mut dirs, root.join("NVIDIA Corporation").join("CUDNN"));
        }
    }

    dirs
}

fn cuda_dependency_status_from_dirs(
    dirs: &[PathBuf],
    required_dlls: &[&str],
) -> Result<Vec<PathBuf>, String> {
    let existing_dirs = dirs
        .iter()
        .filter(|dir| dir.exists())
        .cloned()
        .collect::<Vec<_>>();
    let mut used_dirs = Vec::new();
    let mut missing = Vec::new();

    for dll in required_dlls {
        if let Some(dir) = existing_dirs.iter().find(|dir| dir.join(dll).is_file()) {
            push_unique_path(&mut used_dirs, dir.clone());
        } else {
            missing.push(*dll);
        }
    }

    if missing.is_empty() {
        return Ok(used_dirs);
    }

    let searched = if existing_dirs.is_empty() {
        "no existing CUDA library directories were found".to_string()
    } else {
        existing_dirs
            .iter()
            .map(|dir| dir.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };

    Err(format!(
        "CUDA dependencies are incomplete; missing: {}. Install NVIDIA CUDA Toolkit 12.x and cuDNN 9.x, or add their bin directories to PATH. Searched: {searched}",
        missing.join(", ")
    ))
}

fn cuda_dependency_status(runtime_executable: &Path) -> Result<Vec<PathBuf>, String> {
    cuda_dependency_status_from_dirs(
        &cuda_dependency_search_dirs(runtime_executable),
        SHERPA_CUDA_REQUIRED_DLLS,
    )
}

fn configure_cuda_command_environment(
    command: &mut Command,
    executable: &Path,
    runtime_mode: &str,
) -> Result<(), String> {
    if !runtime_mode.eq_ignore_ascii_case("cuda") {
        return Ok(());
    }

    let dependency_dirs = cuda_dependency_status(executable)?;
    let mut path_entries = dependency_dirs;
    if let Some(existing_path) = std::env::var_os("PATH") {
        path_entries.extend(std::env::split_paths(&existing_path));
    }
    let joined_path = std::env::join_paths(path_entries)
        .map_err(|error| format!("failed to build CUDA PATH: {error}"))?;
    command.env("PATH", joined_path);
    Ok(())
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
            "No usable nvidia-smi was found; tried: {tried}. Errors: {}",
            errors.join("; ")
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
                format!("CUDA is disabled for this session; using CPU: {reason}"),
            )
        } else if let Some(error) = prepare_error {
            (
                "cpu",
                format!("CUDA runtime check failed; using CPU: {error}"),
            )
        } else {
            match (cuda_available, cuda_runtime_installed) {
                (false, _) => (
                    "cpu",
                    "CUDA is selected, but no NVIDIA CUDA environment was detected; transcription will use CPU.".to_string(),
                ),
                (true, false) => (
                    "cpu",
                    "NVIDIA CUDA was detected, but no local CUDA-capable Sherpa runtime was found; transcription will use CPU. Hi-Voicer will not download CUDA files automatically.".to_string(),
                ),
                (true, true) => (
                    "cuda",
                    "CUDA and the CUDA runtime are ready; failures will still fall back to CPU.".to_string(),
                ),
            }
        }
    } else if let Some(error) = prepare_error {
        (
            "cpu",
            format!("CUDA runtime check failed; using CPU: {error}"),
        )
    } else {
        (
            "cpu",
            "CPU is selected for maximum compatibility; CUDA runtime will not be loaded."
                .to_string(),
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
    _requested_mode: &str,
) -> Result<AccelerationStatus, String> {
    let cpu_runtime_installed =
        sherpa_runtime_executable_path(app, SHERPA_CPU_RUNTIME_NAME)?.exists();

    Ok(acceleration_status_from_parts(
        "cpu",
        false,
        None,
        None,
        cpu_runtime_installed,
        false,
        None,
        None,
    ))
}

fn prepare_acceleration_runtime_for_app(
    app: &AppHandle,
    requested_mode: &str,
) -> Result<AccelerationStatus, String> {
    acceleration_status_for_app(app, requested_mode)
}

fn resolve_sherpa_runtime(
    _app: &AppHandle,
    engine: &InstalledEngineConfig,
    _requested_mode: &str,
) -> ResolvedSherpaRuntime {
    let cpu_runtime_executable = PathBuf::from(&engine.executable);
    let cpu_executable = sherpa_cli_executable_for_engine(engine, &cpu_runtime_executable);
    ResolvedSherpaRuntime {
        executable: cpu_executable,
        mode: "cpu".to_string(),
        fallback_reason: None,
    }
}
fn is_sherpa_streaming_cli_model(engine: &InstalledEngineConfig) -> bool {
    matches!(engine.model_id.as_str(), "sherpa-zipformer-zh")
}

fn sherpa_max_single_pass_seconds(engine: &InstalledEngineConfig) -> f64 {
    match engine.model_id.as_str() {
        "qwen3-asr-0.6b" | "sherpa-funasr-nano" => LLM_ASR_CHUNK_SECONDS as f64,
        _ => LONG_AUDIO_THRESHOLD_SECONDS,
    }
}

fn sherpa_chunk_seconds(engine: &InstalledEngineConfig) -> u32 {
    match engine.model_id.as_str() {
        "qwen3-asr-0.6b" | "sherpa-funasr-nano" => LLM_ASR_CHUNK_SECONDS,
        _ => LONG_AUDIO_CHUNK_SECONDS,
    }
}

fn sherpa_cli_executable_for_engine(
    engine: &InstalledEngineConfig,
    runtime_executable: &Path,
) -> PathBuf {
    if is_sherpa_streaming_cli_model(engine) {
        runtime_executable.with_file_name(if cfg!(windows) {
            "sherpa-onnx.exe"
        } else {
            "sherpa-onnx"
        })
    } else {
        runtime_executable.to_path_buf()
    }
}

fn sherpa_websocket_server_path(engine: &InstalledEngineConfig, executable: &Path) -> PathBuf {
    if is_sherpa_streaming_cli_model(engine) {
        executable.with_file_name(if cfg!(windows) {
            "sherpa-onnx-online-websocket-server.exe"
        } else {
            "sherpa-onnx-online-websocket-server"
        })
    } else {
        executable.with_file_name(if cfg!(windows) {
            "sherpa-onnx-offline-websocket-server.exe"
        } else {
            "sherpa-onnx-offline-websocket-server"
        })
    }
}

fn ensure_sherpa_daemon_running(
    app: &AppHandle,
    engine: &InstalledEngineConfig,
    executable: &Path,
    runtime_mode: &str,
) -> Result<(), String> {
    let server_exe = sherpa_websocket_server_path(engine, executable);
    if !server_exe.exists() {
        return Err(
            "Sherpa WebSocket server executable does not exist; fast mode cannot be enabled."
                .to_string(),
        );
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
    configure_cuda_command_environment(&mut command, &server_exe, runtime_mode)?;
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

fn wav_to_sherpa_websocket_payload(wav_path: &Path) -> Result<Vec<u8>, String> {
    let mut reader = hound::WavReader::open(wav_path).map_err(|error| error.to_string())?;
    let spec = reader.spec();
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Err("Sherpa fast mode only supports 16-bit PCM WAV.".to_string());
    }

    let samples = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let audio_bytes = samples
        .len()
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or_else(|| "Audio is too large to send to the Sherpa service.".to_string())?;

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
            0x8 => return Err("Sherpa WebSocket service closed the connection early.".to_string()),
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
                        return Err("Sherpa WebSocket handshake failed.".to_string());
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
                    return Err(format!(
                        "Sherpa WebSocket handshake failed: {response_text}"
                    ));
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

    Err(format!(
        "Sherpa WebSocket service is not ready: {last_error}"
    ))
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
) -> Result<Vec<AudioChunk>, String> {
    let mut reader = hound::WavReader::open(wav_path).map_err(|error| error.to_string())?;
    let spec = reader.spec();
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Err("Long-audio chunking only supports 16-bit PCM WAV.".to_string());
    }

    fs::create_dir_all(chunk_dir).map_err(|error| error.to_string())?;
    let channels = spec.channels as usize;
    let samples_per_second = (spec.sample_rate as usize).saturating_mul(channels).max(1);
    let target_samples = samples_per_second
        .saturating_mul(chunk_seconds.max(1) as usize)
        .max(samples_per_second);
    let search_radius = samples_per_second.saturating_mul(8);
    let analysis_window = samples_per_second / 4;
    let stem = wav_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");

    let samples = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    let mut boundaries = vec![0usize];
    let mut cursor = 0usize;
    while cursor + target_samples < samples.len() {
        let target = cursor + target_samples;
        let start = target
            .saturating_sub(search_radius)
            .max(cursor + samples_per_second * 20);
        let end = (target + search_radius).min(samples.len().saturating_sub(analysis_window + 1));
        let mut best_index = target.min(samples.len());
        let mut best_score = f64::MAX;

        let step = channels.max(1) * 512;
        let mut probe = start;
        while probe < end {
            let window_end = (probe + analysis_window).min(samples.len());
            let score = samples[probe..window_end]
                .iter()
                .map(|sample| {
                    let value = f64::from(*sample) / f64::from(i16::MAX);
                    value * value
                })
                .sum::<f64>()
                / (window_end.saturating_sub(probe).max(1) as f64);
            if score < best_score {
                best_score = score;
                best_index = probe;
            }
            probe = probe.saturating_add(step);
        }

        let boundary = best_index - (best_index % channels.max(1));
        if boundary <= cursor || boundary >= samples.len() {
            break;
        }
        boundaries.push(boundary);
        cursor = boundary;
    }
    boundaries.push(samples.len());

    let mut chunks = Vec::new();
    for (chunk_index, pair) in boundaries.windows(2).enumerate() {
        let start = pair[0];
        let end = pair[1];
        if end <= start {
            continue;
        }
        let chunk_path = chunk_dir.join(format!("{stem}-part-{chunk_index:04}.wav"));
        let mut writer =
            hound::WavWriter::create(&chunk_path, spec).map_err(|error| error.to_string())?;
        for sample in &samples[start..end] {
            writer
                .write_sample(*sample)
                .map_err(|error| error.to_string())?;
        }
        writer.finalize().map_err(|error| error.to_string())?;
        chunks.push(AudioChunk {
            path: chunk_path,
            start: start as f64 / samples_per_second as f64,
            end: end as f64 / samples_per_second as f64,
        });
    }

    Ok(chunks)
}

fn split_wav_into_chunks(
    app: &AppHandle,
    wav_path: &Path,
    chunk_seconds: u32,
) -> Result<(Vec<AudioChunk>, PathBuf), String> {
    let chunk_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("chunks")
        .join(format!("audio-{}", unix_timestamp_millis()?));
    let chunks = split_wav_into_chunks_in_dir(wav_path, &chunk_dir, chunk_seconds)?;
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

fn frames_to_davinci_timecode(frames: u64) -> String {
    let hours = frames / (DAVINCI_TIMECODE_FPS * 60 * 60);
    let remainder = frames % (DAVINCI_TIMECODE_FPS * 60 * 60);
    let minutes = remainder / (DAVINCI_TIMECODE_FPS * 60);
    let remainder = remainder % (DAVINCI_TIMECODE_FPS * 60);
    let seconds = remainder / DAVINCI_TIMECODE_FPS;
    let frame = remainder % DAVINCI_TIMECODE_FPS;
    format!("{hours:02}:{minutes:02}:{seconds:02}:{frame:02}")
}

fn seconds_to_davinci_frames(seconds: f64) -> u64 {
    (seconds.max(0.0) * DAVINCI_TIMECODE_FPS as f64).round() as u64
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptSegment {
    id: String,
    index: usize,
    start: f64,
    end: f64,
    text: String,
    source_audio_path: String,
}

#[derive(Debug, Clone)]
struct AudioChunk {
    path: PathBuf,
    start: f64,
    end: f64,
}

#[derive(Debug, Clone)]
struct TranscriptTextChunk {
    text: String,
    start: f64,
    end: f64,
}

fn is_primary_sentence_break(character: char) -> bool {
    matches!(character, '。' | '！' | '？' | '!' | '?' | '\n')
}

fn is_secondary_sentence_break(character: char) -> bool {
    matches!(character, '，' | '；' | '、' | ',' | ';')
}

fn is_leading_punctuation(character: char) -> bool {
    matches!(
        character,
        '。' | '！' | '？' | '，' | '、' | '；' | '.' | '!' | '?' | ';' | ','
    )
}

fn split_text_into_chunks(text: &str) -> Vec<String> {
    const TARGET_CHARS: usize = 56;
    const MAX_CHARS: usize = 82;
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut pending_punctuation = String::new();

    for character in text.chars() {
        if current.trim().is_empty() && is_leading_punctuation(character) {
            current.clear();
            if let Some(last) = chunks.last_mut() {
                last.push(character);
            } else {
                pending_punctuation.push(character);
            }
            continue;
        }

        if current.is_empty() && !pending_punctuation.is_empty() {
            current.push_str(&pending_punctuation);
            pending_punctuation.clear();
        }

        current.push(character);
        let char_count = current.chars().count();
        let should_break = is_primary_sentence_break(character)
            || (char_count >= TARGET_CHARS && is_secondary_sentence_break(character))
            || char_count >= MAX_CHARS;

        if should_break {
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

fn build_transcript_segments(
    text: &str,
    duration: f64,
    source_audio_path: &Path,
) -> Vec<TranscriptSegment> {
    const MIN_SEGMENT_SECONDS: f64 = 0.5;
    let chunk_count = split_text_into_chunks(text).len().max(1);
    let duration = if duration.is_finite() && duration > 0.0 {
        duration.max(chunk_count as f64 * MIN_SEGMENT_SECONDS)
    } else {
        duration
    };
    build_transcript_segments_from_chunks(
        &[TranscriptTextChunk {
            text: text.to_string(),
            start: 0.0,
            end: duration,
        }],
        source_audio_path,
    )
}

fn build_transcript_segments_from_chunks(
    transcript_chunks: &[TranscriptTextChunk],
    source_audio_path: &Path,
) -> Vec<TranscriptSegment> {
    const MIN_SEGMENT_SECONDS: f64 = 0.5;
    let mut segments = Vec::new();

    for transcript_chunk in transcript_chunks {
        let text_chunks = split_text_into_chunks(&transcript_chunk.text);
        if text_chunks.is_empty() {
            continue;
        }

        let window_start = transcript_chunk.start.max(0.0);
        let window_end = transcript_chunk.end.max(window_start + 0.1);
        let window_duration = window_end - window_start;
        let segment_floor = MIN_SEGMENT_SECONDS.min(window_duration / text_chunks.len() as f64);
        let minimum_total_duration = text_chunks.len() as f64 * segment_floor;
        let duration = if window_duration.is_finite() && window_duration > 0.0 {
            window_duration
        } else {
            (text_chunks.len() as f64 * 1.2).max(minimum_total_duration)
        };
        let char_counts = text_chunks
            .iter()
            .map(|chunk| {
                chunk
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .count()
                    .max(1)
            })
            .collect::<Vec<_>>();
        let total_chars: usize = char_counts.iter().sum();
        let flexible_duration = (duration - minimum_total_duration).max(0.0);
        let allocated_durations = char_counts
            .iter()
            .map(|char_count| {
                segment_floor + flexible_duration * *char_count as f64 / total_chars as f64
            })
            .collect::<Vec<_>>();
        let allocated_total: f64 = allocated_durations.iter().sum();
        let scale = if allocated_total > 0.0 {
            duration / allocated_total
        } else {
            1.0
        };
        let mut cursor = window_start;

        for (chunk_index, text_chunk) in text_chunks.iter().enumerate() {
            let segment_index = segments.len() + 1;
            let end = if chunk_index + 1 == text_chunks.len() {
                window_start + duration
            } else {
                (cursor + allocated_durations[chunk_index] * scale).min(window_start + duration)
            };
            segments.push(TranscriptSegment {
                id: format!(
                    "segment-{}-{}",
                    segment_index,
                    unix_timestamp_millis().unwrap_or(0)
                ),
                index: segment_index,
                start: cursor,
                end,
                text: text_chunk.to_string(),
                source_audio_path: source_audio_path.to_string_lossy().to_string(),
            });
            cursor = end;
        }
    }

    segments
}

fn transcript_text_from_chunks(chunks: &[TranscriptTextChunk]) -> String {
    chunks
        .iter()
        .map(|chunk| clean_subtitle_text(&chunk.text))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn clean_subtitle_text(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn clean_subtitle_text_one_line(text: &str) -> String {
    clean_subtitle_text(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn timeline_text_from_segments(segments: &[TranscriptSegment]) -> String {
    segments
        .iter()
        .filter_map(|segment| {
            let text = clean_subtitle_text(&segment.text);
            if text.is_empty() {
                return None;
            }
            Some(format!(
                "[{} --> {}]\n{}",
                format_timeline_timestamp(segment.start),
                format_timeline_timestamp(segment.end),
                text
            ))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn timeline_txt_text_from_segments(segments: &[TranscriptSegment]) -> String {
    segments
        .iter()
        .filter_map(|segment| {
            let text = clean_subtitle_text_one_line(&segment.text);
            if text.is_empty() {
                return None;
            }
            Some(format!(
                "[{} --> {}] {}",
                format_timeline_timestamp(segment.start),
                format_timeline_timestamp(segment.end),
                text
            ))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn srt_text_from_segments(segments: &[TranscriptSegment]) -> String {
    segments
        .iter()
        .filter_map(|segment| {
            let text = clean_subtitle_text(&segment.text);
            if text.is_empty() {
                None
            } else {
                Some((segment, text))
            }
        })
        .enumerate()
        .map(|(index, (segment, text))| {
            format!(
                "{}\n{} --> {}\n{}",
                index + 1,
                format_srt_timestamp(segment.start),
                format_srt_timestamp(segment.end),
                text
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn clean_resolve_marker_text(text: &str, max_length: usize) -> String {
    let mut without_tags = String::new();
    let mut in_tag = false;
    for character in text.chars() {
        match character {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => without_tags.push(character),
            _ => {}
        }
    }

    let one_line = without_tags
        .replace('|', "/")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if one_line.is_empty() {
        return "Subtitle marker".to_string();
    }
    if one_line.chars().count() > max_length {
        let mut trimmed = one_line
            .chars()
            .take(max_length.saturating_sub(1))
            .collect::<String>();
        trimmed = trimmed.trim_end().to_string();
        trimmed.push_str("...");
        return trimmed;
    }
    one_line
}

fn resolve_marker_edl_from_segments(title: &str, segments: &[TranscriptSegment]) -> String {
    const TIMELINE_START_FRAMES: u64 = DAVINCI_TIMECODE_FPS * 60 * 60;
    const MARKER_DURATION_FRAMES: u64 = 1;
    const MAX_MARKER_TEXT_LENGTH: usize = 180;

    let mut lines = vec![
        format!("TITLE: {title}"),
        "FCM: NON-DROP FRAME".to_string(),
        String::new(),
    ];

    for (event_index, segment) in segments.iter().enumerate() {
        let start_frame = TIMELINE_START_FRAMES + seconds_to_davinci_frames(segment.start);
        let subtitle_duration = seconds_to_davinci_frames(segment.end - segment.start).max(1);
        let duration_frames = MARKER_DURATION_FRAMES.max(1).min(subtitle_duration.max(1));
        let end_frame = start_frame + duration_frames;
        let start_tc = frames_to_davinci_timecode(start_frame);
        let end_tc = frames_to_davinci_timecode(end_frame);
        let text = clean_resolve_marker_text(&segment.text, MAX_MARKER_TEXT_LENGTH);

        lines.push(format!(
            "{:03}  001      V     C        {start_tc} {end_tc} {start_tc} {end_tc}",
            event_index + 1
        ));
        lines.push(format!(
            " |C:ResolveColorYellow |M:{text} |D:{duration_frames}"
        ));
        lines.push(String::new());
    }

    lines.join("\n")
}

fn write_utf8_sig(path: &Path, contents: &str) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(contents.len() + 3);
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(contents.as_bytes());
    fs::write(path, bytes).map_err(|error| error.to_string())
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
    configure_cuda_command_environment(&mut command, executable, runtime_mode)?;
    suppress_command_window(&mut command);

    let duration = wav_duration_seconds(wav_path).unwrap_or(60.0);
    let timeout = Duration::from_secs(((duration * 4.0) as u64 + 120).clamp(120, 14_400));
    let output = run_command_with_timeout(&mut command, timeout, "Sherpa CLI transcription")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    if !output.status.success() {
        return Err(format!("Sherpa transcription failed: {}", combined.trim()));
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
    if allow_daemon && SHERPA_WEBSOCKET_DAEMON_ENABLED {
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
) -> Result<Vec<TranscriptTextChunk>, String> {
    let duration = wav_duration_seconds(wav_path)?;
    let max_single_pass_seconds = sherpa_max_single_pass_seconds(engine);
    if duration <= max_single_pass_seconds {
        emit_transcription_progress(
            app,
            task_id,
            started_at,
            "transcribing",
            35,
            format!(
                "Running local Sherpa model on {}",
                runtime_mode.to_uppercase()
            ),
            0,
            1,
        );
        let text = transcribe_sherpa_wav_once(
            app,
            engine,
            executable,
            wav_path,
            performance,
            false,
            runtime_mode,
        )?;
        emit_transcription_progress(
            app,
            task_id,
            started_at,
            "transcribing",
            92,
            "Transcription complete; generating result files".to_string(),
            1,
            1,
        );
        return Ok(vec![TranscriptTextChunk {
            text,
            start: 0.0,
            end: duration,
        }]);
    }

    let chunk_seconds = sherpa_chunk_seconds(engine);
    let (chunks, chunk_dir) = split_wav_into_chunks(app, wav_path, chunk_seconds)?;
    let total_segments = chunks.len();
    emit_transcription_progress(
        app,
        task_id,
        started_at,
        "splitting",
        15,
        format!("Long audio was split into {total_segments} chunks"),
        0,
        total_segments,
    );

    let worker_count = performance.chunk_workers.max(1).min(total_segments.max(1));
    let queue = Arc::new(Mutex::new(
        chunks
            .iter()
            .cloned()
            .enumerate()
            .collect::<VecDeque<(usize, AudioChunk)>>(),
    ));
    let (sender, receiver) = mpsc::channel::<(usize, Result<TranscriptTextChunk, String>)>();

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
                &chunk.path,
                performance.sherpa_threads,
                &runtime_mode,
            )
            .map(|text| TranscriptTextChunk {
                text,
                start: chunk.start,
                end: chunk.end,
            });
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
            format!("Transcribing chunk {completed}/{total_segments}"),
            completed,
            total_segments,
        );
    }
    for handle in handles {
        let _ = handle.join();
    }
    let _ = fs::remove_dir_all(&chunk_dir);

    let mut transcript_chunks = Vec::new();
    let mut errors = Vec::new();
    for (index, result) in results.into_iter().enumerate() {
        match result {
            Some(Ok(chunk)) if !chunk.text.trim().is_empty() => {
                transcript_chunks.push(TranscriptTextChunk {
                    text: chunk.text.trim().to_string(),
                    start: chunk.start,
                    end: chunk.end,
                })
            }
            Some(Ok(_)) => errors.push(format!("Chunk {} returned no recognized text", index + 1)),
            Some(Err(error)) => errors.push(format!("Chunk {} failed: {error}", index + 1)),
            None => errors.push(format!("Chunk {} returned no result", index + 1)),
        }
    }

    if !transcript_chunks.is_empty() {
        return Ok(transcript_chunks);
    }

    if !errors.is_empty() {
        return Err(format!(
            "Long-audio chunk transcription failed: {}",
            errors.join("; ")
        ));
    }

    Err("Sherpa ran, but no transcription text was parsed after long-audio chunking.".to_string())
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

fn adapter_ram_mb_from_value(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_u64().map(|bytes| bytes / 1024 / 1024),
        serde_json::Value::String(text) => {
            text.parse::<u64>().ok().map(|bytes| bytes / 1024 / 1024)
        }
        _ => None,
    }
}

fn directml_adapter_from_json(value: &serde_json::Value) -> Option<DirectMlAdapterInfo> {
    let name = value.get("Name")?.as_str()?.trim().to_string();
    if name.is_empty() {
        return None;
    }

    Some(DirectMlAdapterInfo {
        name,
        driver_version: value
            .get("DriverVersion")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
        adapter_ram_mb: value.get("AdapterRAM").and_then(adapter_ram_mb_from_value),
        status: value
            .get("Status")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
    })
}

fn directml_adapters_from_powershell_json(text: &str) -> Vec<DirectMlAdapterInfo> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return Vec::new();
    };

    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(directml_adapter_from_json)
            .collect(),
        item => directml_adapter_from_json(&item).into_iter().collect(),
    }
}

fn query_directml_candidate_adapters() -> Vec<DirectMlAdapterInfo> {
    if !cfg!(windows) {
        return Vec::new();
    }

    let mut command = Command::new("powershell");
    command.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        "Get-CimInstance Win32_VideoController | Select-Object Name,DriverVersion,AdapterRAM,Status | ConvertTo-Json -Compress",
    ]);

    let Ok(output) =
        run_command_with_timeout(&mut command, Duration::from_secs(8), "DirectML GPU probe")
    else {
        return Vec::new();
    };

    if !output.status.success() {
        return Vec::new();
    }

    directml_adapters_from_powershell_json(&String::from_utf8_lossy(&output.stdout))
}

fn is_directml_candidate_adapter(adapter: &DirectMlAdapterInfo) -> bool {
    let name = adapter.name.to_ascii_lowercase();
    let status_ok = adapter
        .status
        .as_deref()
        .map(|status| status.eq_ignore_ascii_case("OK"))
        .unwrap_or(true);

    status_ok
        && !name.contains("microsoft basic")
        && !name.contains("remote")
        && !name.contains("render driver")
}

fn ort_outlet_summaries(outlets: &[ort::value::Outlet]) -> Vec<String> {
    outlets
        .iter()
        .map(|outlet| format!("{}: {}", outlet.name(), outlet.dtype()))
        .collect()
}

fn append_proto_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn append_proto_key(out: &mut Vec<u8>, field_number: u32, wire_type: u8) {
    append_proto_varint(out, ((field_number as u64) << 3) | wire_type as u64);
}

fn append_proto_int(out: &mut Vec<u8>, field_number: u32, value: u64) {
    append_proto_key(out, field_number, 0);
    append_proto_varint(out, value);
}

fn append_proto_string(out: &mut Vec<u8>, field_number: u32, value: &str) {
    append_proto_key(out, field_number, 2);
    append_proto_varint(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

fn append_proto_message(out: &mut Vec<u8>, field_number: u32, value: Vec<u8>) {
    append_proto_key(out, field_number, 2);
    append_proto_varint(out, value.len() as u64);
    out.extend_from_slice(&value);
}

#[derive(Debug, Clone)]
struct SenseVoiceLfrFeatures {
    frames: usize,
    values: Vec<f32>,
}

fn hz_to_mel(freq: f32) -> f32 {
    2595.0 * (1.0 + freq / 700.0).log10()
}

fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10.0f32.powf(mel / 2595.0) - 1.0)
}

fn sensevoice_mel_filters(
    sr: usize,
    n_fft: usize,
    n_mels: usize,
    f_min: f32,
    f_max: f32,
) -> Vec<f32> {
    let bins = n_fft / 2 + 1;
    let mel_min = hz_to_mel(f_min);
    let mel_max = hz_to_mel(f_max);
    let mel_points = (0..(n_mels + 2))
        .map(|index| mel_min + (mel_max - mel_min) * index as f32 / (n_mels + 1) as f32)
        .map(mel_to_hz)
        .collect::<Vec<_>>();
    let diffs = mel_points
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect::<Vec<_>>();
    let mut filters = vec![0.0f32; bins * n_mels];

    for bin in 0..bins {
        let freq = bin as f32 * (sr as f32 / 2.0) / (bins - 1) as f32;
        for mel in 0..n_mels {
            let left = mel_points[mel];
            let center = mel_points[mel + 1];
            let right = mel_points[mel + 2];
            let lower = if diffs[mel].abs() > f32::EPSILON {
                (freq - left) / diffs[mel]
            } else {
                0.0
            };
            let upper = if diffs[mel + 1].abs() > f32::EPSILON {
                (right - freq) / diffs[mel + 1]
            } else {
                0.0
            };
            filters[bin * n_mels + mel] = lower.min(upper).max(0.0);
            let _ = center;
        }
    }

    filters
}

fn extract_sensevoice_lfr_features(audio: &[f32]) -> SenseVoiceLfrFeatures {
    const SAMPLE_RATE: usize = 16_000;
    const N_FFT: usize = 400;
    const HOP_LENGTH: usize = 160;
    const N_MELS: usize = 80;
    const LFR_STACK: usize = 7;
    const LFR_SKIP: usize = 6;

    let mut normalized = if audio.is_empty() {
        vec![0.0f32]
    } else {
        let mean = audio.iter().sum::<f32>() / audio.len() as f32;
        audio.iter().map(|sample| sample - mean).collect::<Vec<_>>()
    };

    let mut emphasized = vec![0.0f32; normalized.len()];
    emphasized[0] = normalized[0];
    for index in 1..normalized.len() {
        emphasized[index] = normalized[index] - 0.97 * normalized[index - 1];
    }
    normalized.clear();

    let half_n_fft = N_FFT / 2;
    let mut padded = Vec::with_capacity(emphasized.len() + N_FFT);
    padded.extend(std::iter::repeat(0.0f32).take(half_n_fft));
    padded.extend_from_slice(&emphasized);
    padded.extend(std::iter::repeat(0.0f32).take(half_n_fft));

    let frame_count = if padded.len() >= N_FFT {
        1 + (padded.len() - N_FFT) / HOP_LENGTH
    } else {
        1
    };
    let filters = sensevoice_mel_filters(SAMPLE_RATE, N_FFT, N_MELS, 20.0, 8000.0);
    let window = (0..N_FFT)
        .map(|index| 0.54 - 0.46 * (2.0 * std::f32::consts::PI * index as f32 / N_FFT as f32).cos())
        .collect::<Vec<_>>();

    let bins = N_FFT / 2 + 1;
    let mut log_mel = vec![0.0f32; frame_count * N_MELS];
    let mut magnitudes = vec![0.0f32; bins];

    for frame_index in 0..frame_count {
        let offset = frame_index * HOP_LENGTH;
        for bin in 0..bins {
            let mut real = 0.0f32;
            let mut imag = 0.0f32;
            for index in 0..N_FFT {
                let sample = padded.get(offset + index).copied().unwrap_or(0.0) * window[index];
                let angle = -2.0 * std::f32::consts::PI * bin as f32 * index as f32 / N_FFT as f32;
                real += sample * angle.cos();
                imag += sample * angle.sin();
            }
            magnitudes[bin] = real * real + imag * imag;
        }

        for mel in 0..N_MELS {
            let mut energy = 0.0f32;
            for bin in 0..bins {
                energy += magnitudes[bin] * filters[bin * N_MELS + mel];
            }
            log_mel[frame_index * N_MELS + mel] = (energy + 1.0e-7).ln();
        }
    }

    let lfr_frames = (frame_count + 5) / LFR_SKIP;
    let right_pad_len = lfr_frames * LFR_SKIP + LFR_STACK - frame_count;
    let padded_mel_frames = 3 + frame_count + right_pad_len;
    let mut padded_mel = vec![0.0f32; padded_mel_frames * N_MELS];

    for frame in 0..3 {
        padded_mel[frame * N_MELS..(frame + 1) * N_MELS].copy_from_slice(&log_mel[0..N_MELS]);
    }
    for frame in 0..frame_count {
        let source = frame * N_MELS;
        let target = (frame + 3) * N_MELS;
        padded_mel[target..target + N_MELS].copy_from_slice(&log_mel[source..source + N_MELS]);
    }
    let last_source = (frame_count - 1) * N_MELS;
    for frame in 0..right_pad_len {
        let target = (3 + frame_count + frame) * N_MELS;
        padded_mel[target..target + N_MELS]
            .copy_from_slice(&log_mel[last_source..last_source + N_MELS]);
    }

    let mut values = vec![0.0f32; lfr_frames * N_MELS * LFR_STACK];
    for frame in 0..lfr_frames {
        for stack in 0..LFR_STACK {
            let source_frame = stack + frame * LFR_SKIP;
            let source = source_frame * N_MELS;
            let target = frame * N_MELS * LFR_STACK + stack * N_MELS;
            values[target..target + N_MELS].copy_from_slice(&padded_mel[source..source + N_MELS]);
        }
    }

    SenseVoiceLfrFeatures {
        frames: lfr_frames,
        values,
    }
}

fn directml_identity_model_bytes() -> Vec<u8> {
    fn dim(value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        append_proto_int(&mut out, 1, value);
        out
    }

    fn tensor_shape() -> Vec<u8> {
        let mut out = Vec::new();
        append_proto_message(&mut out, 1, dim(1));
        out
    }

    fn tensor_type() -> Vec<u8> {
        let mut out = Vec::new();
        append_proto_int(&mut out, 1, 1);
        append_proto_message(&mut out, 2, tensor_shape());
        out
    }

    fn value_info(name: &str) -> Vec<u8> {
        let mut type_proto = Vec::new();
        append_proto_message(&mut type_proto, 1, tensor_type());

        let mut out = Vec::new();
        append_proto_string(&mut out, 1, name);
        append_proto_message(&mut out, 2, type_proto);
        out
    }

    let mut node = Vec::new();
    append_proto_string(&mut node, 1, "X");
    append_proto_string(&mut node, 2, "Y");
    append_proto_string(&mut node, 3, "IdentityNode");
    append_proto_string(&mut node, 4, "Identity");

    let mut graph = Vec::new();
    append_proto_message(&mut graph, 1, node);
    append_proto_string(&mut graph, 2, "HiVoicerDirectMlIdentityGraph");
    append_proto_message(&mut graph, 11, value_info("X"));
    append_proto_message(&mut graph, 12, value_info("Y"));

    let mut opset = Vec::new();
    append_proto_int(&mut opset, 2, 13);

    let mut model = Vec::new();
    append_proto_int(&mut model, 1, 7);
    append_proto_string(&mut model, 2, "hi-voicer-directml-probe");
    append_proto_message(&mut model, 7, graph);
    append_proto_message(&mut model, 8, opset);
    model
}

fn create_directml_identity_session() -> Result<DirectMlSessionProbeCliResult, String> {
    let model_path = std::env::temp_dir().join(format!(
        "hi-voicer-directml-identity-{}-{}.onnx",
        std::process::id(),
        unix_timestamp_millis()?
    ));
    fs::write(&model_path, directml_identity_model_bytes())
        .map_err(|error| format!("Failed to write DirectML identity probe model: {error}"))?;
    let result = create_directml_session_from_file(&model_path, "identity");
    let _ = fs::remove_file(&model_path);
    result
}

fn directml_session_builder() -> Result<ort::session::builder::SessionBuilder, String> {
    use ort::ep::{self, ExecutionProvider};

    let directml = ep::DirectML::default();
    if !directml.supported_by_platform() {
        return Err("DirectML execution provider is not supported on this platform.".to_string());
    }

    match directml.is_available() {
        Ok(true) => {}
        Ok(false) => {
            return Err(
                "ONNX Runtime was not built with DirectML execution provider support.".to_string(),
            );
        }
        Err(error) => {
            return Err(format!("DirectML availability check failed: {error}"));
        }
    }

    ort::session::Session::builder()
        .map_err(|error| format!("ONNX Runtime session builder failed: {error}"))?
        .with_no_environment_execution_providers()
        .map_err(|error| format!("Failed to isolate execution providers: {error}"))?
        .with_execution_providers([directml.build().error_on_failure()])
        .map_err(|error| format!("Failed to register DirectML execution provider: {error}"))?
        .with_intra_threads(1)
        .map_err(|error| format!("Failed to set ONNX Runtime thread count: {error}"))?
        .with_parallel_execution(false)
        .map_err(|error| format!("Failed to set ONNX Runtime execution mode: {error}"))
}

fn create_directml_session_from_file(
    model_path: &Path,
    label: &str,
) -> Result<DirectMlSessionProbeCliResult, String> {
    if !cfg!(windows) {
        return Err("DirectML is only available on Windows.".to_string());
    }

    let session = directml_session_builder()?
        .commit_from_file(model_path)
        .map_err(|error| format!("Failed to create DirectML {label} ONNX session: {error}"))?;
    let input_count = session.inputs().len();
    let output_count = session.outputs().len();
    let inputs = ort_outlet_summaries(session.inputs());
    let outputs = ort_outlet_summaries(session.outputs());
    drop(session);

    Ok(DirectMlSessionProbeCliResult {
        ok: true,
        message: format!(
            "DirectML {label} session created; inputs: {input_count}, outputs: {output_count}"
        ),
        model_inputs: inputs,
        model_outputs: outputs,
        onnx_runtime_build: Some(ort::info().to_string()),
    })
}

fn create_directml_sensevoice_encoder_warmup_session(
    model_path: &Path,
) -> Result<DirectMlSessionProbeCliResult, String> {
    if !cfg!(windows) {
        return Err("DirectML is only available on Windows.".to_string());
    }

    use half::f16;
    use ort::{inputs, value::Tensor};

    let mut session = directml_session_builder()?
        .commit_from_file(model_path)
        .map_err(|error| {
            format!("Failed to create DirectML split SenseVoice encoder session: {error}")
        })?;
    let inputs_summary = ort_outlet_summaries(session.inputs());
    let outputs_summary = ort_outlet_summaries(session.outputs());

    let fixed_len = 30usize * 17;
    let frontend_audio = vec![0.0f32; 16_000];
    let lfr_features = extract_sensevoice_lfr_features(&frontend_audio);
    let valid_frames = lfr_features.frames.min(fixed_len);
    let mut padded_features = vec![f16::from_f32(0.0); fixed_len * 560];
    for frame in 0..valid_frames {
        for value_index in 0..560 {
            padded_features[frame * 560 + value_index] =
                f16::from_f32(lfr_features.values[frame * 560 + value_index]);
        }
    }
    if valid_frames > 0 {
        let last_source = (valid_frames - 1) * 560;
        for frame in valid_frames..fixed_len {
            for value_index in 0..560 {
                padded_features[frame * 560 + value_index] =
                    f16::from_f32(lfr_features.values[last_source + value_index]);
            }
        }
    }
    let mut mask_values = vec![f16::from_f32(0.0); fixed_len];
    for value in mask_values.iter_mut().take(valid_frames) {
        *value = f16::from_f32(1.0);
    }

    let speech_feat = Tensor::<f16>::from_array(([1usize, fixed_len, 560], padded_features))
        .map_err(|error| format!("Failed to create encoder speech_feat tensor: {error}"))?;
    let mask = Tensor::<f16>::from_array(([1usize, fixed_len], mask_values))
        .map_err(|error| format!("Failed to create encoder mask tensor: {error}"))?;
    let prompt_ids = Tensor::<i64>::from_array(([1usize, 4], vec![0i64, 1, 2, 14]))
        .map_err(|error| format!("Failed to create encoder prompt_ids tensor: {error}"))?;

    let outputs = session
        .run(inputs![
            "speech_feat" => speech_feat,
            "mask" => mask,
            "prompt_ids" => prompt_ids,
        ])
        .map_err(|error| format!("DirectML split SenseVoice encoder warmup failed: {error}"))?;
    let output_count = outputs.len();
    drop(outputs);
    drop(session);

    Ok(DirectMlSessionProbeCliResult {
        ok: true,
        message: format!(
            "DirectML split SenseVoice encoder frontend warmup completed; LFR frames: {valid_frames}, outputs: {output_count}"
        ),
        model_inputs: inputs_summary,
        model_outputs: outputs_summary,
        onnx_runtime_build: Some(ort::info().to_string()),
    })
}

fn create_directml_sensevoice_ctc_warmup_session(
    model_path: &Path,
) -> Result<DirectMlSessionProbeCliResult, String> {
    if !cfg!(windows) {
        return Err("DirectML is only available on Windows.".to_string());
    }

    use half::f16;
    use ort::{inputs, value::Tensor};

    let mut session = directml_session_builder()?
        .commit_from_file(model_path)
        .map_err(|error| {
            format!("Failed to create DirectML split SenseVoice CTC session: {error}")
        })?;
    let inputs_summary = ort_outlet_summaries(session.inputs());
    let outputs_summary = ort_outlet_summaries(session.outputs());

    let fixed_len = 30usize * 17 + 4;
    let enc_out = Tensor::<f16>::from_array((
        [1usize, fixed_len, 512],
        vec![f16::from_f32(0.0); fixed_len * 512],
    ))
    .map_err(|error| format!("Failed to create CTC enc_out tensor: {error}"))?;

    let outputs = session
        .run(inputs!["enc_out" => enc_out])
        .map_err(|error| format!("DirectML split SenseVoice CTC warmup failed: {error}"))?;
    let output_count = outputs.len();
    drop(outputs);
    drop(session);

    Ok(DirectMlSessionProbeCliResult {
        ok: true,
        message: format!("DirectML split SenseVoice CTC warmup completed; outputs: {output_count}"),
        model_inputs: inputs_summary,
        model_outputs: outputs_summary,
        onnx_runtime_build: Some(ort::info().to_string()),
    })
}

fn create_directml_sensevoice_chain_smoke_session(
    encoder_path: &Path,
    ctc_path: &Path,
) -> Result<DirectMlSessionProbeCliResult, String> {
    if !cfg!(windows) {
        return Err("DirectML is only available on Windows.".to_string());
    }

    use half::f16;
    use ort::{inputs, value::Tensor};

    let mut encoder_session = directml_session_builder()?
        .commit_from_file(encoder_path)
        .map_err(|error| {
            format!("Failed to create DirectML split SenseVoice encoder session: {error}")
        })?;
    let encoder_inputs_summary = ort_outlet_summaries(encoder_session.inputs());
    let encoder_outputs_summary = ort_outlet_summaries(encoder_session.outputs());

    let fixed_len = 30usize * 17;
    let frontend_audio = vec![0.0f32; 16_000];
    let lfr_features = extract_sensevoice_lfr_features(&frontend_audio);
    let valid_frames = lfr_features.frames.min(fixed_len);
    let mut padded_features = vec![f16::from_f32(0.0); fixed_len * 560];
    for frame in 0..valid_frames {
        for value_index in 0..560 {
            padded_features[frame * 560 + value_index] =
                f16::from_f32(lfr_features.values[frame * 560 + value_index]);
        }
    }
    if valid_frames > 0 {
        let last_source = (valid_frames - 1) * 560;
        for frame in valid_frames..fixed_len {
            for value_index in 0..560 {
                padded_features[frame * 560 + value_index] =
                    f16::from_f32(lfr_features.values[last_source + value_index]);
            }
        }
    }
    let mut mask_values = vec![f16::from_f32(0.0); fixed_len];
    for value in mask_values.iter_mut().take(valid_frames) {
        *value = f16::from_f32(1.0);
    }

    let speech_feat = Tensor::<f16>::from_array(([1usize, fixed_len, 560], padded_features))
        .map_err(|error| format!("Failed to create encoder speech_feat tensor: {error}"))?;
    let mask = Tensor::<f16>::from_array(([1usize, fixed_len], mask_values))
        .map_err(|error| format!("Failed to create encoder mask tensor: {error}"))?;
    let prompt_ids = Tensor::<i64>::from_array(([1usize, 4], vec![0i64, 1, 2, 14]))
        .map_err(|error| format!("Failed to create encoder prompt_ids tensor: {error}"))?;

    let encoder_outputs = encoder_session
        .run(inputs![
            "speech_feat" => speech_feat,
            "mask" => mask,
            "prompt_ids" => prompt_ids,
        ])
        .map_err(|error| format!("DirectML split SenseVoice encoder run failed: {error}"))?;
    let (encoder_shape, encoder_data) = encoder_outputs[0]
        .try_extract_tensor::<f16>()
        .map_err(|error| format!("Failed to extract encoder output tensor: {error}"))?;
    let encoder_shape_usize = encoder_shape
        .iter()
        .map(|dim| usize::try_from(*dim).map_err(|_| format!("Invalid encoder output dim: {dim}")))
        .collect::<Result<Vec<_>, _>>()?;
    let encoder_data = encoder_data.to_vec();
    drop(encoder_outputs);
    drop(encoder_session);

    let mut ctc_session = directml_session_builder()?
        .commit_from_file(ctc_path)
        .map_err(|error| {
            format!("Failed to create DirectML split SenseVoice CTC session: {error}")
        })?;
    let ctc_inputs_summary = ort_outlet_summaries(ctc_session.inputs());
    let ctc_outputs_summary = ort_outlet_summaries(ctc_session.outputs());
    let enc_out = Tensor::<f16>::from_array((encoder_shape_usize, encoder_data))
        .map_err(|error| format!("Failed to create CTC enc_out tensor: {error}"))?;
    let ctc_outputs = ctc_session
        .run(inputs!["enc_out" => enc_out])
        .map_err(|error| format!("DirectML split SenseVoice CTC run failed: {error}"))?;
    let (_indices_shape, topk_indices) = ctc_outputs[1]
        .try_extract_tensor::<i32>()
        .map_err(|error| format!("Failed to extract CTC topk_indices tensor: {error}"))?;
    let token_preview = topk_indices
        .iter()
        .step_by(100)
        .skip(4)
        .take(12)
        .map(|token| token.to_string())
        .collect::<Vec<_>>();
    drop(ctc_outputs);
    drop(ctc_session);

    let mut model_inputs = Vec::new();
    model_inputs.extend(
        encoder_inputs_summary
            .iter()
            .map(|item| format!("encoder {item}")),
    );
    model_inputs.extend(ctc_inputs_summary.iter().map(|item| format!("ctc {item}")));

    let mut model_outputs = Vec::new();
    model_outputs.extend(
        encoder_outputs_summary
            .iter()
            .map(|item| format!("encoder {item}")),
    );
    model_outputs.extend(ctc_outputs_summary.iter().map(|item| format!("ctc {item}")));

    Ok(DirectMlSessionProbeCliResult {
        ok: true,
        message: format!(
            "DirectML split SenseVoice encoder->CTC smoke completed; LFR frames: {valid_frames}; top1 token ids: {}",
            token_preview.join(", ")
        ),
        model_inputs,
        model_outputs,
        onnx_runtime_build: Some(ort::info().to_string()),
    })
}

fn create_directml_session_in_child_with_args(
    mode: &str,
    model_paths: &[&Path],
    timeout: Duration,
    description: &str,
) -> Result<DirectMlSessionProbeCliResult, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = Command::new(executable);
    command.arg(mode);
    for model_path in model_paths {
        command.arg(model_path);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    suppress_command_window(&mut command);

    let output = run_command_with_timeout(&mut command, timeout, description)
        .map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        return serde_json::from_str::<DirectMlSessionProbeCliResult>(&stdout).map_err(|error| {
            format!("DirectML child probe returned invalid JSON: {error}; stdout: {stdout}")
        });
    }

    let code = output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated by signal or native exception".to_string());
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(format!(
        "{description} failed with exit code {code}: {detail}"
    ))
}

fn create_directml_session_in_child(
    mode: &str,
    model_path: Option<&Path>,
    timeout: Duration,
    description: &str,
) -> Result<DirectMlSessionProbeCliResult, String> {
    match model_path {
        Some(model_path) => {
            create_directml_session_in_child_with_args(mode, &[model_path], timeout, description)
        }
        None => create_directml_session_in_child_with_args(mode, &[], timeout, description),
    }
}

fn create_directml_provider_session_in_child() -> Result<DirectMlSessionProbeCliResult, String> {
    create_directml_session_in_child(
        "--hi-voicer-directml-provider-probe",
        None,
        Duration::from_secs(15),
        "DirectML provider identity child probe",
    )
}

fn create_directml_sensevoice_session_in_child(
    model_path: &Path,
) -> Result<DirectMlSessionProbeCliResult, String> {
    create_directml_session_in_child(
        "--hi-voicer-directml-probe",
        Some(model_path),
        Duration::from_secs(30),
        "DirectML SenseVoice session child probe",
    )
}

pub fn run_cli_mode() -> bool {
    let mut args = std::env::args_os();
    let _program = args.next();
    let Some(mode) = args.next() else {
        return false;
    };
    let probe_result = match mode.to_string_lossy().as_ref() {
        "--hi-voicer-directml-provider-probe" => {
            suppress_windows_fault_dialogs();
            create_directml_identity_session()
        }
        "--hi-voicer-directml-probe" => {
            suppress_windows_fault_dialogs();
            let Some(model_path) = args.next() else {
                eprintln!("missing model path");
                std::process::exit(2);
            };
            create_directml_session_from_file(&PathBuf::from(model_path), "SenseVoice")
        }
        "--hi-voicer-directml-sensevoice-encoder-warmup" => {
            suppress_windows_fault_dialogs();
            let Some(model_path) = args.next() else {
                eprintln!("missing encoder path");
                std::process::exit(2);
            };
            create_directml_sensevoice_encoder_warmup_session(&PathBuf::from(model_path))
        }
        "--hi-voicer-directml-sensevoice-ctc-warmup" => {
            suppress_windows_fault_dialogs();
            let Some(model_path) = args.next() else {
                eprintln!("missing CTC path");
                std::process::exit(2);
            };
            create_directml_sensevoice_ctc_warmup_session(&PathBuf::from(model_path))
        }
        "--hi-voicer-directml-sensevoice-chain-smoke" => {
            suppress_windows_fault_dialogs();
            let Some(encoder_path) = args.next() else {
                eprintln!("missing encoder path");
                std::process::exit(2);
            };
            let Some(ctc_path) = args.next() else {
                eprintln!("missing CTC path");
                std::process::exit(2);
            };
            create_directml_sensevoice_chain_smoke_session(
                &PathBuf::from(encoder_path),
                &PathBuf::from(ctc_path),
            )
        }
        _ => return false,
    };

    match probe_result {
        Ok(result) => match serde_json::to_string(&result) {
            Ok(raw) => println!("{raw}"),
            Err(error) => {
                eprintln!("failed to serialize DirectML probe result: {error}");
                std::process::exit(3);
            }
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
    true
}

#[derive(Debug, Clone)]
struct DirectMlSplitSenseVoiceCandidate {
    model_dir: PathBuf,
    encoder: PathBuf,
    ctc: PathBuf,
    tokenizer: PathBuf,
}

fn directml_sensevoice_ready(model_path: &Path) -> bool {
    let Ok(engine) = read_sherpa_engine(model_path) else {
        return false;
    };
    engine.model_id == "sensevoice-small"
        && model_path.join("engine.json").exists()
        && model_path.join("model.int8.onnx").exists()
        && model_path.join("tokens.txt").exists()
}

fn split_sensevoice_candidate_for_dir(model_dir: &Path) -> DirectMlSplitSenseVoiceCandidate {
    DirectMlSplitSenseVoiceCandidate {
        model_dir: model_dir.to_path_buf(),
        encoder: model_dir.join("SenseVoice-Encoder.fp16.onnx"),
        ctc: model_dir.join("SenseVoice-CTC.fp16.onnx"),
        tokenizer: model_dir.join("tokenizer.bpe.model"),
    }
}

fn split_sensevoice_missing_files(candidate: &DirectMlSplitSenseVoiceCandidate) -> Vec<String> {
    let mut missing = Vec::new();
    if !candidate.model_dir.exists() {
        missing.push("split model directory".to_string());
    }
    for (label, path) in [
        ("SenseVoice-Encoder.fp16.onnx", &candidate.encoder),
        ("SenseVoice-CTC.fp16.onnx", &candidate.ctc),
        ("tokenizer.bpe.model", &candidate.tokenizer),
    ] {
        if !path.exists() {
            missing.push(label.to_string());
        }
    }
    missing
}

fn directml_split_sensevoice_candidate_dirs(
    requested_model_dir: &str,
    app_models_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let requested = PathBuf::from(requested_model_dir);
    let mut candidates = Vec::new();
    candidates.push(requested.clone());
    candidates.push(requested.join("sensevoice-directml"));
    candidates.push(requested.join("Sensevoice-Small-ONNX"));
    candidates.push(
        requested
            .join("SenseVoice-Small")
            .join("Sensevoice-Small-ONNX"),
    );
    if let Some(parent) = requested.parent() {
        candidates.push(parent.join("sensevoice-directml"));
        candidates.push(parent.join("Sensevoice-Small-ONNX"));
        candidates.push(
            parent
                .join("SenseVoice-Small")
                .join("Sensevoice-Small-ONNX"),
        );
    }
    if let Some(models_dir) = app_models_dir {
        candidates.push(models_dir.join("sensevoice-directml"));
        candidates.push(models_dir.join("Sensevoice-Small-ONNX"));
        candidates.push(
            models_dir
                .join("SenseVoice-Small")
                .join("Sensevoice-Small-ONNX"),
        );
    }

    let mut unique = Vec::new();
    for candidate in candidates {
        if !unique
            .iter()
            .any(|existing: &PathBuf| existing == &candidate)
        {
            unique.push(candidate);
        }
    }
    unique
}

fn select_directml_split_sensevoice_candidate(
    requested_model_dir: &str,
    app_models_dir: Option<PathBuf>,
) -> DirectMlSplitSenseVoiceCandidate {
    let candidates = directml_split_sensevoice_candidate_dirs(requested_model_dir, app_models_dir);
    candidates
        .iter()
        .map(|path| split_sensevoice_candidate_for_dir(path))
        .find(|candidate| split_sensevoice_missing_files(candidate).is_empty())
        .unwrap_or_else(|| {
            candidates
                .first()
                .map(|path| split_sensevoice_candidate_for_dir(path))
                .unwrap_or_else(|| {
                    split_sensevoice_candidate_for_dir(&PathBuf::from(requested_model_dir))
                })
        })
}

fn create_directml_split_sensevoice_sessions_in_child(
    candidate: &DirectMlSplitSenseVoiceCandidate,
) -> Result<DirectMlSessionProbeCliResult, String> {
    create_directml_session_in_child_with_args(
        "--hi-voicer-directml-sensevoice-chain-smoke",
        &[candidate.encoder.as_path(), candidate.ctc.as_path()],
        Duration::from_secs(90),
        "DirectML SenseVoice split encoder-to-CTC child probe",
    )
}

fn directml_probe_candidate_dirs(
    requested_model_dir: &str,
    app_models_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let requested = PathBuf::from(requested_model_dir);
    let mut candidates = Vec::new();
    candidates.push(requested.clone());
    candidates.push(requested.join("sensevoice-small"));
    if let Some(parent) = requested.parent() {
        candidates.push(parent.join("sensevoice-small"));
    }
    if let Some(models_dir) = app_models_dir {
        candidates.push(models_dir.join("sensevoice-small"));
    }

    let mut unique = Vec::new();
    for candidate in candidates {
        if !unique
            .iter()
            .any(|existing: &PathBuf| existing == &candidate)
        {
            unique.push(candidate);
        }
    }
    unique
}

fn select_directml_probe_model_dir(
    requested_model_dir: &str,
    app_models_dir: Option<PathBuf>,
) -> PathBuf {
    let candidates = directml_probe_candidate_dirs(requested_model_dir, app_models_dir);
    candidates
        .iter()
        .find(|candidate| directml_sensevoice_ready(candidate))
        .cloned()
        .unwrap_or_else(|| PathBuf::from(requested_model_dir))
}

fn directml_probe_for_model_dir(
    model_dir: &str,
    app_models_dir: Option<PathBuf>,
) -> DirectMlProbeResult {
    let started_at = Instant::now();
    let model_path = select_directml_probe_model_dir(model_dir, app_models_dir.clone());
    let split_candidate = select_directml_split_sensevoice_candidate(model_dir, app_models_dir);
    let engine = read_sherpa_engine(&model_path).ok();
    let mut missing_files = Vec::new();

    let model_id = engine.as_ref().map(|engine| engine.model_id.clone());
    let model_name = engine.as_ref().map(|engine| engine.model_name.clone());
    let is_sensevoice = model_id.as_deref() == Some("sensevoice-small");

    if !model_path.exists() {
        missing_files.push("model directory".to_string());
    }
    for required in ["engine.json", "model.int8.onnx", "tokens.txt"] {
        if !model_path.join(required).exists() {
            missing_files.push(required.to_string());
        }
    }

    let adapters = query_directml_candidate_adapters();
    let directml_candidate = adapters.iter().any(is_directml_candidate_adapter);
    let model_ready = is_sensevoice && missing_files.is_empty();
    let provider_session_result = if directml_candidate {
        create_directml_provider_session_in_child()
    } else {
        Err(
            "DirectML provider check skipped because no usable GPU adapter was detected."
                .to_string(),
        )
    };
    let provider_session_ready = provider_session_result
        .as_ref()
        .map(|result| result.ok)
        .unwrap_or(false);
    let provider_session_error = provider_session_result.as_ref().err().cloned();

    let split_model_missing_files = split_sensevoice_missing_files(&split_candidate);
    let split_model_ready = split_model_missing_files.is_empty();
    let split_model_session_result = if split_model_ready && provider_session_ready {
        create_directml_split_sensevoice_sessions_in_child(&split_candidate)
    } else if !provider_session_ready {
        Err(
            "Split SenseVoice session check skipped because the DirectML provider probe failed."
                .to_string(),
        )
    } else {
        Err("Split SenseVoice files are incomplete.".to_string())
    };
    let split_model_session_ready = split_model_session_result
        .as_ref()
        .map(|result| result.ok)
        .unwrap_or(false);
    let split_model_session_error = split_model_session_result.as_ref().err().cloned();
    let split_model_inputs = split_model_session_result
        .as_ref()
        .map(|result| result.model_inputs.clone())
        .unwrap_or_default();
    let split_model_outputs = split_model_session_result
        .as_ref()
        .map(|result| result.model_outputs.clone())
        .unwrap_or_default();

    let directml_session_result = if split_model_session_ready {
        split_model_session_result.clone()
    } else if model_ready && provider_session_ready {
        create_directml_sensevoice_session_in_child(&model_path.join("model.int8.onnx"))
    } else if !provider_session_ready {
        Err(
            "SenseVoice session check skipped because the DirectML provider probe failed."
                .to_string(),
        )
    } else {
        Err("DirectML session check skipped because prerequisites are incomplete.".to_string())
    };
    let directml_session_ready = directml_session_result
        .as_ref()
        .map(|result| result.ok)
        .unwrap_or(false);
    let directml_session_error = directml_session_result.as_ref().err().cloned();
    let onnx_runtime_build = directml_session_result
        .as_ref()
        .ok()
        .or_else(|| split_model_session_result.as_ref().ok())
        .or_else(|| provider_session_result.as_ref().ok())
        .and_then(|result| result.onnx_runtime_build.clone());
    let model_inputs = directml_session_result
        .as_ref()
        .map(|result| result.model_inputs.clone())
        .unwrap_or_default();
    let model_outputs = directml_session_result
        .as_ref()
        .map(|result| result.model_outputs.clone())
        .unwrap_or_default();
    let message = if let Ok(session_result) = &directml_session_result {
        session_result.message.clone()
    } else if let Err(provider_error) = &provider_session_result {
        provider_error.clone()
    } else if !is_sensevoice {
        "DirectML PoC currently only targets SenseVoiceSmall.".to_string()
    } else if !missing_files.is_empty() {
        format!(
            "SenseVoiceSmall files are incomplete: {}",
            missing_files.join(", ")
        )
    } else if !directml_candidate {
        "No usable Windows GPU adapter was detected by the DirectML probe.".to_string()
    } else {
        directml_session_error
            .clone()
            .unwrap_or_else(|| "DirectML SenseVoice session check failed.".to_string())
    };
    let next_step = if split_model_session_ready {
        "Split SenseVoice encoder/CTC sessions work with DirectML; next implement fixed-shape audio feature input and CTC decoding behind an experimental engine.".to_string()
    } else if directml_session_ready {
        "Add the DirectML audio feature-extraction and decoder path behind an experimental toggle."
            .to_string()
    } else if provider_session_ready && split_model_ready {
        "DirectML provider works, but split SenseVoice session creation failed; inspect unsupported operators or try another ONNX Runtime version.".to_string()
    } else if provider_session_ready && model_ready {
        "DirectML provider works, but Sherpa SenseVoiceSmall session creation failed; use a DirectML-friendly split model before enabling transcription.".to_string()
    } else if provider_session_ready {
        "DirectML provider works; place split SenseVoice files under models\\sensevoice-directml or CapsWriter-style models\\SenseVoice-Small\\Sensevoice-Small-ONNX.".to_string()
    } else {
        "Keep using the stable CPU/Sherpa path; DirectML provider session is not reliable on this machine.".to_string()
    };

    DirectMlProbeResult {
        directml_candidate,
        provider_session_ready,
        provider_session_error,
        split_model_ready,
        split_model_dir: Some(split_candidate.model_dir.to_string_lossy().to_string()),
        split_model_missing_files,
        split_model_session_ready,
        split_model_session_error,
        split_model_inputs,
        split_model_outputs,
        model_ready,
        directml_session_ready,
        directml_session_error,
        onnx_runtime_build,
        model_inputs,
        model_outputs,
        model_id,
        model_name,
        model_dir: model_path.to_string_lossy().to_string(),
        missing_files,
        adapters,
        elapsed_ms: started_at.elapsed().as_millis(),
        message,
        next_step,
    }
}

fn run_acceleration_smoke_test_inner(
    app: AppHandle,
    request: AccelerationSmokeTestRequest,
) -> Result<AccelerationSmokeTestResult, String> {
    let started_at = Instant::now();
    let requested_mode = "cpu";
    let model_dir = PathBuf::from(&request.model_dir);
    let engine = read_sherpa_engine(&model_dir)?;
    let cpu_executable = PathBuf::from(&engine.executable);
    if !cpu_executable.exists() {
        return Err(
            "Sherpa-ONNX executable does not exist. Configure the model again.".to_string(),
        );
    }
    let _cpu_cli_executable = sherpa_cli_executable_for_engine(&engine, &cpu_executable);

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
            let message = if let Some(reason) = fallback_reason {
                format!("CPU smoke test completed: {reason}")
            } else {
                "CPU smoke test completed; silent audio does not need recognized text.".to_string()
            };
            let fallback_used = false;
            (runtime.mode, fallback_used, text, message)
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
    segments: &[TranscriptSegment],
) -> Result<(String, Vec<String>, Vec<TranscriptionOutputFile>), String> {
    fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;

    let stem = source_audio_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("transcript");
    let suffix = unix_timestamp_millis()?;
    let plain_path = output_dir.join(format!("{stem}-{suffix}.txt"));
    let timeline_path = output_dir.join(format!("{stem}-{suffix}-timeline.txt"));
    let timeline_txt_path = output_dir.join(format!("{stem}-{suffix}-timeline-txt.txt"));
    let srt_path = output_dir.join(format!("{stem}-{suffix}.srt"));
    let resolve_markers_path = output_dir.join(format!("{stem}-{suffix}.markers.edl"));
    let _ = sherpa_audio_path;

    fs::write(&plain_path, text).map_err(|error| error.to_string())?;
    fs::write(&timeline_path, timeline_text_from_segments(segments))
        .map_err(|error| error.to_string())?;
    write_utf8_sig(
        &timeline_txt_path,
        &timeline_txt_text_from_segments(segments),
    )?;
    fs::write(&srt_path, srt_text_from_segments(segments)).map_err(|error| error.to_string())?;
    write_utf8_sig(
        &resolve_markers_path,
        &resolve_marker_edl_from_segments(stem, segments),
    )?;

    let files = vec![
        TranscriptionOutputFile {
            format: "plainText".to_string(),
            label: "Plain text".to_string(),
            path: plain_path.to_string_lossy().to_string(),
        },
        TranscriptionOutputFile {
            format: "timelineText".to_string(),
            label: "Timeline TXT".to_string(),
            path: timeline_path.to_string_lossy().to_string(),
        },
        TranscriptionOutputFile {
            format: "timelineTxt".to_string(),
            label: "Timeline TXT one-line (UTF-8 BOM)".to_string(),
            path: timeline_txt_path.to_string_lossy().to_string(),
        },
        TranscriptionOutputFile {
            format: "srt".to_string(),
            label: "SRT subtitles".to_string(),
            path: srt_path.to_string_lossy().to_string(),
        },
        TranscriptionOutputFile {
            format: "resolveMarkers".to_string(),
            label: "DaVinci Resolve markers".to_string(),
            path: resolve_markers_path.to_string_lossy().to_string(),
        },
    ];
    let output_paths = files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let primary = if preferred_format == "timelineText" {
        timeline_path
    } else if preferred_format == "timelineTxt" {
        timeline_txt_path
    } else if preferred_format == "srt" {
        srt_path
    } else if preferred_format == "resolveMarkers" {
        resolve_markers_path
    } else {
        plain_path
    };

    Ok((primary.to_string_lossy().to_string(), output_paths, files))
}

fn persist_review_audio(
    app: &AppHandle,
    source_audio_path: &Path,
    sherpa_audio_path: &Path,
) -> Result<PathBuf, String> {
    let output_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?
        .join("review-audio");
    fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;
    let stem = source_audio_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    let output_path = output_dir.join(format!("{stem}-review-{}.wav", unix_timestamp_millis()?));
    fs::copy(sherpa_audio_path, &output_path).map_err(|error| error.to_string())?;
    Ok(output_path)
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
        "Converting to 16kHz mono WAV".to_string(),
        0,
        0,
    );
    let audio_path = PathBuf::from(&request.audio_path);
    if !audio_path.exists() {
        return Err("Audio file does not exist.".to_string());
    }
    let sherpa_audio_path = media_to_sherpa_wav(&app, &audio_path)?;
    emit_transcription_progress(
        &app,
        request.task_id.as_deref(),
        started_at,
        "transcoding",
        12,
        "Audio transcoding complete; preparing recognition".to_string(),
        0,
        0,
    );

    let model_dir = PathBuf::from(&request.model_dir);
    let engine = read_sherpa_engine(&model_dir)?;

    let cpu_executable = PathBuf::from(&engine.executable);
    if !cpu_executable.exists() {
        return Err(
            "Sherpa-ONNX executable does not exist. Configure the model again.".to_string(),
        );
    }
    let _cpu_cli_executable = sherpa_cli_executable_for_engine(&engine, &cpu_executable);

    let runtime = resolve_sherpa_runtime(&app, &engine, &request.acceleration_mode);
    emit_transcription_progress(
        &app,
        request.task_id.as_deref(),
        started_at,
        "transcribing",
        19,
        format!("Actual acceleration path: {}", runtime.mode.to_uppercase()),
        0,
        0,
    );
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

    let chunk_result = transcribe_sherpa_wav(
        &app,
        &engine,
        &runtime.executable,
        &sherpa_audio_path,
        runtime_performance,
        request.task_id.as_deref(),
        started_at,
        &runtime.mode,
    );
    let raw_chunks = match chunk_result {
        Ok(chunks) => chunks,
        Err(error) => return Err(error),
    };
    let transcript_chunks = raw_chunks
        .into_iter()
        .map(|chunk| TranscriptTextChunk {
            text: apply_hotwords(&chunk.text, &request.hotwords),
            start: chunk.start,
            end: chunk.end,
        })
        .collect::<Vec<_>>();
    let text = transcript_text_from_chunks(&transcript_chunks);

    if text.is_empty() {
        return Err("Sherpa ran, but no transcription text was parsed.".to_string());
    }

    let duration = wav_duration_seconds(&sherpa_audio_path)?.max(1.0);
    let review_audio_path = persist_review_audio(&app, &audio_path, &sherpa_audio_path)?;
    let segments = if transcript_chunks.is_empty() {
        build_transcript_segments(&text, duration, &review_audio_path)
    } else {
        build_transcript_segments_from_chunks(&transcript_chunks, &review_audio_path)
    };

    let (output_path, output_paths, output_files) = if request.save_output {
        emit_transcription_progress(
            &app,
            request.task_id.as_deref(),
            started_at,
            "exporting",
            95,
            "Generating export files".to_string(),
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
            &segments,
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
        "杞綍瀹屾垚".to_string(),
        0,
        0,
    );

    Ok(TranscribeFileResult {
        text,
        output_path,
        output_paths,
        output_files,
        segments,
        timeline_kind: "estimated".to_string(),
        source_audio_path: review_audio_path.to_string_lossy().to_string(),
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

fn build_recording_track_for_device(
    app: &AppHandle,
    device: cpal::Device,
    supported_config: cpal::SupportedStreamConfig,
    _role: &str,
    file_prefix: &str,
) -> Result<RecordingTrack, String> {
    let sample_format = supported_config.sample_format();
    let stream_config: cpal::StreamConfig = supported_config.into();

    let recordings_dir = app_recordings_dir(app)?;
    let path = recordings_dir.join(format!("{file_prefix}-{}.wav", unix_timestamp_millis()?));

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
        other => return Err(format!("Unsupported recording sample format: {other:?}")),
    }
    .map_err(|error| error.to_string())?;

    stream.play().map_err(|error| error.to_string())?;
    Ok(RecordingTrack {
        stream,
        writer,
        path,
    })
}

fn start_microphone_recording_track(app: &AppHandle) -> Result<RecordingTrack, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "No available microphone input device was found.".to_string())?;
    let supported_config = device
        .default_input_config()
        .map_err(|error| error.to_string())?;
    build_recording_track_for_device(app, device, supported_config, "microphone", "voice")
}

fn start_system_recording_track(app: &AppHandle) -> Result<RecordingTrack, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "No available system output device was found.".to_string())?;
    let supported_config = device
        .default_output_config()
        .map_err(|error| error.to_string())?;
    build_recording_track_for_device(app, device, supported_config, "system", "system").map_err(
        |error| {
            format!(
                "System audio loopback capture failed. Check Windows output device, exclusive-mode settings, and app audio permissions. Details: {error}"
            )
        },
    )
}

fn audio_config_detail(config: &cpal::SupportedStreamConfig) -> String {
    format!(
        "{} Hz / {} channel(s) / {:?}",
        config.sample_rate().0,
        config.channels(),
        config.sample_format()
    )
}

fn system_audio_diagnostic_detail(config: &cpal::SupportedStreamConfig) -> String {
    format!(
        "{}. Output device detected; system-audio recording still depends on WASAPI loopback support and will be verified when recording starts.",
        audio_config_detail(config)
    )
}

fn native_audio_diagnostics(app: &AppHandle) -> NativeAudioDiagnostics {
    let host = cpal::default_host();

    let (microphone_available, microphone_name, microphone_detail) =
        match host.default_input_device() {
            Some(device) => {
                let name = device
                    .name()
                    .unwrap_or_else(|_| "Default microphone".to_string());
                match device.default_input_config() {
                    Ok(config) => (true, Some(name), Some(audio_config_detail(&config))),
                    Err(error) => (false, Some(name), Some(error.to_string())),
                }
            }
            None => (
                false,
                None,
                Some("No default microphone input device was found.".to_string()),
            ),
        };

    let (system_audio_available, system_audio_name, system_audio_detail) =
        match host.default_output_device() {
            Some(device) => {
                let name = device
                    .name()
                    .unwrap_or_else(|_| "Default speaker".to_string());
                match device.default_output_config() {
                    Ok(config) => (
                        true,
                        Some(name),
                        Some(system_audio_diagnostic_detail(&config)),
                    ),
                    Err(error) => (false, Some(name), Some(error.to_string())),
                }
            }
            None => (
                false,
                None,
                Some("No default system output device was found.".to_string()),
            ),
        };

    let ffmpeg_roots = ffmpeg_runtime_search_roots_for_app(app).unwrap_or_default();
    let (ffmpeg_installed, ffmpeg_path, ffmpeg_detail) = match installed_ffmpeg_runtime(app) {
        Ok(Some(path)) => (
            true,
            Some(path.to_string_lossy().to_string()),
            Some("Local ffmpeg runtime is installed.".to_string()),
        ),
        Ok(None) => (false, None, Some(ffmpeg_missing_detail(&ffmpeg_roots))),
        Err(error) => (false, None, Some(error)),
    };

    let message = if microphone_available && system_audio_available && ffmpeg_installed {
        "Native audio environment looks ready.".to_string()
    } else {
        "Native audio environment needs attention before all recording and processing modes are available.".to_string()
    };

    NativeAudioDiagnostics {
        microphone_available,
        microphone_name,
        microphone_detail,
        system_audio_available,
        system_audio_name,
        system_audio_detail,
        ffmpeg_installed,
        ffmpeg_path,
        ffmpeg_detail,
        message,
    }
}

fn start_microphone_and_system_recording_tracks(
    app: &AppHandle,
) -> Result<Vec<RecordingTrack>, String> {
    let microphone = start_microphone_recording_track(app)?;
    match start_system_recording_track(app) {
        Ok(system) => Ok(vec![microphone, system]),
        Err(error) => {
            let microphone_path = microphone.path.clone();
            let _ = stop_recording_track(microphone);
            let _ = fs::remove_file(microphone_path);
            Err(error)
        }
    }
}
fn start_recording_session(app: &AppHandle, source: &str) -> Result<RecordingSession, String> {
    let tracks = match source {
        "system" => vec![start_system_recording_track(app)?],
        "microphoneAndSystem" => start_microphone_and_system_recording_tracks(app)?,
        _ => vec![start_microphone_recording_track(app)?],
    };
    let path = tracks
        .first()
        .map(|track| track.path.clone())
        .ok_or_else(|| "No recording tracks were started.".to_string())?;
    Ok(RecordingSession {
        source: source.to_string(),
        tracks,
        path,
        paste_target_window: capture_paste_target_window(),
    })
}

fn stop_recording_track(track: RecordingTrack) -> Result<PathBuf, String> {
    let RecordingTrack {
        stream,
        writer,
        path,
        ..
    } = track;
    drop(stream);

    let mut guard = writer.lock().map_err(|error| error.to_string())?;
    if let Some(writer) = guard.take() {
        writer.finalize().map_err(|error| error.to_string())?;
    }

    Ok(path)
}

fn validate_recording_output_file(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!(
            "recording file was not created: {}",
            path.to_string_lossy()
        ));
    }

    let duration = wav_duration_seconds(path)
        .map_err(|error| format!("recording file is not readable: {error}"))?;
    if duration < MIN_RECORDING_SECONDS {
        return Err(format!(
            "recording is too short or empty ({duration:.3}s): {}",
            path.to_string_lossy()
        ));
    }

    Ok(())
}

fn validate_recording_output_files(paths: &[PathBuf]) -> Result<(), String> {
    let mut errors = Vec::new();
    for path in paths {
        if let Err(error) = validate_recording_output_file(path) {
            errors.push(error);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Recording did not contain usable audio. Try again and keep recording for a little longer. Details: {}",
            errors.join("; ")
        ))
    }
}

fn stop_recording_session(session: RecordingSession) -> Result<StoppedRecording, String> {
    let RecordingSession {
        source,
        tracks,
        path,
        ..
    } = session;
    let mut output_paths = Vec::new();

    for track in tracks {
        output_paths.push(stop_recording_track(track)?);
    }

    if let Err(error) = validate_recording_output_files(&output_paths) {
        for path in &output_paths {
            let _ = fs::remove_file(path);
        }
        return Err(error);
    }

    let primary_path = output_paths.first().cloned().unwrap_or(path);
    Ok(StoppedRecording {
        source,
        primary_path,
        output_paths,
    })
}

fn start_recording_from_runtime(app: &AppHandle, state: &RuntimeState) -> Result<String, String> {
    let mut recording = state.recording.lock().map_err(|error| error.to_string())?;
    if recording.is_some() {
        return Err("Recording is already in progress.".to_string());
    }
    let recording_source = {
        let settings = state.settings.lock().map_err(|error| error.to_string())?;
        settings.recording_source.clone()
    };

    let session = start_recording_session(app, &recording_source)?;
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

fn emit_recording_error(app: &AppHandle, message: impl Into<String>) {
    let _ = app.emit(
        "recording-error",
        RecordingErrorEvent {
            message: message.into(),
        },
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
            .ok_or_else(|| "There is no active recording.".to_string())?
    };
    let settings = {
        let settings = state.settings.lock().map_err(|error| error.to_string())?;
        settings.clone()
    };

    Ok((session, settings))
}

fn recording_mix_filter(input_count: usize) -> String {
    format!("amix=inputs={input_count}:duration=longest:dropout_transition=0")
}

fn mix_recording_tracks(app: &AppHandle, paths: &[PathBuf]) -> Result<PathBuf, String> {
    if paths.len() < 2 {
        return paths
            .first()
            .cloned()
            .ok_or_else(|| "No recording tracks were available to mix.".to_string());
    }

    let ffmpeg = resolve_ffmpeg_runtime(app)?;
    let recordings_dir = app_recordings_dir(app)?;
    let output_path = recordings_dir.join(format!("mixed-{}.wav", unix_timestamp_millis()?));

    let mut command = Command::new(ffmpeg);
    command.arg("-y");
    for path in paths {
        command.arg("-i").arg(path);
    }
    command
        .arg("-filter_complex")
        .arg(recording_mix_filter(paths.len()))
        .arg("-vn")
        .arg("-acodec")
        .arg("pcm_s16le")
        .arg("-ar")
        .arg("48000")
        .arg("-ac")
        .arg("1")
        .arg(&output_path);
    suppress_command_window(&mut command);

    let output = run_command_with_timeout(
        &mut command,
        Duration::from_secs(900),
        "recording track mix",
    )?;
    if output.status.success() {
        return Ok(output_path);
    }

    Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

fn prepare_recording_audio(
    app: &AppHandle,
    stopped: &mut StoppedRecording,
) -> Result<PathBuf, String> {
    if stopped.source == "microphoneAndSystem" && stopped.output_paths.len() >= 2 {
        let mixed_path = mix_recording_tracks(app, &stopped.output_paths)?;
        stopped.primary_path = mixed_path.clone();
        stopped.output_paths.push(mixed_path.clone());
        return Ok(mixed_path);
    }

    Ok(stopped.primary_path.clone())
}

fn finish_recording_with_settings(
    app: AppHandle,
    session: RecordingSession,
    settings: UserSettings,
    paste: bool,
) -> Result<TranscribeFileResult, String> {
    let paste_target_window = session.paste_target_window;
    let mut stopped = stop_recording_session(session)?;
    let audio_path = if settings.recording_mode == "audioOnly" {
        stopped.primary_path.clone()
    } else {
        prepare_recording_audio(&app, &mut stopped)?
    };
    if settings.recording_mode == "audioOnly" {
        let audio_path_text = audio_path.to_string_lossy().to_string();
        let output_paths = stopped
            .output_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        return Ok(TranscribeFileResult {
            text: format!("Recording saved: {audio_path_text}"),
            output_path: audio_path_text.clone(),
            output_paths,
            output_files: Vec::new(),
            segments: Vec::new(),
            timeline_kind: "estimated".to_string(),
            source_audio_path: audio_path_text,
        });
    }

    let model_dir = if settings.input_model_dir.trim().is_empty() {
        settings.model_dir.clone()
    } else {
        settings.input_model_dir.clone()
    };

    if model_dir.trim().is_empty() {
        return Err("Download and configure an offline model in Settings first.".to_string());
    }

    let result = transcribe_file_with_sherpa(
        app,
        TranscribeFileRequest {
            audio_path: audio_path.to_string_lossy().to_string(),
            model_dir,
            task_id: None,
            performance_mode: "stable".to_string(),
            acceleration_mode: settings.acceleration_mode,
            hotwords: settings.hotwords,
            output_format: settings.export_format,
            save_output: settings.save_recordings,
        },
    )?;

    if paste {
        paste_text_to_target_window(&result.text, paste_target_window)?;
    }

    if !settings.save_recordings {
        for path in stopped.output_paths {
            let _ = fs::remove_file(path);
        }
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
            Ok(Err(error)) => {
                eprintln!("global shortcut stop failed: {error}");
                emit_recording_error(&app, error);
            }
            Err(error) => {
                let message = format!("Recording stop task failed: {error}");
                eprintln!("{message}");
                emit_recording_error(&app, message);
            }
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
                emit_recording_error(app, error);
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
                    emit_recording_error(app, error);
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
    let show_item = MenuItem::with_id(
        app,
        "show",
        "\u{6253}\u{5f00} Hi-Voicer",
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", "\u{9000}\u{51fa}", true, None::<&str>)?;
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
    Ok(settings)
}

#[tauri::command]
fn save_settings(
    settings: UserSettings,
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<UserSettings, String> {
    let settings = settings.normalized();
    apply_launch_at_startup(settings.launch_at_startup)?;
    apply_mini_window_visibility(&app, settings.show_mini_window);
    write_settings(&app, &settings)?;
    register_global_recording_shortcut(&app, &settings.shortcut)?;
    let mut stored = state.settings.lock().expect("settings mutex poisoned");
    *stored = settings.clone();
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
                    "wav", "mp3", "m4a", "aac", "flac", "ogg", "opus", "wma", "mp4", "mkv", "mov",
                    "webm", "avi",
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
fn open_path_dir(request: OpenPathDirRequest) -> Result<String, String> {
    let requested_path = PathBuf::from(request.path.trim());
    if request.path.trim().is_empty() {
        return Err("Output path is empty.".to_string());
    }

    let dir = if requested_path.is_dir() {
        requested_path.clone()
    } else {
        requested_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| requested_path.clone())
    };
    if !dir.exists() {
        return Err(format!(
            "Output directory does not exist: {}",
            dir.display()
        ));
    }

    #[cfg(windows)]
    {
        let mut command = Command::new("explorer");
        if requested_path.is_file() {
            command.arg(format!("/select,{}", requested_path.display()));
        } else {
            command.arg(&dir);
        }
        command.spawn().map_err(|error| error.to_string())?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&dir)
            .spawn()
            .map_err(|error| error.to_string())?;
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(&dir)
            .spawn()
            .map_err(|error| error.to_string())?;
    }

    Ok(dir.to_string_lossy().to_string())
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
            return Err("The file to export does not exist.".to_string());
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

fn run_ffmpeg_audio_command(
    app: &AppHandle,
    input_path: &Path,
    output_path: &Path,
    pre_input_args: &[String],
    post_input_args: &[String],
    description: &str,
) -> Result<(), String> {
    let ffmpeg = resolve_ffmpeg_runtime(app)?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let mut command = Command::new(ffmpeg);
    command.arg("-y");
    command.args(pre_input_args.iter().map(String::as_str));
    command.arg("-i").arg(input_path);
    command.args(post_input_args.iter().map(String::as_str));
    command
        .arg("-vn")
        .arg("-acodec")
        .arg("pcm_s16le")
        .arg("-ar")
        .arg("48000")
        .arg("-ac")
        .arg("1")
        .arg(output_path);
    suppress_command_window(&mut command);

    let output = match run_command_with_timeout(&mut command, Duration::from_secs(900), description)
    {
        Ok(output) => output,
        Err(error) => {
            let _ = fs::remove_file(output_path);
            return Err(error);
        }
    };
    if output.status.success() {
        return Ok(());
    }

    let _ = fs::remove_file(output_path);
    Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

fn audio_segment_ffmpeg_args(start_seconds: f64, end_seconds: f64) -> (Vec<String>, Vec<String>) {
    let start = (start_seconds - 0.15).max(0.0);
    let end = (end_seconds + 0.15).max(start + 0.1);
    let duration = (end - start).max(0.1);
    (
        vec!["-ss".to_string(), format!("{start:.3}")],
        vec!["-t".to_string(), format!("{duration:.3}")],
    )
}

fn clean_optional_path(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn output_dir_near_source(
    source_path: &Path,
    destination_dir: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(destination_dir) = clean_optional_path(destination_dir) {
        return Ok(PathBuf::from(destination_dir));
    }

    if let Some(parent) = source_path.parent() {
        if !parent.as_os_str().is_empty() {
            return Ok(parent.to_path_buf());
        }
    }

    std::env::current_dir().map_err(|error| error.to_string())
}

fn safe_output_file_name(suggested_name: Option<&str>, fallback: String) -> String {
    clean_optional_path(suggested_name)
        .and_then(|name| Path::new(name).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or(fallback)
}

fn audio_segment_output_path(
    source_path: &Path,
    destination_dir: Option<&str>,
    suggested_name: Option<&str>,
    timestamp: u128,
) -> Result<PathBuf, String> {
    let stem = source_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("segment");
    let file_name =
        safe_output_file_name(suggested_name, format!("{stem}-segment-{timestamp}.wav"));
    Ok(output_dir_near_source(source_path, destination_dir)?.join(file_name))
}

fn processed_audio_output_path(
    input_path: &Path,
    preset_slug: &str,
    destination_dir: Option<&str>,
    timestamp: u128,
) -> Result<PathBuf, String> {
    let stem = input_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    Ok(output_dir_near_source(input_path, destination_dir)?
        .join(format!("{stem}-{preset_slug}-{timestamp}.wav")))
}

fn audio_output_extension(format: &str) -> Result<&'static str, String> {
    match format {
        "wav" => Ok("wav"),
        "mp3" => Ok("mp3"),
        "m4a" => Ok("m4a"),
        "aac" => Ok("aac"),
        "flac" => Ok("flac"),
        "ogg" => Ok("ogg"),
        "opus" => Ok("opus"),
        other => Err(format!("Unsupported output format: {other}")),
    }
}

fn audio_output_codec_args(format: &str, stream_copy: bool) -> Result<Vec<String>, String> {
    if stream_copy {
        return Ok(vec![
            "-vn".to_string(),
            "-c:a".to_string(),
            "copy".to_string(),
        ]);
    }

    let args = match audio_output_extension(format)? {
        "wav" => vec!["-vn", "-acodec", "pcm_s16le", "-ar", "48000", "-ac", "1"],
        "mp3" => vec!["-vn", "-acodec", "libmp3lame", "-b:a", "192k"],
        "m4a" | "aac" => vec!["-vn", "-acodec", "aac", "-b:a", "192k"],
        "flac" => vec!["-vn", "-acodec", "flac"],
        "ogg" => vec!["-vn", "-acodec", "libvorbis", "-q:a", "5"],
        "opus" => vec!["-vn", "-acodec", "libopus", "-b:a", "128k"],
        _ => unreachable!(),
    };

    Ok(args.into_iter().map(ToOwned::to_owned).collect())
}

fn audio_tool_output_path(
    input_path: &Path,
    suffix: &str,
    output_format: &str,
    destination_dir: Option<&str>,
    timestamp: u128,
    suggested_name: Option<&str>,
) -> Result<PathBuf, String> {
    let extension = audio_output_extension(output_format)?;
    let stem = input_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    let fallback = format!("{stem}-{suffix}-{timestamp}.{extension}");
    let mut file_name = safe_output_file_name(suggested_name, fallback);
    if Path::new(&file_name).extension().is_some() {
        file_name = Path::new(&file_name)
            .with_extension(extension)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&file_name)
            .to_string();
    } else {
        file_name.push('.');
        file_name.push_str(extension);
    }
    Ok(output_dir_near_source(input_path, destination_dir)?.join(file_name))
}

fn run_ffmpeg_general_command(
    app: &AppHandle,
    input_path: &Path,
    output_path: &Path,
    pre_input_args: &[String],
    post_input_args: &[String],
    output_format: &str,
    stream_copy: bool,
    description: &str,
) -> Result<(), String> {
    let ffmpeg = resolve_ffmpeg_runtime(app)?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let mut command = Command::new(ffmpeg);
    command.arg("-y");
    command.args(pre_input_args.iter().map(String::as_str));
    command.arg("-i").arg(input_path);
    command.args(post_input_args.iter().map(String::as_str));
    command.args(audio_output_codec_args(output_format, stream_copy)?);
    command.arg(output_path);
    suppress_command_window(&mut command);

    let output = match run_command_with_timeout(&mut command, Duration::from_secs(900), description)
    {
        Ok(output) => output,
        Err(error) => {
            let _ = fs::remove_file(output_path);
            return Err(error);
        }
    };
    if output.status.success() {
        return Ok(());
    }

    let _ = fs::remove_file(output_path);
    Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

fn is_supported_audio_input(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("wav")
            | Some("mp3")
            | Some("m4a")
            | Some("aac")
            | Some("flac")
            | Some("ogg")
            | Some("opus")
            | Some("wma")
            | Some("mp4")
            | Some("mov")
            | Some("mkv")
            | Some("webm")
            | Some("avi")
    )
}

fn collect_supported_audio_files(directory: &Path) -> Result<Vec<PathBuf>, String> {
    if !directory.is_dir() {
        return Err("Selected path is not a folder.".to_string());
    }

    let mut files = Vec::new();
    let mut child_dirs = Vec::new();
    let mut entries = fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            child_dirs.push(path);
        } else if path.is_file() && is_supported_audio_input(&path) {
            files.push(path);
        }
    }

    for child_dir in child_dirs {
        files.extend(collect_supported_audio_files(&child_dir)?);
    }

    Ok(files)
}

fn audio_preview_file_name(source_path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    source_path.to_string_lossy().hash(&mut hasher);
    let hash = hasher.finish();
    let file_name = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("audio-preview.wav");
    format!("{hash:016x}-{file_name}")
}

fn prepare_audio_preview_in_dir(source_path: &Path, cache_root: &Path) -> Result<PathBuf, String> {
    if !source_path.is_file() {
        return Err("Preview audio file does not exist.".to_string());
    }
    if !is_supported_audio_input(source_path) {
        return Err("Preview file is not a supported audio or video format.".to_string());
    }

    let preview_dir = cache_root.join("audio-previews");
    fs::create_dir_all(&preview_dir).map_err(|error| error.to_string())?;
    let preview_path = preview_dir.join(audio_preview_file_name(source_path));
    fs::copy(source_path, &preview_path).map_err(|error| error.to_string())?;
    Ok(preview_path)
}

#[tauri::command]
async fn prepare_audio_preview(
    app: AppHandle,
    request: AudioPreviewRequest,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cache_root = app
            .path()
            .app_cache_dir()
            .map_err(|error| error.to_string())?;
        prepare_audio_preview_in_dir(&PathBuf::from(request.audio_path), &cache_root)
            .map(|path| path.to_string_lossy().to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn parse_ffprobe_fps(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "0/0" || trimmed.eq_ignore_ascii_case("N/A") {
        return None;
    }
    if let Some((left, right)) = trimmed.split_once('/') {
        let numerator = left.trim().parse::<f64>().ok()?;
        let denominator = right.trim().parse::<f64>().ok()?;
        if denominator <= 0.0 {
            return None;
        }
        let fps = numerator / denominator;
        return fps.is_finite().then_some(fps).filter(|fps| *fps > 0.0);
    }
    let fps = trimmed.parse::<f64>().ok()?;
    fps.is_finite().then_some(fps).filter(|fps| *fps > 0.0)
}

fn probe_media_duration_seconds(app: &AppHandle, media_path: &Path) -> Option<f64> {
    let ffprobe = resolve_ffprobe_runtime(app).ok()?;
    let mut command = Command::new(ffprobe);
    command
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(media_path);
    suppress_command_window(&mut command);

    let output =
        run_command_with_timeout(&mut command, Duration::from_secs(20), "duration probe").ok()?;
    if !output.status.success() {
        return None;
    }
    let duration = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()?;
    duration
        .is_finite()
        .then_some(duration)
        .filter(|value| *value > 0.0)
}

fn probe_media_frame_rate_inner(app: &AppHandle, media_path: &Path) -> ProbeMediaFrameRateResult {
    if !media_path.exists() {
        return ProbeMediaFrameRateResult {
            fps: 25.0,
            source: "fallback".to_string(),
            message: "Source file does not exist. Using 25fps fallback.".to_string(),
        };
    }

    let Ok(ffprobe) = resolve_ffprobe_runtime(app) else {
        return ProbeMediaFrameRateResult {
            fps: 25.0,
            source: "fallback".to_string(),
            message: "ffprobe was not found. Using 25fps fallback.".to_string(),
        };
    };

    let mut command = Command::new(ffprobe);
    command
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("stream=avg_frame_rate,r_frame_rate")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(media_path);
    suppress_command_window(&mut command);

    let Ok(output) =
        run_command_with_timeout(&mut command, Duration::from_secs(20), "frame-rate probe")
    else {
        return ProbeMediaFrameRateResult {
            fps: 25.0,
            source: "fallback".to_string(),
            message: "Frame-rate detection failed. Using 25fps fallback.".to_string(),
        };
    };

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(fps) = parse_ffprobe_fps(line) {
                return ProbeMediaFrameRateResult {
                    fps,
                    source: "video".to_string(),
                    message: format!("Detected video frame rate: {fps:.3}fps."),
                };
            }
        }
    }

    ProbeMediaFrameRateResult {
        fps: 25.0,
        source: "fallback".to_string(),
        message: "No video frame rate was detected. Using 25fps fallback.".to_string(),
    }
}

#[tauri::command]
async fn probe_media_frame_rate(
    app: AppHandle,
    request: ProbeMediaFrameRateRequest,
) -> Result<ProbeMediaFrameRateResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        Ok(probe_media_frame_rate_inner(
            &app,
            &PathBuf::from(request.media_path),
        ))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn prepare_audio_waveform(
    app: AppHandle,
    request: AudioWaveformRequest,
) -> Result<AudioWaveformResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source_path = PathBuf::from(&request.media_path);
        if !source_path.exists() {
            return Err("Source audio file does not exist.".to_string());
        }
        if !is_supported_audio_input(&source_path) {
            return Err("Source file is not a supported audio or video format.".to_string());
        }

        let width = request.width.unwrap_or(1600).clamp(320, 3200);
        let height = request.height.unwrap_or(220).clamp(80, 600);
        let mut hasher = DefaultHasher::new();
        source_path.to_string_lossy().hash(&mut hasher);
        width.hash(&mut hasher);
        height.hash(&mut hasher);
        let hash = hasher.finish();
        let cache_root = app
            .path()
            .app_cache_dir()
            .map_err(|error| error.to_string())?;
        let waveform_dir = cache_root.join("audio-waveforms");
        fs::create_dir_all(&waveform_dir).map_err(|error| error.to_string())?;
        let waveform_path = waveform_dir.join(format!("{hash:016x}-{width}x{height}.png"));
        let duration_seconds = probe_media_duration_seconds(&app, &source_path).unwrap_or(0.0);

        if waveform_path.exists() {
            return Ok(AudioWaveformResult {
                waveform_path: waveform_path.to_string_lossy().to_string(),
                duration_seconds,
                message: "Waveform loaded from cache".to_string(),
            });
        }

        let ffmpeg = resolve_ffmpeg_runtime(&app)?;
        let mut command = Command::new(ffmpeg);
        command
            .arg("-y")
            .arg("-i")
            .arg(&source_path)
            .arg("-filter_complex")
            .arg(format!(
                "showwavespic=s={}x{}:colors=0xeff6ff,format=rgba",
                width, height
            ))
            .arg("-frames:v")
            .arg("1")
            .arg(&waveform_path);
        suppress_command_window(&mut command);

        let output =
            run_command_with_timeout(&mut command, Duration::from_secs(900), "audio waveform")?;
        if !output.status.success() {
            let _ = fs::remove_file(&waveform_path);
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }

        Ok(AudioWaveformResult {
            waveform_path: waveform_path.to_string_lossy().to_string(),
            duration_seconds,
            message: "Waveform generated".to_string(),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn convert_audio_file(
    app: AppHandle,
    request: ConvertAudioFileRequest,
) -> Result<AudioProcessingResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let input_path = PathBuf::from(&request.audio_path);
        if !input_path.exists() {
            return Err("Audio or video file does not exist.".to_string());
        }
        if !is_supported_audio_input(&input_path) {
            return Err("Input file is not a supported audio or video format.".to_string());
        }

        let output_path = audio_tool_output_path(
            &input_path,
            "converted",
            &request.output_format,
            request.destination_dir.as_deref(),
            unix_timestamp_millis()?,
            None,
        )?;
        run_ffmpeg_general_command(
            &app,
            &input_path,
            &output_path,
            &[],
            &[],
            &request.output_format,
            false,
            "audio conversion",
        )?;
        Ok(AudioProcessingResult {
            output_path: output_path.to_string_lossy().to_string(),
            message: "Audio conversion complete".to_string(),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn clip_audio_segment(
    app: AppHandle,
    request: ClipAudioSegmentRequest,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source_path = PathBuf::from(&request.source_audio_path);
        if !source_path.exists() {
            return Err("Source audio file does not exist.".to_string());
        }
        if request.end_seconds <= request.start_seconds {
            return Err("Clip end time must be after start time.".to_string());
        }

        let output_path = audio_tool_output_path(
            &source_path,
            "clip",
            &request.output_format,
            request.destination_dir.as_deref(),
            unix_timestamp_millis()?,
            request.suggested_name.as_deref(),
        )?;
        let pre_input_args = vec![
            "-ss".to_string(),
            format!("{:.3}", request.start_seconds.max(0.0)),
        ];
        let post_input_args = vec![
            "-t".to_string(),
            format!(
                "{:.3}",
                (request.end_seconds - request.start_seconds).max(0.001)
            ),
        ];
        run_ffmpeg_general_command(
            &app,
            &source_path,
            &output_path,
            &pre_input_args,
            &post_input_args,
            &request.output_format,
            false,
            "audio clip export",
        )?;
        Ok(output_path.to_string_lossy().to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn clip_audio_segments(
    app: AppHandle,
    request: ClipAudioSegmentsRequest,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source_path = PathBuf::from(&request.source_audio_path);
        if !source_path.exists() {
            return Err("Source audio file does not exist.".to_string());
        }
        if request.segments.is_empty() {
            return Err("At least one clip segment is required.".to_string());
        }
        for segment in &request.segments {
            if segment.end_seconds <= segment.start_seconds {
                return Err("Every clip segment end time must be after start time.".to_string());
            }
        }

        let timestamp = unix_timestamp_millis()?;
        if request.merge_segments {
            let cache_root = app
                .path()
                .app_cache_dir()
                .map_err(|error| error.to_string())?;
            let temp_dir = cache_root
                .join("audio-clips")
                .join(format!("segments-{timestamp}"));
            fs::create_dir_all(&temp_dir).map_err(|error| error.to_string())?;
            let mut temp_paths = Vec::new();
            let result = (|| {
                for (index, segment) in request.segments.iter().enumerate() {
                    let temp_output = temp_dir.join(format!("clip-{:03}.wav", index + 1));
                    let pre_input_args = vec![
                        "-ss".to_string(),
                        format!("{:.3}", segment.start_seconds.max(0.0)),
                    ];
                    let post_input_args = vec![
                        "-t".to_string(),
                        format!(
                            "{:.3}",
                            (segment.end_seconds - segment.start_seconds).max(0.001)
                        ),
                    ];
                    run_ffmpeg_general_command(
                        &app,
                        &source_path,
                        &temp_output,
                        &pre_input_args,
                        &post_input_args,
                        "wav",
                        false,
                        "audio multi-clip temp export",
                    )?;
                    temp_paths.push(temp_output);
                }

                merge_audio_paths(
                    &app,
                    &temp_paths,
                    "reencode",
                    &request.output_format,
                    request.destination_dir.as_deref(),
                    request.suggested_name.as_deref(),
                    unix_timestamp_millis()?,
                )
            })();
            let _ = fs::remove_dir_all(&temp_dir);
            let merged = result?;
            return Ok(vec![merged.to_string_lossy().to_string()]);
        }

        let mut output_paths = Vec::new();
        for (index, segment) in request.segments.iter().enumerate() {
            let suggested_name = segment.suggested_name.as_deref();
            let output_path = audio_tool_output_path(
                &source_path,
                &format!("clip-{:03}", index + 1),
                &request.output_format,
                request.destination_dir.as_deref(),
                unix_timestamp_millis()?,
                suggested_name,
            )?;
            let pre_input_args = vec![
                "-ss".to_string(),
                format!("{:.3}", segment.start_seconds.max(0.0)),
            ];
            let post_input_args = vec![
                "-t".to_string(),
                format!(
                    "{:.3}",
                    (segment.end_seconds - segment.start_seconds).max(0.001)
                ),
            ];
            run_ffmpeg_general_command(
                &app,
                &source_path,
                &output_path,
                &pre_input_args,
                &post_input_args,
                &request.output_format,
                false,
                "audio multi-clip export",
            )?;
            output_paths.push(output_path.to_string_lossy().to_string());
        }
        Ok(output_paths)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn split_audio_file(
    app: AppHandle,
    request: SplitAudioFileRequest,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source_path = PathBuf::from(&request.source_audio_path);
        if !source_path.exists() {
            return Err("Source audio file does not exist.".to_string());
        }
        if request.segment_seconds <= 0.0 || !request.segment_seconds.is_finite() {
            return Err("Split length must be greater than 0 seconds.".to_string());
        }

        let extension = audio_output_extension(&request.output_format)?;
        let timestamp = unix_timestamp_millis()?;
        let stem = source_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("audio");
        let output_dir = output_dir_near_source(&source_path, request.destination_dir.as_deref())?;
        fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;
        let output_pattern = output_dir.join(format!("{stem}-split-{timestamp}-%03d.{extension}"));

        let ffmpeg = resolve_ffmpeg_runtime(&app)?;
        let mut command = Command::new(ffmpeg);
        command
            .arg("-y")
            .arg("-i")
            .arg(&source_path)
            .args(audio_output_codec_args(&request.output_format, false)?)
            .arg("-f")
            .arg("segment")
            .arg("-segment_time")
            .arg(format!("{:.3}", request.segment_seconds))
            .arg("-reset_timestamps")
            .arg("1")
            .arg(&output_pattern);
        suppress_command_window(&mut command);

        let output =
            run_command_with_timeout(&mut command, Duration::from_secs(900), "audio split")?;
        if !output.status.success() {
            let prefix = format!("{stem}-split-{timestamp}-");
            if let Ok(entries) = fs::read_dir(&output_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(extension))
                    {
                        let _ = fs::remove_file(path);
                    }
                }
            }
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }

        let prefix = format!("{stem}-split-{timestamp}-");
        let mut outputs = fs::read_dir(&output_dir)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(extension))
            })
            .collect::<Vec<_>>();
        outputs.sort();
        Ok(outputs
            .into_iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn ffmpeg_concat_line(path: &Path) -> String {
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''");
    format!("file '{normalized}'")
}

fn merge_audio_paths(
    app: &AppHandle,
    audio_paths: &[PathBuf],
    mode: &str,
    output_format: &str,
    destination_dir: Option<&str>,
    suggested_name: Option<&str>,
    timestamp: u128,
) -> Result<PathBuf, String> {
    if audio_paths.len() < 2 {
        return Err("At least two audio files are required for merge.".to_string());
    }
    for path in audio_paths {
        if !path.exists() {
            return Err(format!(
                "Merge source does not exist: {}",
                path.to_string_lossy()
            ));
        }
    }

    let first_path = audio_paths.first().expect("len checked");
    let output_path = audio_tool_output_path(
        first_path,
        "merged",
        output_format,
        destination_dir,
        timestamp,
        suggested_name,
    )?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let cache_root = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?;
    let concat_dir = cache_root.join("audio-concat");
    fs::create_dir_all(&concat_dir).map_err(|error| error.to_string())?;
    let concat_path = concat_dir.join(format!("concat-{timestamp}.txt"));
    let concat_text = audio_paths
        .iter()
        .map(|path| ffmpeg_concat_line(path))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&concat_path, concat_text).map_err(|error| error.to_string())?;

    let ffmpeg = resolve_ffmpeg_runtime(app)?;
    let stream_copy = mode == "copy";
    let mut command = Command::new(ffmpeg);
    command
        .arg("-y")
        .arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&concat_path)
        .args(audio_output_codec_args(output_format, stream_copy)?)
        .arg(&output_path);
    suppress_command_window(&mut command);

    let output = run_command_with_timeout(&mut command, Duration::from_secs(900), "audio merge");
    let _ = fs::remove_file(&concat_path);
    match output {
        Ok(output) if output.status.success() => Ok(output_path),
        Ok(output) => {
            let _ = fs::remove_file(&output_path);
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
        Err(error) => {
            let _ = fs::remove_file(&output_path);
            Err(error)
        }
    }
}

#[tauri::command]
async fn merge_audio_files(
    app: AppHandle,
    request: MergeAudioFilesRequest,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mode = match request.mode.as_str() {
            "copy" | "reencode" => request.mode.as_str(),
            other => return Err(format!("Unsupported merge mode: {other}")),
        };
        let paths = request
            .audio_paths
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let output_path = merge_audio_paths(
            &app,
            &paths,
            mode,
            &request.output_format,
            request.destination_dir.as_deref(),
            request.suggested_name.as_deref(),
            unix_timestamp_millis()?,
        )?;
        Ok(output_path.to_string_lossy().to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn export_audio_segment(
    app: AppHandle,
    request: ExportAudioSegmentRequest,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source_path = PathBuf::from(&request.source_audio_path);
        if !source_path.exists() {
            return Err("Source audio file does not exist.".to_string());
        }

        let output_path = audio_segment_output_path(
            &source_path,
            request.destination_dir.as_deref(),
            request.suggested_name.as_deref(),
            unix_timestamp_millis()?,
        )?;
        let (pre_input_args, post_input_args) =
            audio_segment_ffmpeg_args(request.start_seconds, request.end_seconds);

        run_ffmpeg_audio_command(
            &app,
            &source_path,
            &output_path,
            &pre_input_args,
            &post_input_args,
            "audio segment export",
        )?;
        Ok(output_path.to_string_lossy().to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn list_audio_files_in_directory(
    request: ListAudioFilesInDirectoryRequest,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        collect_supported_audio_files(&PathBuf::from(request.directory_path)).map(|paths| {
            paths
                .into_iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect()
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

fn audio_filter_chain(options: &AudioProcessingOptions) -> String {
    let mut filters = Vec::new();

    if options.trim_silence {
        filters.push("silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.2");
        filters.push("areverse");
        filters.push("silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.3");
        filters.push("areverse");
    }
    if options.voice_filter {
        filters.push("highpass=f=80");
        filters.push("lowpass=f=7800");
    }
    if options.hum_reduction {
        filters.push("equalizer=f=50:t=q:w=1:g=-18");
        filters.push("equalizer=f=60:t=q:w=1:g=-18");
        filters.push("equalizer=f=100:t=q:w=1:g=-10");
        filters.push("equalizer=f=120:t=q:w=1:g=-10");
    }
    if options.noise_reduction {
        filters.push("afftdn=nf=-25");
    }
    if options.normalize {
        filters.push("loudnorm=I=-16:TP=-1.5:LRA=11");
    }

    if filters.is_empty() {
        "anull".to_string()
    } else {
        filters.join(",")
    }
}

fn audio_processing_preset_slug(preset: &str) -> Result<&'static str, String> {
    match preset {
        "normalize" => Ok("normalize"),
        "trimSilence" => Ok("trim-silence"),
        "voiceBasic" => Ok("voice-basic"),
        "humReduction" => Ok("hum-reduction"),
        "lowHighPass" => Ok("low-high-pass"),
        other => Err(format!("Unsupported audio processing preset: {other}")),
    }
}

#[tauri::command]
async fn process_audio_file(
    app: AppHandle,
    request: ProcessAudioFileRequest,
) -> Result<AudioProcessingResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let input_path = PathBuf::from(&request.audio_path);
        if !input_path.exists() {
            return Err("Audio file does not exist.".to_string());
        }

        let preset_slug = audio_processing_preset_slug(&request.options.preset)?;
        let output_path = processed_audio_output_path(
            &input_path,
            preset_slug,
            request.destination_dir.as_deref(),
            unix_timestamp_millis()?,
        )?;
        let filter = audio_filter_chain(&request.options);
        let args = vec!["-af".to_string(), filter];

        run_ffmpeg_audio_command(
            &app,
            &input_path,
            &output_path,
            &[],
            &args,
            "audio processing",
        )?;
        Ok(AudioProcessingResult {
            output_path: output_path.to_string_lossy().to_string(),
            message: "Audio processing complete".to_string(),
        })
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
async fn run_directml_probe(
    app: AppHandle,
    request: DirectMlProbeRequest,
) -> Result<DirectMlProbeResult, String> {
    let app_models_dir = app
        .path()
        .app_local_data_dir()
        .ok()
        .map(|data_dir| data_dir.join("models"));
    tauri::async_runtime::spawn_blocking(move || {
        directml_probe_for_model_dir(&request.model_dir, app_models_dir)
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_native_audio_diagnostics(app: AppHandle) -> NativeAudioDiagnostics {
    native_audio_diagnostics(&app)
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
            open_path_dir,
            save_text_file,
            save_existing_file,
            prepare_audio_preview,
            probe_media_frame_rate,
            prepare_audio_waveform,
            export_audio_segment,
            convert_audio_file,
            clip_audio_segment,
            clip_audio_segments,
            split_audio_file,
            merge_audio_files,
            list_audio_files_in_directory,
            process_audio_file,
            transcribe_file,
            validate_model_dir,
            get_acceleration_status,
            prepare_acceleration_runtime,
            run_acceleration_smoke_test,
            run_directml_probe,
            get_native_audio_diagnostics,
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
    fn clipboard_settle_delay_scales_for_long_text() {
        let short_delay = clipboard_settle_delay("short");
        let long_delay = clipboard_settle_delay(&"长文本".repeat(800));

        assert!(short_delay >= Duration::from_millis(80));
        assert!(long_delay > short_delay);
        assert!(long_delay <= Duration::from_millis(980));
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
        assert!(settings.hotwords.is_empty());
    }

    #[test]
    fn user_settings_normalization_rejects_invalid_runtime_modes() {
        let settings = UserSettings {
            shortcut: String::new(),
            selected_model_id: String::new(),
            paste_mode: "bad".to_string(),
            recording_mode: "bad".to_string(),
            recording_source: "speakers".to_string(),
            acceleration_mode: "gpu".to_string(),
            export_format: "xml".to_string(),
            theme: "blue".to_string(),
            term_categories: Vec::new(),
            ..UserSettings::default()
        }
        .normalized();

        assert_eq!(settings.shortcut, "CapsLock");
        assert_eq!(settings.selected_model_id, "sensevoice-small");
        assert_eq!(settings.paste_mode, "clipboard");
        assert_eq!(settings.recording_mode, "hold");
        assert_eq!(settings.recording_source, "microphone");
        assert_eq!(settings.acceleration_mode, "cpu");
        assert_eq!(settings.export_format, "plainText");
        assert_eq!(settings.theme, "light");
        assert!(!settings.term_categories.is_empty());
    }

    #[test]
    fn applies_enabled_hotword_replacements_longest_first() {
        let rules = vec![
            HotwordRule {
                id: "short".to_string(),
                source: "tao".to_string(),
                target: "Tauri".to_string(),
                enabled: true,
                ..Default::default()
            },
            HotwordRule {
                id: "long".to_string(),
                source: "tao app".to_string(),
                target: "Tauri app".to_string(),
                enabled: true,
                ..Default::default()
            },
            HotwordRule {
                id: "disabled".to_string(),
                source: "disabled term".to_string(),
                target: "SHOULD_NOT_APPEAR".to_string(),
                enabled: false,
                ..Default::default()
            },
        ];

        assert_eq!(
            apply_hotwords("tao app, disabled term", &rules),
            "Tauri app, disabled term"
        );
    }

    #[test]
    fn transcribe_request_defaults_to_cpu_acceleration() {
        let request: TranscribeFileRequest =
            serde_json::from_str(r#"{"audioPath":"sample.wav","modelDir":"models/demo"}"#)
                .expect("request");

        assert_eq!(request.acceleration_mode, "cpu");
        assert!(request.hotwords.is_empty());
    }

    #[test]
    fn retired_acceleration_modes_do_not_change_performance() {
        let fast = transcription_performance("fast");
        let retired_mode = performance_for_acceleration(fast, "cuda");
        let cpu = performance_for_acceleration(fast, "cpu");

        assert_eq!(retired_mode.file_workers, fast.file_workers);
        assert_eq!(retired_mode.chunk_workers, fast.chunk_workers);
        assert_eq!(retired_mode.sherpa_threads, fast.sherpa_threads);
        assert_eq!(cpu.file_workers, fast.file_workers);
        assert_eq!(cpu.chunk_workers, fast.chunk_workers);
        assert_eq!(cpu.sherpa_threads, fast.sherpa_threads);
    }

    #[test]
    fn acceleration_status_keeps_cpu_effective_when_cuda_is_unavailable() {
        let status =
            acceleration_status_from_parts("cuda", false, None, None, true, false, None, None);

        assert_eq!(status.selected_mode, "cuda");
        assert_eq!(status.effective_mode, "cpu");
        assert!(status.message.contains("CPU"));
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
    fn acceleration_status_uses_cpu_when_cuda_runtime_is_not_prepared() {
        let status = acceleration_status_from_parts(
            "cuda",
            true,
            Some("RTX 4090 / driver 555.85 / VRAM 24564 MB".to_string()),
            None,
            true,
            false,
            None,
            None,
        );

        assert_eq!(status.selected_mode, "cuda");
        assert_eq!(status.effective_mode, "cpu");
        assert!(!status.cuda_runtime_installed);
        assert!(status
            .message
            .contains("no local CUDA-capable Sherpa runtime"));
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
        assert!(status.message.contains("disabled for this session"));
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
    fn sensevoice_lfr_features_have_expected_shape_for_one_second_audio() {
        let audio = vec![0.0f32; 16_000];
        let features = extract_sensevoice_lfr_features(&audio);

        assert_eq!(features.frames, 17);
        assert_eq!(features.values.len(), 17 * 560);
        assert!(features.values.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn sensevoice_lfr_features_handle_short_audio() {
        let audio = vec![0.0f32; 400];
        let features = extract_sensevoice_lfr_features(&audio);

        assert!(features.frames >= 1);
        assert_eq!(features.values.len(), features.frames * 560);
        assert!(features.values.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn directml_identity_probe_model_has_expected_onnx_markers() {
        let bytes = directml_identity_model_bytes();
        let text = String::from_utf8_lossy(&bytes);

        assert!(text.contains("Identity"));
        assert!(text.contains("HiVoicerDirectMlIdentityGraph"));
    }

    #[test]
    fn split_sensevoice_candidate_detects_ready_capswriter_layout() {
        let root = std::env::temp_dir().join(format!(
            "hi-voicer-split-sensevoice-test-{}-{}",
            std::process::id(),
            unix_timestamp_millis().unwrap_or(0)
        ));
        let model_dir = root.join("SenseVoice-Small").join("Sensevoice-Small-ONNX");
        fs::create_dir_all(&model_dir).expect("create split model dir");
        for file in [
            "SenseVoice-Encoder.fp16.onnx",
            "SenseVoice-CTC.fp16.onnx",
            "tokenizer.bpe.model",
        ] {
            fs::write(model_dir.join(file), b"placeholder").expect("write split file");
        }

        let candidate =
            select_directml_split_sensevoice_candidate(root.to_string_lossy().as_ref(), None);

        assert_eq!(candidate.model_dir, model_dir);
        assert!(split_sensevoice_missing_files(&candidate).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn directml_probe_candidates_include_app_sensevoice_model() {
        let app_models_dir =
            PathBuf::from(r"C:\Users\tester\AppData\Local\com.local.hivoicer\models");
        let candidates = directml_probe_candidate_dirs(
            r"C:\Portable\Hi-Voicer\models",
            Some(app_models_dir.clone()),
        );

        assert!(candidates.contains(&app_models_dir.join("sensevoice-small")));
    }

    #[test]
    fn directml_probe_candidates_include_sibling_sensevoice_model() {
        let candidates = directml_probe_candidate_dirs(r"C:\Models\sherpa-paraformer-zh", None);

        assert!(candidates.contains(&PathBuf::from(r"C:\Models\sensevoice-small")));
    }

    #[test]
    fn parses_directml_adapter_probe_json() {
        let adapters = directml_adapters_from_powershell_json(
            r#"[{"Name":"NVIDIA GeForce RTX 3060 Laptop GPU","DriverVersion":"31.0.15.8195","AdapterRAM":4293918720,"Status":"OK"}]"#,
        );

        assert_eq!(adapters.len(), 1);
        assert!(is_directml_candidate_adapter(&adapters[0]));
        assert_eq!(adapters[0].adapter_ram_mb, Some(4095));
    }

    #[test]
    fn rejects_basic_display_adapter_as_directml_candidate() {
        let adapter = DirectMlAdapterInfo {
            name: "Microsoft Basic Display Adapter".to_string(),
            driver_version: None,
            adapter_ram_mb: None,
            status: Some("OK".to_string()),
        };

        assert!(!is_directml_candidate_adapter(&adapter));
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
        let engine = InstalledEngineConfig {
            engine: "sherpa-onnx".to_string(),
            model_id: "sensevoice-small".to_string(),
            model_name: "sensevoice-small".to_string(),
            model_dir: String::new(),
            executable: String::new(),
            args: String::new(),
            required_files: Vec::new(),
        };
        let cpu_executable = PathBuf::from(
            r"C:\HiVoicer\runtimes\sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts\bin\sherpa-onnx-offline.exe",
        );
        let cuda_executable = PathBuf::from(
            r"C:\HiVoicer\runtimes\sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda\bin\sherpa-onnx-offline.exe",
        );

        let cpu_server = sherpa_websocket_server_path(&engine, &cpu_executable);
        let cuda_server = sherpa_websocket_server_path(&engine, &cuda_executable);

        assert_ne!(cpu_server, cuda_server);
        assert!(cpu_server.to_string_lossy().contains("static-MT"));
        assert!(cuda_server.to_string_lossy().contains("cuda-12.x"));
        assert!(cpu_server
            .to_string_lossy()
            .contains("sherpa-onnx-offline-websocket-server.exe"));
    }

    #[test]
    fn zipformer_uses_streaming_sherpa_cli_and_server() {
        let engine = InstalledEngineConfig {
            engine: "sherpa-onnx".to_string(),
            model_id: "sherpa-zipformer-zh".to_string(),
            model_name: "Sherpa Zipformer".to_string(),
            model_dir: String::new(),
            executable: String::new(),
            args: String::new(),
            required_files: Vec::new(),
        };
        let offline_executable = PathBuf::from(
            r"C:\HiVoicer\runtimes\sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts\bin\sherpa-onnx-offline.exe",
        );

        let cli_executable = sherpa_cli_executable_for_engine(&engine, &offline_executable);
        let server_executable = sherpa_websocket_server_path(&engine, &cli_executable);

        assert!(cli_executable
            .to_string_lossy()
            .ends_with("sherpa-onnx.exe"));
        assert!(server_executable
            .to_string_lossy()
            .ends_with("sherpa-onnx-online-websocket-server.exe"));
    }

    #[test]
    fn llm_asr_models_use_short_chunks() {
        let qwen = InstalledEngineConfig {
            engine: "sherpa-onnx".to_string(),
            model_id: "qwen3-asr-0.6b".to_string(),
            model_name: "Qwen3-ASR 0.6B".to_string(),
            model_dir: String::new(),
            executable: String::new(),
            args: String::new(),
            required_files: Vec::new(),
        };
        let funasr = InstalledEngineConfig {
            model_id: "sherpa-funasr-nano".to_string(),
            model_name: "Sherpa FunASR-Nano".to_string(),
            ..qwen.clone()
        };
        let paraformer = InstalledEngineConfig {
            model_id: "sherpa-paraformer-zh".to_string(),
            model_name: "Sherpa Paraformer".to_string(),
            ..qwen.clone()
        };

        assert_eq!(sherpa_chunk_seconds(&qwen), LLM_ASR_CHUNK_SECONDS);
        assert_eq!(sherpa_chunk_seconds(&funasr), LLM_ASR_CHUNK_SECONDS);
        assert_eq!(
            sherpa_max_single_pass_seconds(&qwen),
            LLM_ASR_CHUNK_SECONDS as f64
        );
        assert_eq!(sherpa_chunk_seconds(&paraformer), LONG_AUDIO_CHUNK_SECONDS);
        assert_eq!(
            sherpa_max_single_pass_seconds(&paraformer),
            LONG_AUDIO_THRESHOLD_SECONDS
        );
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
        let output = r#"{"lang":"<|zh|>","text":"hello, test passed"}"#;

        assert_eq!(extract_transcription_text(output), "hello, test passed");
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
  "text": "text from audio",
  "tokens": ["text", "from", "audio"]
}
Elapsed seconds: 0.16
"#;

        assert_eq!(extract_transcription_text(output), "text from audio");
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
    fn ignores_qwen_language_placeholder_result() {
        let output = r#"{"lang":"","emotion":"","event":"","text":"language","timestamps":[],"durations":[],"tokens":["language"],"ys_log_probs":[],"words":[]}"#;

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
        let segments = build_transcript_segments("hello", 1.0, &source_audio);

        let (primary, outputs, files) = write_transcription_outputs(
            &root,
            "plainText",
            &source_audio,
            &sherpa_audio,
            "hello",
            &segments,
        )
        .expect("outputs");

        assert_eq!(outputs.len(), 5);
        assert_eq!(files.len(), 5);
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
        let segments = build_transcript_segments("hello", 1.0, &source_audio);

        let (primary, outputs, files) = write_transcription_outputs(
            &root,
            "timelineText",
            &source_audio,
            &sherpa_audio,
            "hello",
            &segments,
        )
        .expect("outputs");

        assert!(PathBuf::from(&primary).starts_with(&root));
        assert!(primary.ends_with("-timeline.txt"));
        assert_eq!(outputs.len(), 5);
        assert_eq!(
            files
                .iter()
                .map(|file| file.format.as_str())
                .collect::<Vec<_>>(),
            vec![
                "plainText",
                "timelineText",
                "timelineTxt",
                "srt",
                "resolveMarkers"
            ]
        );
        assert!(fs::read_to_string(&files[1].path)
            .expect("timeline txt")
            .contains("[00:00:00:00 -->"));
        let timeline_txt_bytes = fs::read(&files[2].path).expect("timeline txt bom");
        assert_eq!(&timeline_txt_bytes[..3], &[0xEF, 0xBB, 0xBF]);
        let timeline_txt =
            String::from_utf8(timeline_txt_bytes[3..].to_vec()).expect("timeline txt utf8");
        assert!(timeline_txt.contains("[00:00:00:00 --> 00:00:01:00] hello"));
        assert!(!timeline_txt.contains("]\nhello"));
        assert!(fs::read_to_string(&files[3].path)
            .expect("srt")
            .contains("00:00:00,000 -->"));
        let resolve_markers = fs::read_to_string(&files[4].path).expect("resolve markers");
        assert!(resolve_markers.contains("TITLE: sample"));
        assert!(resolve_markers.contains("FCM: NON-DROP FRAME"));
        assert!(resolve_markers.contains("01:00:00:00 01:00:00:01"));
        assert!(resolve_markers.contains("|C:ResolveColorYellow |M:hello |D:1"));
    }

    #[test]
    fn formats_timeline_timestamp_as_davinci_timecode() {
        assert_eq!(format_timeline_timestamp(80.5), "00:01:20:12");
    }

    #[test]
    fn formats_srt_timestamp_with_standard_millisecond_comma() {
        assert_eq!(format_srt_timestamp(80.5), "00:01:20,500");
    }

    #[test]
    fn timeline_txt_can_be_primary_without_replacing_original_timeline_text() {
        let root =
            test_root("timeline-txt-can-be-primary-without-replacing-original-timeline-text");
        let source_audio = root.join("sample.wav");
        let sherpa_audio = root.join("sample-16k.wav");
        fs::write(&source_audio, b"source").expect("write source");
        write_test_wav(&sherpa_audio);
        let segments = build_transcript_segments("hello", 1.0, &source_audio);

        let (primary, _outputs, files) = write_transcription_outputs(
            &root,
            "timelineTxt",
            &source_audio,
            &sherpa_audio,
            "hello",
            &segments,
        )
        .expect("outputs");

        assert!(primary.ends_with("-timeline-txt.txt"));
        assert!(files
            .iter()
            .any(|file| file.path.ends_with("-timeline.txt")));
        assert!(files
            .iter()
            .any(|file| file.path.ends_with("-timeline-txt.txt")));
    }

    #[test]
    fn uses_resolve_markers_as_primary_output_when_requested() {
        let root = test_root("uses-resolve-markers-as-primary-output-when-requested");
        let source_audio = root.join("sample.wav");
        let sherpa_audio = root.join("sample-16k.wav");
        fs::write(&source_audio, b"source").expect("write source");
        write_test_wav(&sherpa_audio);
        let segments = build_transcript_segments("hello", 1.0, &source_audio);

        let (primary, _outputs, _files) = write_transcription_outputs(
            &root,
            "resolveMarkers",
            &source_audio,
            &sherpa_audio,
            "hello",
            &segments,
        )
        .expect("outputs");

        assert!(PathBuf::from(&primary).starts_with(&root));
        assert!(primary.ends_with(".markers.edl"));
    }

    #[test]
    fn audio_segment_args_keep_seek_before_input_and_duration_after_input() {
        let (pre_input_args, post_input_args) = audio_segment_ffmpeg_args(1.0, 2.0);

        assert_eq!(pre_input_args, vec!["-ss".to_string(), "0.850".to_string()]);
        assert_eq!(post_input_args, vec!["-t".to_string(), "1.300".to_string()]);

        let (pre_input_args, post_input_args) = audio_segment_ffmpeg_args(0.05, 0.1);
        assert_eq!(pre_input_args, vec!["-ss".to_string(), "0.000".to_string()]);
        assert_eq!(post_input_args, vec!["-t".to_string(), "0.250".to_string()]);
    }

    #[test]
    fn audio_segment_output_defaults_next_to_source_audio() {
        let source = PathBuf::from(r"C:\Recordings\demo.wav");
        let output = audio_segment_output_path(&source, None, Some("demo-segment-001.wav"), 1234)
            .expect("output path");

        assert_eq!(output, PathBuf::from(r"C:\Recordings\demo-segment-001.wav"));
    }

    #[test]
    fn processed_audio_output_uses_custom_directory_when_provided() {
        let input = PathBuf::from(r"C:\Recordings\voice.wav");
        let output =
            processed_audio_output_path(&input, "voice-basic", Some(r"D:\Processed"), 1234)
                .expect("output path");

        assert_eq!(
            output,
            PathBuf::from(r"D:\Processed\voice-voice-basic-1234.wav")
        );
    }

    #[test]
    fn audio_output_format_only_accepts_supported_formats() {
        assert_eq!(audio_output_extension("mp3").expect("mp3"), "mp3");
        assert_eq!(audio_output_extension("opus").expect("opus"), "opus");
        assert!(audio_output_extension("../bad").is_err());
    }

    #[test]
    fn audio_output_codec_args_support_copy_and_reencode_modes() {
        assert_eq!(
            audio_output_codec_args("wav", false).expect("wav"),
            vec![
                "-vn".to_string(),
                "-acodec".to_string(),
                "pcm_s16le".to_string(),
                "-ar".to_string(),
                "48000".to_string(),
                "-ac".to_string(),
                "1".to_string()
            ]
        );
        assert_eq!(
            audio_output_codec_args("mp3", true).expect("copy"),
            vec!["-vn".to_string(), "-c:a".to_string(), "copy".to_string()]
        );
    }

    #[test]
    fn audio_tool_output_path_uses_requested_format_and_custom_name() {
        let input = PathBuf::from(r"C:\Recordings\voice.wav");
        let output = audio_tool_output_path(
            &input,
            "clip",
            "mp3",
            Some(r"D:\Processed"),
            1234,
            Some("take-one"),
        )
        .expect("output path");

        assert_eq!(output, PathBuf::from(r"D:\Processed\take-one.mp3"));

        let output = audio_tool_output_path(
            &input,
            "clip",
            "flac",
            Some(r"D:\Processed"),
            1234,
            Some("take-one.mp3"),
        )
        .expect("output path");

        assert_eq!(output, PathBuf::from(r"D:\Processed\take-one.flac"));
    }

    #[test]
    fn parses_ffprobe_fractional_frame_rates() {
        assert_eq!(parse_ffprobe_fps("25/1").expect("25fps"), 25.0);
        assert!((parse_ffprobe_fps("30000/1001").expect("ntsc") - 29.970).abs() < 0.01);
        assert!(parse_ffprobe_fps("0/0").is_none());
        assert!(parse_ffprobe_fps("N/A").is_none());
    }

    #[test]
    fn concat_lines_normalize_windows_paths() {
        let line = ffmpeg_concat_line(&PathBuf::from(r"C:\Audio Files\voice.wav"));

        assert_eq!(line, "file 'C:/Audio Files/voice.wav'");
    }

    #[test]
    fn collects_supported_audio_files_from_directory_recursively() {
        let root = test_root("collects-supported-audio-files-from-directory-recursively");
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("create nested dir");
        fs::write(root.join("voice.wav"), b"wav").expect("write wav");
        fs::write(nested.join("meeting.mp3"), b"mp3").expect("write mp3");
        fs::write(root.join("notes.txt"), b"text").expect("write text");

        let files = collect_supported_audio_files(&root).expect("collect files");

        assert_eq!(
            files,
            vec![root.join("voice.wav"), nested.join("meeting.mp3")]
        );
    }

    #[test]
    fn prepare_audio_preview_copies_file_into_cache_dir() {
        let root = test_root("prepare-audio-preview-copies-file-into-cache-dir");
        let source = root.join("source").join("voice.wav");
        let cache = root.join("cache");
        fs::create_dir_all(source.parent().expect("source parent")).expect("create source dir");
        fs::write(&source, b"wav bytes").expect("write source wav");

        let preview = prepare_audio_preview_in_dir(&source, &cache).expect("prepare preview");

        assert!(preview.starts_with(cache.join("audio-previews")));
        assert!(preview
            .file_name()
            .and_then(|name| name.to_str())
            .expect("preview file name")
            .ends_with("-voice.wav"));
        assert_eq!(fs::read(&preview).expect("read preview"), b"wav bytes");
    }

    #[test]
    fn prepare_audio_preview_rejects_missing_file() {
        let root = test_root("prepare-audio-preview-rejects-missing-file");
        let error = prepare_audio_preview_in_dir(&root.join("missing.wav"), &root.join("cache"))
            .expect_err("missing source should fail");

        assert!(error.contains("does not exist"));
    }

    #[test]
    fn recording_mix_filter_does_not_amplify_tracks() {
        let filter = recording_mix_filter(2);

        assert_eq!(
            filter,
            "amix=inputs=2:duration=longest:dropout_transition=0"
        );
        assert!(!filter.contains("volume="));
    }

    #[test]
    fn audio_config_detail_includes_sample_rate_channels_and_format() {
        let config = cpal::SupportedStreamConfig::new(
            1,
            cpal::SampleRate(48_000),
            cpal::SupportedBufferSize::Unknown,
            cpal::SampleFormat::F32,
        );

        let detail = audio_config_detail(&config);

        assert!(detail.contains("48000 Hz"));
        assert!(detail.contains("1 channel"));
        assert!(detail.contains("F32"));
    }

    #[test]
    fn system_audio_diagnostic_detail_marks_loopback_as_runtime_verified() {
        let config = cpal::SupportedStreamConfig::new(
            2,
            cpal::SampleRate(48_000),
            cpal::SupportedBufferSize::Unknown,
            cpal::SampleFormat::F32,
        );

        let detail = system_audio_diagnostic_detail(&config);

        assert!(detail.contains("48000 Hz"));
        assert!(detail.contains("WASAPI loopback"));
        assert!(detail.contains("verified when recording starts"));
    }

    #[test]
    fn sherpa_runtime_candidates_include_bundled_and_portable_locations() {
        let data_dir = PathBuf::from(r"C:\Users\tester\AppData\Local\com.local.hivoicer");
        let resource_dir = PathBuf::from(r"C:\Program Files\Hi-Voicer");
        let executable_dir = PathBuf::from(r"C:\Portable\Hi-Voicer");
        let roots =
            sherpa_runtime_search_roots(&data_dir, Some(&resource_dir), Some(&executable_dir));
        let candidates = sherpa_runtime_executable_candidates(&roots, SHERPA_CUDA_RUNTIME_NAME);

        assert!(candidates.contains(&PathBuf::from(
            r"C:\Users\tester\AppData\Local\com.local.hivoicer\engines\sherpa\v1.13.2\sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda\bin\sherpa-onnx-offline.exe"
        )));
        assert!(candidates.contains(&PathBuf::from(
            r"C:\Program Files\Hi-Voicer\engines\sherpa\v1.13.2\sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda\bin\sherpa-onnx-offline.exe"
        )));
        assert!(candidates.contains(&PathBuf::from(
            r"C:\Portable\Hi-Voicer\engines\sherpa\v1.13.2\sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda\bin\sherpa-onnx-offline.exe"
        )));
    }
    #[test]
    fn cuda_root_candidates_include_versioned_bin_children() {
        let root = test_root("cuda-root-candidates-include-versioned-bin-children");
        let versioned_bin = root.join("v9.7").join("bin").join("12.8");
        fs::create_dir_all(&versioned_bin).expect("create versioned cudnn bin");

        let mut dirs = Vec::new();
        push_cuda_root_candidates(&mut dirs, root.clone());

        assert!(dirs.contains(&versioned_bin));
    }
    #[test]
    fn cuda_dependency_status_reports_missing_required_dlls() {
        let root = test_root("cuda-dependency-status-reports-missing-required-dlls");
        fs::create_dir_all(&root).expect("create cuda dir");

        let error = cuda_dependency_status_from_dirs(
            &[root.clone()],
            &["cudart64_12.dll", "cublasLt64_12.dll"],
        )
        .expect_err("missing dlls");

        assert!(error.contains("cudart64_12.dll"));
        assert!(error.contains("cublasLt64_12.dll"));
        assert!(error.contains(&root.to_string_lossy().to_string()));
    }

    #[test]
    fn cuda_dependency_status_collects_split_dependency_dirs() {
        let root = test_root("cuda-dependency-status-collects-split-dependency-dirs");
        let cuda_bin = root.join("cuda").join("bin");
        let cudnn_bin = root.join("cudnn").join("bin");
        fs::create_dir_all(&cuda_bin).expect("create cuda bin");
        fs::create_dir_all(&cudnn_bin).expect("create cudnn bin");
        fs::write(cuda_bin.join("cudart64_12.dll"), b"dll").expect("write cudart");
        fs::write(cudnn_bin.join("cudnn64_9.dll"), b"dll").expect("write cudnn");

        let dirs = cuda_dependency_status_from_dirs(
            &[cuda_bin.clone(), cudnn_bin.clone()],
            &["cudart64_12.dll", "cudnn64_9.dll"],
        )
        .expect("cuda deps");

        assert_eq!(dirs, vec![cuda_bin, cudnn_bin]);
    }
    #[test]
    fn ffmpeg_search_roots_include_offline_locations() {
        let data_dir = PathBuf::from(r"C:\Users\tester\AppData\Local\com.local.hivoicer");
        let resource_dir = PathBuf::from(r"C:\Program Files\Hi-Voicer");
        let executable_dir = PathBuf::from(r"C:\Portable\Hi-Voicer");

        let roots =
            ffmpeg_runtime_search_roots(&data_dir, Some(&resource_dir), Some(&executable_dir));

        assert!(roots.contains(&PathBuf::from(
            r"C:\Users\tester\AppData\Local\com.local.hivoicer\engines\ffmpeg"
        )));
        assert!(roots.contains(&PathBuf::from(r"C:\Program Files\Hi-Voicer\engines\ffmpeg")));
        assert!(roots.contains(&PathBuf::from(r"C:\Program Files\Hi-Voicer\ffmpeg")));
        assert!(roots.contains(&PathBuf::from(r"C:\Portable\Hi-Voicer\engines\ffmpeg")));
    }

    #[test]
    fn finds_ffmpeg_recursively_without_installing() {
        let root = test_root("finds-ffmpeg-recursively-without-installing");
        let ffmpeg_dir = root.join("engines").join("ffmpeg").join("bin");
        fs::create_dir_all(&ffmpeg_dir).expect("create ffmpeg dir");
        let ffmpeg_path = ffmpeg_dir.join("ffmpeg.exe");
        fs::write(&ffmpeg_path, b"exe").expect("write ffmpeg");

        let found = find_ffmpeg_in_roots(&[root.join("engines").join("ffmpeg")])
            .expect("lookup")
            .expect("ffmpeg");

        assert_eq!(found, ffmpeg_path);
    }

    #[test]
    fn missing_ffmpeg_message_is_offline_first() {
        let message = ffmpeg_missing_message();

        assert!(message.contains("offline audio"));
        assert!(message.contains("will not download"));
        assert!(message.contains("ffmpeg.exe"));
    }

    #[test]
    fn missing_ffmpeg_detail_includes_actionable_search_locations() {
        let roots = vec![
            PathBuf::from(r"C:\Users\tester\AppData\Local\com.local.hivoicer\engines\ffmpeg"),
            PathBuf::from(r"C:\Program Files\Hi-Voicer\engines\ffmpeg"),
        ];

        let detail = ffmpeg_missing_detail(&roots);

        assert!(detail.contains("ffmpeg.exe was not found"));
        assert!(detail.contains(r"C:\Users\tester\AppData\Local\com.local.hivoicer\engines\ffmpeg"));
        assert!(detail.contains(r"C:\Program Files\Hi-Voicer\engines\ffmpeg"));
        assert!(detail.contains("PATH"));
        assert!(detail.contains("will not download"));
    }

    #[test]
    fn audio_processing_preset_slug_only_accepts_known_presets() {
        assert_eq!(
            audio_processing_preset_slug("voiceBasic").expect("preset"),
            "voice-basic"
        );
        assert_eq!(
            audio_processing_preset_slug("trimSilence").expect("preset"),
            "trim-silence"
        );
        assert!(audio_processing_preset_slug("../bad").is_err());
    }

    #[test]
    fn trim_silence_filter_does_not_stop_at_first_internal_pause() {
        let filter = audio_filter_chain(&AudioProcessingOptions {
            preset: "voiceBasic".to_string(),
            normalize: true,
            trim_silence: true,
            hum_reduction: false,
            voice_filter: true,
            noise_reduction: true,
        });

        assert!(!filter.contains("stop_periods=1"));
        assert!(filter.contains("areverse,silenceremove=start_periods=1"));
    }

    #[test]
    fn transcript_chunks_keep_leading_punctuation_with_previous_line() {
        let chunks = split_text_into_chunks("第一句话。，第二句话继续说完。");

        assert_eq!(chunks, vec!["第一句话。，", "第二句话继续说完。"]);
        assert!(!chunks.iter().any(|chunk| chunk.starts_with('，')));
    }

    #[test]
    fn transcript_chunks_prefer_sentence_breaks_before_hard_limits() {
        let text = "这是第一句完整的话。这是一个比较长的句子，里面有逗号，可以在需要的时候拆开，但不应该把标点放到下一行开头。";
        let chunks = split_text_into_chunks(text);

        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0], "这是第一句完整的话。");
        assert!(!chunks.iter().any(|chunk| {
            chunk.starts_with('，') || chunk.starts_with('。') || chunk.starts_with('、')
        }));
    }

    #[test]
    fn transcript_segments_have_monotonic_non_overlapping_estimated_times() {
        let source_audio = PathBuf::from("source.wav");
        let segments = build_transcript_segments(
            "a? this is a much longer subtitle segment with many words? z?",
            1.0,
            &source_audio,
        );

        assert!(segments.len() >= 3);
        for pair in segments.windows(2) {
            assert!(pair[0].end <= pair[1].start);
            assert!(pair[0].end > pair[0].start);
        }
        assert_eq!(segments.first().expect("first").start, 0.0);
        assert!(segments.last().expect("last").end >= 1.5);
    }

    #[test]
    fn transcript_segments_handle_non_finite_estimated_duration() {
        let source_audio = PathBuf::from("source.wav");
        let segments = build_transcript_segments("hello? world?", f64::NAN, &source_audio);

        assert_eq!(segments.len(), 2);
        assert!(segments
            .iter()
            .all(|segment| segment.start.is_finite() && segment.end.is_finite()));
        assert_eq!(segments[0].start, 0.0);
        assert!(segments[0].end <= segments[1].start);
    }

    #[test]
    fn transcript_segments_preserve_transcription_chunk_boundaries() {
        let source_audio = PathBuf::from("source.wav");
        let transcript_chunks = vec![
            TranscriptTextChunk {
                text: "short.".to_string(),
                start: 0.0,
                end: 60.0,
            },
            TranscriptTextChunk {
                text: "this chunk has much more spoken text and should still start at one minute."
                    .to_string(),
                start: 60.0,
                end: 120.0,
            },
        ];

        let segments = build_transcript_segments_from_chunks(&transcript_chunks, &source_audio);

        assert_eq!(segments[0].start, 0.0);
        assert!(segments
            .iter()
            .any(|segment| (segment.start - 60.0).abs() < 0.001));
        assert_eq!(segments.last().expect("last").end, 120.0);
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
    fn retired_cuda_runtime_args_do_not_force_cuda_provider() {
        assert_eq!(
            sherpa_args_for_runtime("--tokens=a --model=b", Some(2), "cuda").expect("args"),
            vec![
                "--tokens=a".to_string(),
                "--model=b".to_string(),
                "--num-threads=2".to_string()
            ]
        );
    }

    #[test]
    fn retired_cuda_runtime_args_preserve_existing_provider_forms() {
        assert_eq!(
            sherpa_args_for_runtime("--provider=cpu --tokens=a", None, "cuda").expect("args"),
            vec!["--provider=cpu".to_string(), "--tokens=a".to_string()]
        );
        assert_eq!(
            sherpa_args_for_runtime("--provider cpu --tokens=a", None, "cuda").expect("args"),
            vec![
                "--provider".to_string(),
                "cpu".to_string(),
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
            let duration = wav_duration_seconds(&chunk.path).expect("duration");
            assert!((duration - 1.0).abs() < 0.01);
        }
    }

    #[test]
    fn recording_output_validation_accepts_usable_wav() {
        let root = test_root("recording-output-validation-accepts-usable-wav");
        let wav_path = root.join("voice.wav");
        write_test_wav_seconds(&wav_path, 1);

        validate_recording_output_file(&wav_path).expect("valid recording");
    }

    #[test]
    fn recording_output_validation_rejects_empty_wav() {
        let root = test_root("recording-output-validation-rejects-empty-wav");
        let wav_path = root.join("empty.wav");
        write_test_wav_seconds(&wav_path, 0);

        let error = validate_recording_output_files(&[wav_path]).expect_err("empty recording");

        assert!(error.contains("Recording did not contain usable audio"));
        assert!(error.contains("too short or empty"));
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
