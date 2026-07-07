mod app_state;
mod config;

use app_state::AppSnapshot;
use config::{HotwordRule, UserSettings};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet, VecDeque},
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
const SHERPA_WEBSOCKET_DAEMON_ENABLED: bool = true;
const SHERPA_WEBSOCKET_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const DAVINCI_TIMECODE_FPS: u64 = 25;
const LONG_AUDIO_CHUNK_SECONDS: u32 = 60;
const LONG_AUDIO_THRESHOLD_SECONDS: f64 = 300.0;
const LLM_ASR_CHUNK_SECONDS: u32 = 20;
const QWEN_ASR_CHUNK_SECONDS: u32 = 20;
const QWEN_ASR_MAX_NEW_TOKENS: usize = 128;
const QWEN_SILENT_CHUNK_RMS_THRESHOLD: f64 = 0.0008;
const QWEN_SILENT_CHUNK_PEAK_THRESHOLD: f64 = 0.006;
const MIN_RECORDING_SECONDS: f64 = 0.05;
const SHERPA_RUNTIME_TAG: &str = "v1.13.2";
const SHERPA_CPU_RUNTIME_NAME: &str = "sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts";
const SHERPA_CPU_ARCHIVE_NAME: &str =
    "sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts.tar.bz2";
const SHERPA_CPU_RUNTIME_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts.tar.bz2";

struct RuntimeState {
    settings: Mutex<UserSettings>,
    recording: Mutex<Option<RecordingSession>>,
    sherpa_daemon: Mutex<Option<SherpaDaemon>>,
    directml_sensevoice: Mutex<Option<DirectMlSenseVoiceRuntime>>,
    sherpa_runtime_install: Mutex<()>,
    transcription_cancellations: Mutex<HashSet<String>>,
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

struct DirectMlSenseVoiceRuntime {
    encoder_path: PathBuf,
    ctc_path: PathBuf,
    tokenizer_path: PathBuf,
    pieces: Vec<String>,
    encoder_session: ort::session::Session,
    ctc_session: ort::session::Session,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelTranscriptionRequest {
    task_id: String,
}

fn mark_transcription_cancelled(app: &AppHandle, task_id: &str) -> Result<(), String> {
    let state = app.state::<RuntimeState>();
    let mut cancellations = state
        .transcription_cancellations
        .lock()
        .map_err(|error| error.to_string())?;
    cancellations.insert(task_id.to_string());
    Ok(())
}

fn clear_transcription_cancelled(app: &AppHandle, task_id: Option<&str>) -> Result<(), String> {
    let Some(task_id) = task_id else {
        return Ok(());
    };
    let state = app.state::<RuntimeState>();
    let mut cancellations = state
        .transcription_cancellations
        .lock()
        .map_err(|error| error.to_string())?;
    cancellations.remove(task_id);
    Ok(())
}

fn transcription_cancelled(app: &AppHandle, task_id: Option<&str>) -> bool {
    let Some(task_id) = task_id else {
        return false;
    };
    let state = app.state::<RuntimeState>();
    state
        .transcription_cancellations
        .lock()
        .map(|cancellations| cancellations.contains(task_id))
        .unwrap_or(false)
}

fn ensure_transcription_not_cancelled(
    app: &AppHandle,
    task_id: Option<&str>,
) -> Result<(), String> {
    if transcription_cancelled(app, task_id) {
        Err("Transcription cancelled.".to_string())
    } else {
        Ok(())
    }
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

fn performance_for_model_and_acceleration(
    performance: TranscriptionPerformance,
    engine: &InstalledEngineConfig,
    acceleration_mode: &str,
) -> TranscriptionPerformance {
    let mut performance = performance_for_acceleration(performance, acceleration_mode);
    if engine.model_id == "qwen3-asr-0.6b" {
        if performance.file_workers <= 1 && performance.chunk_workers <= 1 {
            performance.sherpa_threads = 6;
        } else if performance.chunk_workers <= 2 {
            performance.sherpa_threads = 3;
        } else {
            performance.sherpa_threads = 2;
        }
    }
    performance
}
fn directml_transcription_supported_for_model(engine: &InstalledEngineConfig) -> bool {
    engine.model_id == "sensevoice-small"
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
    used_acceleration_mode: String,
    acceleration_fallback_used: bool,
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

    if !matches!(engine.engine.as_str(), "sherpa-onnx" | "faster-whisper") {
        return ModelValidationResult {
            valid: false,
            model_name: engine.model_name,
            message: format!("Unsupported engine: {}", engine.engine),
        };
    }

    if !PathBuf::from(&engine.executable).exists() {
        let runtime_name = if engine.engine == "faster-whisper" {
            "Faster-Whisper worker"
        } else {
            "Sherpa-ONNX executable"
        };
        return ModelValidationResult {
            valid: false,
            model_name: engine.model_name,
            message: format!(
                "The {runtime_name} does not exist. Download and configure the model again."
            ),
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

fn sherpa_args_for_engine_runtime(
    engine: &InstalledEngineConfig,
    threads: Option<usize>,
    runtime_mode: &str,
) -> Result<Vec<String>, String> {
    let mut parsed = sherpa_args_for_runtime(&engine.args, threads, runtime_mode)?;
    if engine.model_id == "qwen3-asr-0.6b" {
        set_sherpa_arg_value(
            &mut parsed,
            "--qwen3-asr-max-new-tokens",
            &QWEN_ASR_MAX_NEW_TOKENS.to_string(),
            true,
        );
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

    if _requested_mode == "directml" {
        return Ok(AccelerationStatus {
            selected_mode: "directml".to_string(),
            effective_mode: "directml".to_string(),
            cuda_available: false,
            cuda_device_summary: None,
            cuda_detection_error: None,
            cpu_runtime_installed,
            cuda_runtime_installed: false,
            cuda_disabled_reason: None,
            message: "DirectML experimental mode is selected. Production transcription currently supports split SenseVoice; Qwen3-ASR uses optimized Sherpa CPU while its DirectML chain remains diagnostic-only.".to_string(),
        });
    }

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
        "qwen3-asr-0.6b" => QWEN_ASR_CHUNK_SECONDS as f64,
        "sherpa-funasr-nano" => LLM_ASR_CHUNK_SECONDS as f64,
        _ => LONG_AUDIO_THRESHOLD_SECONDS,
    }
}

fn sherpa_chunk_seconds(engine: &InstalledEngineConfig) -> u32 {
    match engine.model_id.as_str() {
        "qwen3-asr-0.6b" => QWEN_ASR_CHUNK_SECONDS,
        "sherpa-funasr-nano" => LLM_ASR_CHUNK_SECONDS,
        _ => LONG_AUDIO_CHUNK_SECONDS,
    }
}

#[cfg(test)]
fn is_cjk_text_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{20000}'..='\u{2A6DF}'
            | '\u{2A700}'..='\u{2B73F}'
            | '\u{2B740}'..='\u{2B81F}'
            | '\u{2B820}'..='\u{2CEAF}'
            | '\u{30000}'..='\u{3134F}'
    )
}

#[cfg(test)]
fn looks_like_qwen_latin_language_drift(text: &str) -> bool {
    let cjk_count = text.chars().filter(|ch| is_cjk_text_char(*ch)).count();
    if cjk_count > 0 {
        return false;
    }

    let alphabetic_count = text.chars().filter(|ch| ch.is_alphabetic()).count();
    let word_count = text
        .split(|ch: char| !ch.is_alphabetic())
        .filter(|word| word.chars().count() >= 2)
        .count();

    alphabetic_count >= 32 && word_count >= 6
}

#[cfg(test)]
fn should_keep_sherpa_chunk_text(engine: &InstalledEngineConfig, text: &str) -> bool {
    if engine.model_id != "qwen3-asr-0.6b" {
        return true;
    }
    !looks_like_qwen_latin_language_drift(text)
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
fn sherpa_daemon_supported_for_model(engine: &InstalledEngineConfig) -> bool {
    engine.model_id == "qwen3-asr-0.6b"
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
    command.args(sherpa_args_for_engine_runtime(engine, None, runtime_mode)?);
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

fn decode_sherpa_websocket_text(bytes: &[u8]) -> String {
    match String::from_utf8(bytes.to_vec()) {
        Ok(text) => text,
        Err(_) => {
            let (text, _, _) = encoding_rs::GBK.decode(bytes);
            text.into_owned()
        }
    }
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

                for chunk in payload.chunks(10_240) {
                    stream
                        .write_all(&websocket_frame(0x2, chunk))
                        .map_err(|error| error.to_string())?;
                }
                let response = read_websocket_message(&mut stream)?;
                let _ = stream.write_all(&websocket_frame(0x1, b"Done"));
                let response_text = decode_sherpa_websocket_text(&response);
                let text = extract_transcription_text(&response_text);
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

fn wav_likely_silent_for_qwen(wav_path: &Path) -> Result<bool, String> {
    let mut reader = hound::WavReader::open(wav_path).map_err(|error| error.to_string())?;
    let spec = reader.spec();
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Ok(false);
    }

    let mut sample_count = 0usize;
    let mut square_sum = 0.0f64;
    let mut peak = 0.0f64;
    for sample in reader.samples::<i16>() {
        let value = f64::from(sample.map_err(|error| error.to_string())?) / f64::from(i16::MAX);
        sample_count += 1;
        square_sum += value * value;
        peak = peak.max(value.abs());
    }

    if sample_count == 0 {
        return Ok(true);
    }

    let rms = (square_sum / sample_count as f64).sqrt();
    Ok(rms < QWEN_SILENT_CHUNK_RMS_THRESHOLD && peak < QWEN_SILENT_CHUNK_PEAK_THRESHOLD)
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

#[derive(Debug, Deserialize)]
struct FasterWhisperWorkerOutput {
    #[serde(default)]
    text: String,
    #[serde(default)]
    segments: Vec<FasterWhisperWorkerSegment>,
}

#[derive(Debug, Deserialize)]
struct FasterWhisperWorkerSegment {
    start: f64,
    end: f64,
    text: String,
}

fn parse_faster_whisper_worker_output(
    raw: &str,
    fallback_duration: f64,
) -> Result<Vec<TranscriptTextChunk>, String> {
    let output: FasterWhisperWorkerOutput = serde_json::from_str(raw)
        .map_err(|error| format!("Invalid Faster-Whisper JSON: {error}"))?;

    let chunks = output
        .segments
        .into_iter()
        .filter_map(|segment| {
            let text = segment.text.trim();
            if text.is_empty() {
                None
            } else {
                Some(TranscriptTextChunk {
                    text: text.to_string(),
                    start: segment.start.max(0.0),
                    end: segment.end.max(segment.start.max(0.0) + 0.1),
                })
            }
        })
        .collect::<Vec<_>>();

    if !chunks.is_empty() {
        return Ok(chunks);
    }

    let text = output.text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }

    Ok(vec![TranscriptTextChunk {
        text: text.to_string(),
        start: 0.0,
        end: fallback_duration.max(0.1),
    }])
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

fn faster_whisper_args_for_engine(
    engine: &InstalledEngineConfig,
    wav_path: &Path,
    output_json_path: &Path,
) -> Result<Vec<String>, String> {
    let mut args = split_command_args(&engine.args)?
        .into_iter()
        .map(|arg| {
            arg.replace("{modelDir}", &engine.model_dir)
                .replace("{audioPath}", &wav_path.to_string_lossy())
                .replace("{outputJson}", &output_json_path.to_string_lossy())
        })
        .collect::<Vec<_>>();

    args.push("--audio".to_string());
    args.push(wav_path.to_string_lossy().to_string());
    args.push("--model-dir".to_string());
    args.push(engine.model_dir.clone());
    args.push("--output-json".to_string());
    args.push(output_json_path.to_string_lossy().to_string());
    Ok(args)
}

fn transcribe_faster_whisper_wav(
    app: &AppHandle,
    engine: &InstalledEngineConfig,
    wav_path: &Path,
    task_id: Option<&str>,
    started_at: Instant,
) -> Result<Vec<TranscriptTextChunk>, String> {
    let executable = PathBuf::from(&engine.executable);
    if !executable.exists() {
        return Err("Faster-Whisper worker executable does not exist.".to_string());
    }

    emit_transcription_progress(
        app,
        task_id,
        started_at,
        "transcribing",
        19,
        "Actual acceleration path: Faster-Whisper worker".to_string(),
        0,
        0,
    );

    let output_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("faster-whisper");
    fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;
    let output_json_path =
        output_dir.join(format!("faster-whisper-{}.json", unix_timestamp_millis()?));

    let mut command = Command::new(&executable);
    command.args(faster_whisper_args_for_engine(
        engine,
        wav_path,
        &output_json_path,
    )?);
    if let Some(parent) = executable.parent() {
        command.current_dir(parent);
    }
    suppress_command_window(&mut command);

    let duration = wav_duration_seconds(wav_path).unwrap_or(60.0);
    let timeout = Duration::from_secs(((duration * 6.0) as u64 + 180).clamp(180, 21_600));
    let output = run_command_with_timeout(&mut command, timeout, "Faster-Whisper transcription")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!(
            "Faster-Whisper worker failed: {}",
            format!("{stdout}\n{stderr}").trim()
        ));
    }

    let raw_json = if output_json_path.exists() {
        fs::read_to_string(&output_json_path).map_err(|error| error.to_string())?
    } else {
        stdout.trim().to_string()
    };
    let _ = fs::remove_file(&output_json_path);

    parse_faster_whisper_worker_output(&raw_json, duration)
}

fn transcribe_sherpa_wav_cli(
    executable: &Path,
    engine: &InstalledEngineConfig,
    wav_path: &Path,
    sherpa_threads: usize,
    runtime_mode: &str,
) -> Result<String, String> {
    let mut command = Command::new(executable);
    command.args(sherpa_args_for_engine_runtime(
        engine,
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
    ensure_transcription_not_cancelled(app, task_id)?;
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
            sherpa_daemon_supported_for_model(engine),
            runtime_mode,
        )?;
        ensure_transcription_not_cancelled(app, task_id)?;
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
    let task_id_owned = task_id.map(str::to_string);
    for _ in 0..worker_count {
        let app = app.clone();
        let queue = Arc::clone(&queue);
        let sender = sender.clone();
        let engine = engine.clone();
        let executable = executable.to_path_buf();
        let runtime_mode = runtime_mode.to_string();
        let task_id = task_id_owned.clone();
        handles.push(thread::spawn(move || loop {
            if transcription_cancelled(&app, task_id.as_deref()) {
                return;
            }
            let next = {
                let Ok(mut queue) = queue.lock() else {
                    return;
                };
                queue.pop_front()
            };
            let Some((index, chunk)) = next else {
                return;
            };
            let result = if transcription_cancelled(&app, task_id.as_deref()) {
                Err("Transcription cancelled.".to_string())
            } else if engine.model_id == "qwen3-asr-0.6b"
                && matches!(wav_likely_silent_for_qwen(&chunk.path), Ok(true))
            {
                Ok(TranscriptTextChunk {
                    text: String::new(),
                    start: chunk.start,
                    end: chunk.end,
                })
            } else {
                transcribe_sherpa_wav_once(
                    &app,
                    &engine,
                    &executable,
                    &chunk.path,
                    performance,
                    sherpa_daemon_supported_for_model(&engine),
                    &runtime_mode,
                )
                .map(|text| TranscriptTextChunk {
                    text,
                    start: chunk.start,
                    end: chunk.end,
                })
            };
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

    ensure_transcription_not_cancelled(app, task_id)?;
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

fn read_protobuf_varint(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;

    while *offset < bytes.len() && shift < 64 {
        let byte = bytes[*offset];
        *offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }

    None
}

fn skip_protobuf_field(bytes: &[u8], offset: &mut usize, wire_type: u64) -> bool {
    match wire_type {
        0 => read_protobuf_varint(bytes, offset).is_some(),
        1 => {
            *offset = offset.saturating_add(8);
            *offset <= bytes.len()
        }
        2 => {
            let Some(length) = read_protobuf_varint(bytes, offset) else {
                return false;
            };
            let Ok(length) = usize::try_from(length) else {
                return false;
            };
            *offset = offset.saturating_add(length);
            *offset <= bytes.len()
        }
        5 => {
            *offset = offset.saturating_add(4);
            *offset <= bytes.len()
        }
        _ => false,
    }
}

fn sentencepiece_piece_from_vocab_message(message: &[u8]) -> Option<String> {
    let mut offset = 0usize;

    while offset < message.len() {
        let key = read_protobuf_varint(message, &mut offset)?;
        let field_number = key >> 3;
        let wire_type = key & 0x07;

        if field_number == 1 && wire_type == 2 {
            let length = usize::try_from(read_protobuf_varint(message, &mut offset)?).ok()?;
            let end = offset.checked_add(length)?;
            if end > message.len() {
                return None;
            }
            return String::from_utf8(message[offset..end].to_vec()).ok();
        }

        if !skip_protobuf_field(message, &mut offset, wire_type) {
            return None;
        }
    }

    None
}

fn load_sentencepiece_pieces(tokenizer_path: &Path) -> Result<Vec<String>, String> {
    let bytes = fs::read(tokenizer_path).map_err(|error| {
        format!(
            "Failed to read SentencePiece tokenizer {}: {error}",
            tokenizer_path.display()
        )
    })?;
    let mut offset = 0usize;
    let mut pieces = Vec::new();

    while offset < bytes.len() {
        let Some(key) = read_protobuf_varint(&bytes, &mut offset) else {
            break;
        };
        let field_number = key >> 3;
        let wire_type = key & 0x07;

        if field_number == 1 && wire_type == 2 {
            let Some(length) = read_protobuf_varint(&bytes, &mut offset) else {
                break;
            };
            let length = usize::try_from(length)
                .map_err(|_| "SentencePiece vocab message is too large.".to_string())?;
            let end = offset
                .checked_add(length)
                .ok_or_else(|| "SentencePiece vocab message length overflowed.".to_string())?;
            if end > bytes.len() {
                return Err("SentencePiece tokenizer ended inside a vocab message.".to_string());
            }
            if let Some(piece) = sentencepiece_piece_from_vocab_message(&bytes[offset..end]) {
                pieces.push(piece);
            }
            offset = end;
            continue;
        }

        if !skip_protobuf_field(&bytes, &mut offset, wire_type) {
            return Err(
                "SentencePiece tokenizer contains an unsupported protobuf field.".to_string(),
            );
        }
    }

    if pieces.is_empty() {
        return Err("SentencePiece tokenizer did not contain any vocab pieces.".to_string());
    }

    Ok(pieces)
}

fn decode_sentencepiece_pieces(token_ids: &[i32], pieces: &[String]) -> String {
    let mut text = String::new();

    for token_id in token_ids {
        let Ok(index) = usize::try_from(*token_id) else {
            continue;
        };
        let Some(piece) = pieces.get(index) else {
            continue;
        };
        text.push_str(&piece.replace('▁', " "));
    }

    text.trim().to_string()
}

fn ctc_greedy_decode_top1(
    topk_indices: &[i32],
    topk_shape: &[usize],
    valid_frames: usize,
    pieces: &[String],
) -> (Vec<i32>, String) {
    let prompt_len = 4usize;
    let blank_id = 0i32;
    let time_steps = topk_shape.get(1).copied().unwrap_or(0);
    let top_k = topk_shape.get(2).copied().unwrap_or(100);
    let end = time_steps.min(valid_frames.saturating_add(prompt_len));
    let mut collapsed = Vec::new();
    let mut previous: Option<i32> = None;

    for frame in prompt_len..end {
        let offset = frame.saturating_mul(top_k);
        let Some(token_id) = topk_indices.get(offset).copied() else {
            break;
        };
        if Some(token_id) == previous {
            continue;
        }
        previous = Some(token_id);
        if token_id != blank_id {
            collapsed.push(token_id);
        }
    }

    let text = decode_sentencepiece_pieces(&collapsed, pieces);
    (collapsed, text)
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

fn read_wav_mono_16k_f32(wav_path: &Path) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(wav_path).map_err(|error| error.to_string())?;
    let spec = reader.spec();
    if spec.sample_rate != 16_000 || spec.channels != 1 {
        return Err(format!(
            "DirectML SenseVoice expects 16kHz mono WAV; got {} Hz / {} channels.",
            spec.sample_rate, spec.channels
        ));
    }

    match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|sample| {
                sample
                    .map(|value| value as f32 / i16::MAX as f32)
                    .map_err(|error| error.to_string())
            })
            .collect(),
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .map(|sample| sample.map_err(|error| error.to_string()))
            .collect(),
        _ => Err(format!(
            "DirectML SenseVoice expects PCM s16 or f32 WAV; got {:?} / {} bits.",
            spec.sample_format, spec.bits_per_sample
        )),
    }
}

fn pad_sensevoice_lfr_features(
    lfr_features: &SenseVoiceLfrFeatures,
    fixed_len: usize,
) -> (Vec<half::f16>, Vec<half::f16>, usize) {
    let valid_frames = lfr_features.frames.min(fixed_len);
    let mut padded_features = vec![half::f16::from_f32(0.0); fixed_len * 560];
    for frame in 0..valid_frames {
        for value_index in 0..560 {
            padded_features[frame * 560 + value_index] =
                half::f16::from_f32(lfr_features.values[frame * 560 + value_index]);
        }
    }
    if valid_frames > 0 {
        let last_source = (valid_frames - 1) * 560;
        for frame in valid_frames..fixed_len {
            for value_index in 0..560 {
                padded_features[frame * 560 + value_index] =
                    half::f16::from_f32(lfr_features.values[last_source + value_index]);
            }
        }
    }
    let mut mask_values = vec![half::f16::from_f32(0.0); fixed_len];
    for value in mask_values.iter_mut().take(valid_frames) {
        *value = half::f16::from_f32(1.0);
    }

    (padded_features, mask_values, valid_frames)
}

fn fft_radix2_in_place(real: &mut [f32], imag: &mut [f32]) {
    let n = real.len();
    debug_assert!(n.is_power_of_two());
    debug_assert_eq!(imag.len(), n);

    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            real.swap(i, j);
            imag.swap(i, j);
        }
    }

    let mut len = 2usize;
    while len <= n {
        let angle = -2.0 * std::f32::consts::PI / len as f32;
        let w_len_real = angle.cos();
        let w_len_imag = angle.sin();

        for start in (0..n).step_by(len) {
            let mut w_real = 1.0f32;
            let mut w_imag = 0.0f32;
            for offset in 0..(len / 2) {
                let even = start + offset;
                let odd = even + len / 2;

                let odd_real = real[odd] * w_real - imag[odd] * w_imag;
                let odd_imag = real[odd] * w_imag + imag[odd] * w_real;
                let even_real = real[even];
                let even_imag = imag[even];

                real[even] = even_real + odd_real;
                imag[even] = even_imag + odd_imag;
                real[odd] = even_real - odd_real;
                imag[odd] = even_imag - odd_imag;

                let next_w_real = w_real * w_len_real - w_imag * w_len_imag;
                w_imag = w_real * w_len_imag + w_imag * w_len_real;
                w_real = next_w_real;
            }
        }

        len <<= 1;
    }
}

#[derive(Debug, Clone)]
struct QwenFbankFeatures {
    frames: usize,
    values: Vec<f32>,
}

fn qwen_hz_to_mel(freq: f32) -> f32 {
    const F_MIN: f32 = 0.0;
    const F_SP: f32 = 200.0 / 3.0;
    const MIN_LOG_HZ: f32 = 1000.0;
    const MIN_LOG_MEL: f32 = (MIN_LOG_HZ - F_MIN) / F_SP;
    const LOG_STEP: f32 = 1.856_297_990_365_626_3 / 27.0;

    if freq >= MIN_LOG_HZ {
        MIN_LOG_MEL + (freq / MIN_LOG_HZ).ln() / LOG_STEP
    } else {
        (freq - F_MIN) / F_SP
    }
}

fn qwen_mel_to_hz(mel: f32) -> f32 {
    const F_MIN: f32 = 0.0;
    const F_SP: f32 = 200.0 / 3.0;
    const MIN_LOG_HZ: f32 = 1000.0;
    const MIN_LOG_MEL: f32 = (MIN_LOG_HZ - F_MIN) / F_SP;
    const LOG_STEP: f32 = 1.856_297_990_365_626_3 / 27.0;

    if mel >= MIN_LOG_MEL {
        MIN_LOG_HZ * ((mel - MIN_LOG_MEL) * LOG_STEP).exp()
    } else {
        F_MIN + F_SP * mel
    }
}

fn qwen_mel_filters(sr: usize, n_fft: usize, n_mels: usize, f_min: f32, f_max: f32) -> Vec<f32> {
    let bins = n_fft / 2 + 1;
    let mel_min = qwen_hz_to_mel(f_min);
    let mel_max = qwen_hz_to_mel(f_max);
    let mel_points = (0..(n_mels + 2))
        .map(|index| mel_min + (mel_max - mel_min) * index as f32 / (n_mels + 1) as f32)
        .map(qwen_mel_to_hz)
        .collect::<Vec<_>>();
    let diffs = mel_points
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect::<Vec<_>>();
    let mut filters = vec![0.0f32; bins * n_mels];

    for bin in 0..bins {
        let freq = bin as f32 * (sr as f32 / 2.0) / (bins - 1) as f32;
        for mel in 0..n_mels {
            let down = if diffs[mel].abs() > f32::EPSILON {
                (freq - mel_points[mel]) / diffs[mel]
            } else {
                0.0
            };
            let up = if diffs[mel + 1].abs() > f32::EPSILON {
                (mel_points[mel + 2] - freq) / diffs[mel + 1]
            } else {
                0.0
            };
            let norm = if (mel_points[mel + 2] - mel_points[mel]).abs() > f32::EPSILON {
                2.0 / (mel_points[mel + 2] - mel_points[mel])
            } else {
                1.0
            };
            filters[bin * n_mels + mel] = down.min(up).max(0.0) * norm;
        }
    }

    filters
}

fn reflected_audio_sample(audio: &[f32], index: isize) -> f32 {
    if audio.is_empty() {
        return 0.0;
    }

    let mut index = index;
    let len = audio.len() as isize;
    while index < 0 || index >= len {
        if index < 0 {
            index = -index - 1;
        } else {
            index = 2 * len - 1 - index;
        }
    }

    audio[index as usize]
}

#[cfg(test)]
fn real_dft_power_spectrum(frame: &[f32]) -> Vec<f32> {
    let n_fft = frame.len();
    let bins = n_fft / 2 + 1;
    let mut power = vec![0.0f32; bins];
    for (bin, value) in power.iter_mut().enumerate() {
        let mut real = 0.0f32;
        let mut imag = 0.0f32;
        for (n, sample) in frame.iter().enumerate() {
            let angle = -2.0 * std::f32::consts::PI * bin as f32 * n as f32 / n_fft as f32;
            real += sample * angle.cos();
            imag += sample * angle.sin();
        }
        *value = real * real + imag * imag;
    }
    power
}

fn whisper_fft_complex(input: &[f32]) -> Vec<(f32, f32)> {
    let n = input.len();
    if n == 1 {
        return vec![(input[0], 0.0)];
    }
    if n % 2 == 1 {
        let mut output = vec![(0.0f32, 0.0f32); n];
        for (k, value) in output.iter_mut().enumerate() {
            let mut real = 0.0f32;
            let mut imag = 0.0f32;
            for (sample_index, sample) in input.iter().enumerate() {
                let angle = 2.0 * std::f32::consts::PI * k as f32 * sample_index as f32 / n as f32;
                real += sample * angle.cos();
                imag -= sample * angle.sin();
            }
            *value = (real, imag);
        }
        return output;
    }

    let even = input.iter().step_by(2).copied().collect::<Vec<_>>();
    let odd = input.iter().skip(1).step_by(2).copied().collect::<Vec<_>>();
    let even_fft = whisper_fft_complex(&even);
    let odd_fft = whisper_fft_complex(&odd);
    let mut output = vec![(0.0f32, 0.0f32); n];

    for k in 0..(n / 2) {
        let theta = 2.0 * std::f32::consts::PI * k as f32 / n as f32;
        let twiddle_real = theta.cos();
        let twiddle_imag = -theta.sin();
        let (odd_real, odd_imag) = odd_fft[k];
        let transformed_real = twiddle_real * odd_real - twiddle_imag * odd_imag;
        let transformed_imag = twiddle_real * odd_imag + twiddle_imag * odd_real;
        let (even_real, even_imag) = even_fft[k];

        output[k] = (even_real + transformed_real, even_imag + transformed_imag);
        output[k + n / 2] = (even_real - transformed_real, even_imag - transformed_imag);
    }

    output
}

fn whisper_fft_power_spectrum(frame: &[f32]) -> Vec<f32> {
    let spectrum = whisper_fft_complex(frame);
    spectrum
        .iter()
        .take(frame.len() / 2 + 1)
        .map(|(real, imag)| real * real + imag * imag)
        .collect()
}

fn extract_qwen_fbank_features(audio: &[f32]) -> QwenFbankFeatures {
    const SAMPLE_RATE: usize = 16_000;
    const N_FFT: usize = 400;
    const HOP_LENGTH: usize = 160;
    const N_MELS: usize = 128;

    let source = if audio.is_empty() {
        &[0.0f32][..]
    } else {
        audio
    };
    let frame_count = ((source.len() + HOP_LENGTH / 2) / HOP_LENGTH).max(1);
    let filters = qwen_mel_filters(SAMPLE_RATE, N_FFT, N_MELS, 0.0, 8000.0);
    let window = (0..N_FFT)
        .map(|index| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * index as f32 / N_FFT as f32).cos())
        .collect::<Vec<_>>();
    let bins = N_FFT / 2 + 1;
    let mut values = vec![0.0f32; frame_count * N_MELS];
    let mut max_log = f32::NEG_INFINITY;

    for frame_index in 0..frame_count {
        let frame_start = frame_index as isize * HOP_LENGTH as isize + HOP_LENGTH as isize / 2
            - N_FFT as isize / 2;
        let mut frame = vec![0.0f32; N_FFT];
        for index in 0..N_FFT {
            frame[index] =
                reflected_audio_sample(source, frame_start + index as isize) * window[index];
        }
        let magnitudes = whisper_fft_power_spectrum(&frame);
        for mel in 0..N_MELS {
            let mut energy = 0.0f32;
            for bin in 0..bins {
                energy += magnitudes[bin] * filters[bin * N_MELS + mel];
            }
            let log_value = energy.max(1.0e-10).log10();
            max_log = max_log.max(log_value);
            values[frame_index * N_MELS + mel] = log_value;
        }
    }

    let floor = max_log - 8.0;
    for value in &mut values {
        *value = (value.max(floor) + 4.0) / 4.0;
    }

    QwenFbankFeatures {
        frames: frame_count,
        values,
    }
}
fn extract_sensevoice_lfr_features(audio: &[f32]) -> SenseVoiceLfrFeatures {
    const SAMPLE_RATE: usize = 16_000;
    const WINDOW_LENGTH: usize = 400;
    const FFT_LENGTH: usize = 512;
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

    let half_n_fft = WINDOW_LENGTH / 2;
    let mut padded = Vec::with_capacity(emphasized.len() + WINDOW_LENGTH);
    padded.extend(std::iter::repeat(0.0f32).take(half_n_fft));
    padded.extend_from_slice(&emphasized);
    padded.extend(std::iter::repeat(0.0f32).take(half_n_fft));

    let frame_count = if padded.len() >= WINDOW_LENGTH {
        1 + (padded.len() - WINDOW_LENGTH) / HOP_LENGTH
    } else {
        1
    };
    let filters = sensevoice_mel_filters(SAMPLE_RATE, FFT_LENGTH, N_MELS, 20.0, 8000.0);
    let window = (0..WINDOW_LENGTH)
        .map(|index| {
            0.54 - 0.46 * (2.0 * std::f32::consts::PI * index as f32 / WINDOW_LENGTH as f32).cos()
        })
        .collect::<Vec<_>>();

    let bins = FFT_LENGTH / 2 + 1;
    let mut log_mel = vec![0.0f32; frame_count * N_MELS];
    let mut magnitudes = vec![0.0f32; bins];
    let mut fft_real = vec![0.0f32; FFT_LENGTH];
    let mut fft_imag = vec![0.0f32; FFT_LENGTH];

    for frame_index in 0..frame_count {
        let offset = frame_index * HOP_LENGTH;
        fft_real.fill(0.0);
        fft_imag.fill(0.0);
        for index in 0..WINDOW_LENGTH {
            fft_real[index] = padded.get(offset + index).copied().unwrap_or(0.0) * window[index];
        }
        fft_radix2_in_place(&mut fft_real, &mut fft_imag);
        for bin in 0..bins {
            magnitudes[bin] = fft_real[bin] * fft_real[bin] + fft_imag[bin] * fft_imag[bin];
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

fn cpu_session_builder() -> Result<ort::session::builder::SessionBuilder, String> {
    ort::session::Session::builder()
        .map_err(|error| format!("ONNX Runtime CPU session builder failed: {error}"))?
        .with_no_environment_execution_providers()
        .map_err(|error| format!("Failed to isolate execution providers: {error}"))?
        .with_intra_threads(1)
        .map_err(|error| format!("Failed to set ONNX Runtime thread count: {error}"))?
        .with_parallel_execution(false)
        .map_err(|error| format!("Failed to set ONNX Runtime execution mode: {error}"))
}

fn qwen_smoke_session_builder(
    use_directml: bool,
) -> Result<ort::session::builder::SessionBuilder, String> {
    if use_directml {
        directml_session_builder()
    } else {
        cpu_session_builder()
    }
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

fn run_directml_sensevoice_lfr_chunk(
    encoder_session: &mut ort::session::Session,
    ctc_session: &mut ort::session::Session,
    pieces: &[String],
    lfr_features: &SenseVoiceLfrFeatures,
) -> Result<(String, Vec<i32>, usize), String> {
    use half::f16;
    use ort::{inputs, value::Tensor};

    let fixed_len = 30usize * 17;
    let (padded_features, mask_values, valid_frames) =
        pad_sensevoice_lfr_features(lfr_features, fixed_len);
    if valid_frames == 0 {
        return Ok((String::new(), Vec::new(), 0));
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

    let enc_out = Tensor::<f16>::from_array((encoder_shape_usize, encoder_data))
        .map_err(|error| format!("Failed to create CTC enc_out tensor: {error}"))?;
    let ctc_outputs = ctc_session
        .run(inputs!["enc_out" => enc_out])
        .map_err(|error| format!("DirectML split SenseVoice CTC run failed: {error}"))?;
    let (indices_shape, topk_indices) = ctc_outputs[1]
        .try_extract_tensor::<i32>()
        .map_err(|error| format!("Failed to extract CTC topk_indices tensor: {error}"))?;
    let topk_shape = indices_shape
        .iter()
        .map(|dim| usize::try_from(*dim).map_err(|_| format!("Invalid CTC topk dim: {dim}")))
        .collect::<Result<Vec<_>, _>>()?;
    let (collapsed_ids, decoded_text) =
        ctc_greedy_decode_top1(&topk_indices, &topk_shape, valid_frames, pieces);
    drop(ctc_outputs);

    Ok((decoded_text, collapsed_ids, valid_frames))
}

fn create_directml_sensevoice_wav_transcription_session(
    wav_path: &Path,
    encoder_path: &Path,
    ctc_path: &Path,
    tokenizer_path: &Path,
) -> Result<DirectMlSessionProbeCliResult, String> {
    if !cfg!(windows) {
        return Err("DirectML is only available on Windows.".to_string());
    }

    let pieces = load_sentencepiece_pieces(tokenizer_path)?;
    let mut encoder_session = directml_session_builder()?
        .commit_from_file(encoder_path)
        .map_err(|error| {
            format!("Failed to create DirectML split SenseVoice encoder session: {error}")
        })?;
    let encoder_inputs_summary = ort_outlet_summaries(encoder_session.inputs());
    let encoder_outputs_summary = ort_outlet_summaries(encoder_session.outputs());
    let mut ctc_session = directml_session_builder()?
        .commit_from_file(ctc_path)
        .map_err(|error| {
            format!("Failed to create DirectML split SenseVoice CTC session: {error}")
        })?;
    let ctc_inputs_summary = ort_outlet_summaries(ctc_session.inputs());
    let ctc_outputs_summary = ort_outlet_summaries(ctc_session.outputs());

    let audio = read_wav_mono_16k_f32(wav_path)?;
    let chunk_samples = 30usize * 16_000;
    let total_chunks = audio.len().div_ceil(chunk_samples).max(1);
    let mut text_chunks = Vec::new();

    let mut total_feature_ms = 0u128;
    let mut total_inference_ms = 0u128;

    for chunk_audio in audio.chunks(chunk_samples) {
        let feature_started_at = Instant::now();
        let lfr_features = extract_sensevoice_lfr_features(chunk_audio);
        total_feature_ms += feature_started_at.elapsed().as_millis();

        let inference_started_at = Instant::now();
        let (text, _collapsed_ids, _valid_frames) = run_directml_sensevoice_lfr_chunk(
            &mut encoder_session,
            &mut ctc_session,
            &pieces,
            &lfr_features,
        )?;
        total_inference_ms += inference_started_at.elapsed().as_millis();

        let text = clean_subtitle_text_one_line(&text);
        if !text.is_empty() {
            text_chunks.push(text);
        }
    }

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
            "DirectML split SenseVoice WAV transcription completed; chunks: {total_chunks}; feature {total_feature_ms} ms; inference {total_inference_ms} ms; decoded text: {}",
            text_chunks.join("\n")
        ),
        model_inputs,
        model_outputs,
        onnx_runtime_build: Some(ort::info().to_string()),
    })
}

fn create_directml_sensevoice_chain_smoke_session(
    encoder_path: &Path,
    ctc_path: &Path,
    tokenizer_path: &Path,
) -> Result<DirectMlSessionProbeCliResult, String> {
    if !cfg!(windows) {
        return Err("DirectML is only available on Windows.".to_string());
    }

    use half::f16;
    use ort::{inputs, value::Tensor};

    let pieces = load_sentencepiece_pieces(tokenizer_path)?;

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
    let (indices_shape, topk_indices) = ctc_outputs[1]
        .try_extract_tensor::<i32>()
        .map_err(|error| format!("Failed to extract CTC topk_indices tensor: {error}"))?;
    let topk_shape = indices_shape
        .iter()
        .map(|dim| usize::try_from(*dim).map_err(|_| format!("Invalid CTC topk dim: {dim}")))
        .collect::<Result<Vec<_>, _>>()?;
    let (collapsed_ids, decoded_text) =
        ctc_greedy_decode_top1(&topk_indices, &topk_shape, valid_frames, &pieces);
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
            "DirectML split SenseVoice encoder->CTC smoke completed; LFR frames: {valid_frames}; top1 token ids: {}; collapsed ids: {}; decoded text: {}",
            token_preview.join(", "),
            collapsed_ids
                .iter()
                .map(|token| token.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            decoded_text
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
        "--hi-voicer-directml-sensevoice-transcribe-wav" => {
            suppress_windows_fault_dialogs();
            let Some(wav_path) = args.next() else {
                eprintln!("missing WAV path");
                std::process::exit(2);
            };
            let Some(encoder_path) = args.next() else {
                eprintln!("missing encoder path");
                std::process::exit(2);
            };
            let Some(ctc_path) = args.next() else {
                eprintln!("missing CTC path");
                std::process::exit(2);
            };
            let Some(tokenizer_path) = args.next() else {
                eprintln!("missing tokenizer path");
                std::process::exit(2);
            };
            create_directml_sensevoice_wav_transcription_session(
                &PathBuf::from(wav_path),
                &PathBuf::from(encoder_path),
                &PathBuf::from(ctc_path),
                &PathBuf::from(tokenizer_path),
            )
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
            let Some(tokenizer_path) = args.next() else {
                eprintln!("missing tokenizer path");
                std::process::exit(2);
            };
            create_directml_sensevoice_chain_smoke_session(
                &PathBuf::from(encoder_path),
                &PathBuf::from(ctc_path),
                &PathBuf::from(tokenizer_path),
            )
        }
        "--hi-voicer-directml-qwen-chain-smoke" => {
            suppress_windows_fault_dialogs();
            let Some(conv_frontend_path) = args.next() else {
                eprintln!("missing conv_frontend path");
                std::process::exit(2);
            };
            let Some(encoder_path) = args.next() else {
                eprintln!("missing encoder path");
                std::process::exit(2);
            };
            let Some(decoder_path) = args.next() else {
                eprintln!("missing decoder path");
                std::process::exit(2);
            };
            create_directml_qwen_chain_smoke_session(
                &PathBuf::from(conv_frontend_path),
                &PathBuf::from(encoder_path),
                &PathBuf::from(decoder_path),
            )
        }
        "--hi-voicer-directml-qwen-wav-smoke" => {
            suppress_windows_fault_dialogs();
            let Some(conv_frontend_path) = args.next() else {
                eprintln!("missing conv_frontend path");
                std::process::exit(2);
            };
            let Some(encoder_path) = args.next() else {
                eprintln!("missing encoder path");
                std::process::exit(2);
            };
            let Some(decoder_path) = args.next() else {
                eprintln!("missing decoder path");
                std::process::exit(2);
            };
            let Some(wav_path) = args.next() else {
                eprintln!("missing wav path");
                std::process::exit(2);
            };
            create_directml_qwen_wav_smoke_session(
                &PathBuf::from(conv_frontend_path),
                &PathBuf::from(encoder_path),
                &PathBuf::from(decoder_path),
                &PathBuf::from(wav_path),
            )
        }
        "--hi-voicer-directml-qwen-greedy-wav-smoke" => {
            suppress_windows_fault_dialogs();
            let Some(conv_frontend_path) = args.next() else {
                eprintln!("missing conv_frontend path");
                std::process::exit(2);
            };
            let Some(encoder_path) = args.next() else {
                eprintln!("missing encoder path");
                std::process::exit(2);
            };
            let Some(decoder_path) = args.next() else {
                eprintln!("missing decoder path");
                std::process::exit(2);
            };
            let Some(wav_path) = args.next() else {
                eprintln!("missing wav path");
                std::process::exit(2);
            };
            let max_new_tokens = match args.next() {
                Some(value) => match value.to_string_lossy().parse::<usize>() {
                    Ok(value) => Ok(Some(value)),
                    Err(error) => Err(format!(
                        "invalid max_new_tokens for Qwen greedy smoke: {error}"
                    )),
                },
                None => Ok(None),
            };
            match max_new_tokens {
                Ok(max_new_tokens) => create_directml_qwen_greedy_wav_smoke_session(
                    &PathBuf::from(conv_frontend_path),
                    &PathBuf::from(encoder_path),
                    &PathBuf::from(decoder_path),
                    &PathBuf::from(wav_path),
                    max_new_tokens.unwrap_or(8),
                ),
                Err(error) => Err(error),
            }
        }
        "--hi-voicer-cpu-qwen-greedy-wav-smoke" => {
            suppress_windows_fault_dialogs();
            let Some(conv_frontend_path) = args.next() else {
                eprintln!("missing conv_frontend path");
                std::process::exit(2);
            };
            let Some(encoder_path) = args.next() else {
                eprintln!("missing encoder path");
                std::process::exit(2);
            };
            let Some(decoder_path) = args.next() else {
                eprintln!("missing decoder path");
                std::process::exit(2);
            };
            let Some(wav_path) = args.next() else {
                eprintln!("missing wav path");
                std::process::exit(2);
            };
            let max_new_tokens = match args.next() {
                Some(value) => match value.to_string_lossy().parse::<usize>() {
                    Ok(value) => Ok(Some(value)),
                    Err(error) => Err(format!(
                        "invalid max_new_tokens for Qwen greedy smoke: {error}"
                    )),
                },
                None => Ok(None),
            };
            match max_new_tokens {
                Ok(max_new_tokens) => create_cpu_qwen_greedy_wav_smoke_session(
                    &PathBuf::from(conv_frontend_path),
                    &PathBuf::from(encoder_path),
                    &PathBuf::from(decoder_path),
                    &PathBuf::from(wav_path),
                    max_new_tokens.unwrap_or(8),
                ),
                Err(error) => Err(error),
            }
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

#[derive(Debug, Clone)]
struct DirectMlQwenCandidate {
    conv_frontend: PathBuf,
    encoder: PathBuf,
    decoder: PathBuf,
    tokenizer_dir: PathBuf,
}

fn qwen_candidate_for_dir(model_dir: &Path) -> DirectMlQwenCandidate {
    DirectMlQwenCandidate {
        conv_frontend: model_dir.join("conv_frontend.onnx"),
        encoder: model_dir.join("encoder.int8.onnx"),
        decoder: model_dir.join("decoder.int8.onnx"),
        tokenizer_dir: model_dir.join("tokenizer"),
    }
}

fn qwen_missing_files(candidate: &DirectMlQwenCandidate) -> Vec<String> {
    let required = [
        ("conv_frontend.onnx", &candidate.conv_frontend),
        ("encoder.int8.onnx", &candidate.encoder),
        ("decoder.int8.onnx", &candidate.decoder),
        (
            "tokenizer/merges.txt",
            &candidate.tokenizer_dir.join("merges.txt"),
        ),
        (
            "tokenizer/tokenizer_config.json",
            &candidate.tokenizer_dir.join("tokenizer_config.json"),
        ),
        (
            "tokenizer/vocab.json",
            &candidate.tokenizer_dir.join("vocab.json"),
        ),
    ];
    required
        .iter()
        .filter_map(|(name, path)| (!path.exists()).then(|| (*name).to_string()))
        .collect()
}

fn directml_qwen_ready(model_path: &Path) -> bool {
    qwen_missing_files(&qwen_candidate_for_dir(model_path)).is_empty()
}

fn is_qwen_model_layout(model_path: &Path, missing_files: &[String]) -> bool {
    let dir_name_matches = model_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case("qwen3-asr-0.6b"))
        .unwrap_or(false);
    dir_name_matches || missing_files.is_empty()
}

fn create_directml_qwen_chain_smoke_session(
    conv_frontend_path: &Path,
    encoder_path: &Path,
    decoder_path: &Path,
) -> Result<DirectMlSessionProbeCliResult, String> {
    let input_frames = 120usize;
    let input_features = vec![0.0f32; input_frames * 128];
    create_directml_qwen_chain_smoke_session_with_features(
        conv_frontend_path,
        encoder_path,
        decoder_path,
        input_frames,
        input_features,
        "zero fbank",
    )
}

fn create_directml_qwen_wav_smoke_session(
    conv_frontend_path: &Path,
    encoder_path: &Path,
    decoder_path: &Path,
    wav_path: &Path,
) -> Result<DirectMlSessionProbeCliResult, String> {
    let feature_started_at = Instant::now();
    let audio = read_wav_mono_16k_f32(wav_path)?;
    let features = extract_qwen_fbank_features(&audio);
    let feature_ms = feature_started_at.elapsed().as_millis();
    let mut result = create_directml_qwen_chain_smoke_session_with_features(
        conv_frontend_path,
        encoder_path,
        decoder_path,
        features.frames,
        features.values,
        "wav fbank",
    )?;
    result.message = format!(
        "{}; wav samples: {}; feature extraction: {} ms",
        result.message,
        audio.len(),
        feature_ms
    );
    Ok(result)
}

fn load_qwen_token_ids(tokenizer_dir: &Path) -> Result<HashMap<String, i64>, String> {
    let vocab_path = tokenizer_dir.join("vocab.json");
    let config_path = tokenizer_dir.join("tokenizer_config.json");
    let vocab_text = fs::read_to_string(&vocab_path).map_err(|error| {
        format!(
            "Failed to read Qwen vocab {}: {error}",
            vocab_path.display()
        )
    })?;
    let config_text = fs::read_to_string(&config_path).map_err(|error| {
        format!(
            "Failed to read Qwen tokenizer config {}: {error}",
            config_path.display()
        )
    })?;
    let vocab_json: serde_json::Value = serde_json::from_str(&vocab_text)
        .map_err(|error| format!("Failed to parse Qwen vocab JSON: {error}"))?;
    let config_json: serde_json::Value = serde_json::from_str(&config_text)
        .map_err(|error| format!("Failed to parse Qwen tokenizer config JSON: {error}"))?;
    let mut ids = HashMap::new();

    if let Some(vocab) = vocab_json.as_object() {
        for (token, value) in vocab {
            if let Some(id) = value.as_i64() {
                ids.insert(token.clone(), id);
            }
        }
    }
    if let Some(added) = config_json
        .get("added_tokens_decoder")
        .and_then(|value| value.as_object())
    {
        for (key, value) in added {
            let Some(token) = value.get("content").and_then(|item| item.as_str()) else {
                continue;
            };
            let id = value
                .get("id")
                .and_then(|item| item.as_i64())
                .or_else(|| key.parse::<i64>().ok());
            if let Some(id) = id {
                ids.insert(token.to_string(), id);
            }
        }
    }

    Ok(ids)
}

fn qwen_required_token_id(ids: &HashMap<String, i64>, token: &str) -> Result<i64, String> {
    ids.get(token)
        .copied()
        .ok_or_else(|| format!("Qwen tokenizer is missing required token: {token}"))
}

fn qwen_utf8_encode(code_point: u32) -> String {
    char::from_u32(code_point)
        .unwrap_or(char::REPLACEMENT_CHARACTER)
        .to_string()
}

fn qwen_byte_to_unicode() -> Vec<String> {
    let mut used = [false; 256];
    let mut bytes = Vec::with_capacity(256);
    let mut code_points = Vec::with_capacity(256);

    for byte in 33u16..=126 {
        bytes.push(byte as u8);
        used[byte as usize] = true;
    }
    for byte in 161u16..=172 {
        bytes.push(byte as u8);
        used[byte as usize] = true;
    }
    for byte in 174u16..=255 {
        bytes.push(byte as u8);
        used[byte as usize] = true;
    }
    for byte in &bytes {
        code_points.push(*byte as u32);
    }

    let mut n = 0u32;
    for byte in 0u16..=255 {
        if !used[byte as usize] {
            bytes.push(byte as u8);
            code_points.push(256 + n);
            n += 1;
        }
    }

    let mut table = vec![String::new(); 256];
    for (byte, code_point) in bytes.into_iter().zip(code_points.into_iter()) {
        table[byte as usize] = qwen_utf8_encode(code_point);
    }
    table
}

fn qwen_unicode_to_byte() -> HashMap<String, u8> {
    qwen_byte_to_unicode()
        .into_iter()
        .enumerate()
        .map(|(byte, piece)| (piece, byte as u8))
        .collect()
}

fn is_qwen_special_token(token: &str) -> bool {
    token.starts_with("<|") && token.ends_with("|>")
}

fn is_qwen_skippable_special_token(token: &str) -> bool {
    matches!(token, "<|im_start|>" | "<|im_end|>")
}

fn decode_qwen_byte_level_piece(piece: &str, unicode_to_byte: &HashMap<String, u8>) -> Vec<u8> {
    let mut bytes = Vec::new();
    for ch in piece.chars() {
        let item = ch.to_string();
        if let Some(byte) = unicode_to_byte.get(&item) {
            bytes.push(*byte);
        } else {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    bytes
}

fn load_qwen_id_to_token(tokenizer_dir: &Path) -> Result<Vec<String>, String> {
    let ids = load_qwen_token_ids(tokenizer_dir)?;
    let max_id = ids
        .values()
        .copied()
        .filter(|id| *id >= 0)
        .max()
        .unwrap_or(0) as usize;
    let mut id_to_token = vec![String::new(); max_id + 1];
    for (token, id) in ids {
        if id >= 0 {
            let index = id as usize;
            if index >= id_to_token.len() {
                id_to_token.resize(index + 1, String::new());
            }
            id_to_token[index] = token;
        }
    }
    Ok(id_to_token)
}

fn decode_qwen_token_ids(tokenizer_dir: &Path, token_ids: &[i64]) -> Result<String, String> {
    let id_to_token = load_qwen_id_to_token(tokenizer_dir)?;
    let unicode_to_byte = qwen_unicode_to_byte();
    let mut output = String::new();
    let mut pending = Vec::new();

    for id in token_ids {
        if *id < 0 {
            continue;
        }
        let Some(token) = id_to_token.get(*id as usize) else {
            continue;
        };
        if token.is_empty() {
            continue;
        }
        if is_qwen_special_token(token) {
            if !pending.is_empty() {
                output.push_str(&String::from_utf8_lossy(&pending));
                pending.clear();
            }
            if !is_qwen_skippable_special_token(token) {
                output.push_str(token);
            }
        } else {
            pending.extend(decode_qwen_byte_level_piece(token, &unicode_to_byte));
        }
    }

    if !pending.is_empty() {
        output.push_str(&String::from_utf8_lossy(&pending));
    }

    Ok(output.replace(char::REPLACEMENT_CHARACTER, ""))
}

fn clean_qwen_generated_text(
    tokenizer_dir: &Path,
    generated_ids: &[i64],
) -> Result<String, String> {
    let ids = load_qwen_token_ids(tokenizer_dir)?;
    let asr_text_id = ids.get("<asr_text>").copied();
    let mut cleaned_ids = generated_ids.to_vec();

    if let Some(asr_text_id) = asr_text_id {
        let prefix_window = cleaned_ids.len().min(16);
        if let Some(position) = cleaned_ids
            .iter()
            .take(prefix_window)
            .position(|id| *id == asr_text_id)
        {
            if position > 0 {
                let prefix = decode_qwen_token_ids(tokenizer_dir, &cleaned_ids[..=position])?;
                if prefix.starts_with("language ") && prefix.ends_with("<asr_text>") {
                    cleaned_ids = cleaned_ids[position + 1..].to_vec();
                }
            }
        }
    }

    let mut text = decode_qwen_token_ids(tokenizer_dir, &cleaned_ids)?;
    if let Some(position) = text.find("<asr_text>") {
        text = text[position + "<asr_text>".len()..].to_string();
    }
    Ok(text)
}
fn build_qwen_default_source_ids(
    tokenizer_dir: &Path,
    audio_token_len: usize,
) -> Result<Vec<i64>, String> {
    let ids = load_qwen_token_ids(tokenizer_dir)?;
    let newline = char::from_u32(0x010A)
        .ok_or_else(|| "Failed to construct Qwen byte-level newline token.".to_string())?
        .to_string();
    let im_start = qwen_required_token_id(&ids, "<|im_start|>")?;
    let im_end = qwen_required_token_id(&ids, "<|im_end|>")?;
    let audio_start = qwen_required_token_id(&ids, "<|audio_start|>")?;
    let audio_pad = qwen_required_token_id(&ids, "<|audio_pad|>")?;
    let audio_end = qwen_required_token_id(&ids, "<|audio_end|>")?;
    let system = qwen_required_token_id(&ids, "system")?;
    let user = qwen_required_token_id(&ids, "user")?;
    let assistant = qwen_required_token_id(&ids, "assistant")?;
    let nl = qwen_required_token_id(&ids, &newline)?;

    let mut source_ids = Vec::with_capacity(16 + audio_token_len);
    source_ids.extend([
        im_start,
        system,
        nl,
        im_end,
        nl,
        im_start,
        user,
        nl,
        audio_start,
    ]);
    source_ids.extend(std::iter::repeat(audio_pad).take(audio_token_len));
    source_ids.extend([audio_end, im_end, nl, im_start, assistant, nl]);
    Ok(source_ids)
}

fn qwen_argmax_token_from_logits(
    logits: &[f32],
    logits_shape: &[i64],
    time_index: usize,
) -> Result<i64, String> {
    if logits_shape.len() < 3 {
        return Err("Qwen decoder logits tensor must be at least 3-D.".to_string());
    }
    let time_dim = usize::try_from(logits_shape[1])
        .map_err(|_| format!("Invalid Qwen logits time dimension: {}", logits_shape[1]))?;
    let vocab_size = usize::try_from(logits_shape[2])
        .map_err(|_| format!("Invalid Qwen logits vocab dimension: {}", logits_shape[2]))?;
    if time_index >= time_dim || vocab_size == 0 {
        return Err(format!(
            "Qwen decoder logits index is invalid: time_index {time_index}, shape {:?}",
            logits_shape
        ));
    }
    let start = time_index
        .checked_mul(vocab_size)
        .ok_or_else(|| "Qwen logits offset overflowed.".to_string())?;
    let end = start
        .checked_add(vocab_size)
        .ok_or_else(|| "Qwen logits end offset overflowed.".to_string())?;
    let row = logits
        .get(start..end)
        .ok_or_else(|| "Qwen logits tensor is shorter than its shape.".to_string())?;

    row.iter()
        .enumerate()
        .filter(|(_, value)| value.is_finite())
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .map(|(index, _)| index as i64)
        .ok_or_else(|| "Qwen decoder logits row has no finite values.".to_string())
}

fn copy_qwen_cache_delta(target: &mut [f32], start_position: usize, delta: &[f32], seq_len: usize) {
    const QWEN_KV_WIDTH: usize = 8 * 128;
    let target_start = start_position.saturating_mul(QWEN_KV_WIDTH);
    let copy_len = seq_len
        .saturating_mul(QWEN_KV_WIDTH)
        .min(delta.len())
        .min(target.len().saturating_sub(target_start));
    if copy_len > 0 {
        target[target_start..target_start + copy_len].copy_from_slice(&delta[..copy_len]);
    }
}

fn qwen_feat_to_audio_tokens_len(feat_len: usize, chunk_size: usize) -> usize {
    if feat_len == 0 || chunk_size == 0 {
        return 0;
    }

    fn conv_out_len_3x_stride2(mut n: usize) -> usize {
        n = (n + 1) / 2;
        n = (n + 1) / 2;
        (n + 1) / 2
    }

    fn after_cnn(mut n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        n = (n - 1) / 2 + 1;
        n = (n - 1) / 2 + 1;
        (n - 1) / 2 + 1
    }

    let full = feat_len / chunk_size;
    let rem = feat_len % chunk_size;
    let mut out = full * conv_out_len_3x_stride2(chunk_size);
    if rem > 0 {
        out += after_cnn(rem);
    }
    out
}

fn qwen_valid_audio_tokens(feat_frames: usize, conv_frames: usize) -> usize {
    qwen_feat_to_audio_tokens_len(feat_frames, 100).min(conv_frames)
}

fn trim_qwen_audio_features(shape: &mut Vec<usize>, data: &mut Vec<f32>) -> usize {
    if shape.len() != 3 || shape[0] != 1 || shape[1] == 0 || shape[2] == 0 {
        return shape.get(1).copied().unwrap_or(0);
    }

    let frames = shape[1];
    let hidden = shape[2];
    let mut valid_frames = frames;
    while valid_frames > 0 {
        let start = (valid_frames - 1) * hidden;
        let end = start + hidden;
        let max_abs = data[start..end]
            .iter()
            .map(|value| value.abs())
            .fold(0.0f32, f32::max);
        if max_abs > 1e-6 {
            break;
        }
        valid_frames -= 1;
    }

    truncate_qwen_audio_features(shape, data, valid_frames)
}

fn truncate_qwen_audio_features(
    shape: &mut Vec<usize>,
    data: &mut Vec<f32>,
    keep_frames: usize,
) -> usize {
    if shape.len() != 3 || shape[0] != 1 || shape[1] == 0 || shape[2] == 0 {
        return shape.get(1).copied().unwrap_or(0);
    }

    let frames = shape[1];
    let hidden = shape[2];
    let keep_frames = keep_frames.min(frames);
    if keep_frames > 0 && keep_frames < frames {
        data.truncate(keep_frames * hidden);
        shape[1] = keep_frames;
    }

    shape[1]
}
fn create_directml_qwen_greedy_wav_smoke_session(
    conv_frontend_path: &Path,
    encoder_path: &Path,
    decoder_path: &Path,
    wav_path: &Path,
    max_new_tokens: usize,
) -> Result<DirectMlSessionProbeCliResult, String> {
    create_qwen_greedy_wav_smoke_session(
        conv_frontend_path,
        encoder_path,
        decoder_path,
        wav_path,
        max_new_tokens,
        true,
    )
}

fn create_cpu_qwen_greedy_wav_smoke_session(
    conv_frontend_path: &Path,
    encoder_path: &Path,
    decoder_path: &Path,
    wav_path: &Path,
    max_new_tokens: usize,
) -> Result<DirectMlSessionProbeCliResult, String> {
    create_qwen_greedy_wav_smoke_session(
        conv_frontend_path,
        encoder_path,
        decoder_path,
        wav_path,
        max_new_tokens,
        false,
    )
}
fn create_qwen_greedy_wav_smoke_session(
    conv_frontend_path: &Path,
    encoder_path: &Path,
    decoder_path: &Path,
    wav_path: &Path,
    max_new_tokens: usize,
    use_directml: bool,
) -> Result<DirectMlSessionProbeCliResult, String> {
    if use_directml && !cfg!(windows) {
        return Err("DirectML is only available on Windows.".to_string());
    }

    use ort::{
        inputs,
        value::{Tensor, TensorRef},
    };

    let provider_label = if use_directml { "DirectML" } else { "CPU" };
    let total_started_at = Instant::now();
    let feature_started_at = Instant::now();
    let audio = read_wav_mono_16k_f32(wav_path)?;
    let features = extract_qwen_fbank_features(&audio);
    let feature_ms = feature_started_at.elapsed().as_millis();

    let mut conv_session = qwen_smoke_session_builder(use_directml)?
        .commit_from_file(conv_frontend_path)
        .map_err(|error| {
            format!("Failed to create {provider_label} Qwen conv_frontend session: {error}")
        })?;
    let conv_inputs_summary = ort_outlet_summaries(conv_session.inputs());
    let conv_outputs_summary = ort_outlet_summaries(conv_session.outputs());
    let conv_started_at = Instant::now();
    let input_features =
        Tensor::<f32>::from_array(([1usize, features.frames, 128], features.values))
            .map_err(|error| format!("Failed to create Qwen input_features tensor: {error}"))?;
    let conv_outputs = conv_session
        .run(inputs!["input_features" => input_features])
        .map_err(|error| format!("{provider_label} Qwen conv_frontend run failed: {error}"))?;
    let conv_ms = conv_started_at.elapsed().as_millis();
    let (conv_shape, conv_data) = conv_outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|error| format!("Failed to extract Qwen conv output tensor: {error}"))?;
    let conv_shape_usize = conv_shape
        .iter()
        .map(|dim| {
            usize::try_from(*dim).map_err(|_| format!("Invalid Qwen conv output dim: {dim}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let conv_frames = conv_shape_usize.get(1).copied().unwrap_or(0).max(1);
    let valid_audio_tokens = qwen_valid_audio_tokens(features.frames, conv_frames);
    let conv_data = conv_data.to_vec();
    drop(conv_outputs);
    drop(conv_session);

    let mut encoder_session = qwen_smoke_session_builder(use_directml)?
        .commit_from_file(encoder_path)
        .map_err(|error| {
            format!("Failed to create {provider_label} Qwen encoder session: {error}")
        })?;
    let encoder_inputs_summary = ort_outlet_summaries(encoder_session.inputs());
    let encoder_outputs_summary = ort_outlet_summaries(encoder_session.outputs());
    let encoder_features = Tensor::<f32>::from_array((conv_shape_usize, conv_data))
        .map_err(|error| format!("Failed to create Qwen encoder input_features tensor: {error}"))?;
    let mut feature_attention_mask_values = vec![false; conv_frames];
    for value in feature_attention_mask_values
        .iter_mut()
        .take(valid_audio_tokens.min(conv_frames))
    {
        *value = true;
    }
    let feature_attention_mask =
        Tensor::<bool>::from_array(([1usize, conv_frames], feature_attention_mask_values))
            .map_err(|error| {
                format!("Failed to create Qwen feature_attention_mask tensor: {error}")
            })?;
    let encoder_started_at = Instant::now();
    let encoder_outputs = encoder_session
        .run(inputs![
            "input_features" => encoder_features,
            "feature_attention_mask" => feature_attention_mask,
        ])
        .map_err(|error| format!("{provider_label} Qwen encoder run failed: {error}"))?;
    let encoder_ms = encoder_started_at.elapsed().as_millis();
    let (audio_shape, audio_data) = encoder_outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|error| format!("Failed to extract Qwen encoder output tensor: {error}"))?;
    let audio_shape_usize = audio_shape
        .iter()
        .map(|dim| {
            usize::try_from(*dim).map_err(|_| format!("Invalid Qwen audio feature dim: {dim}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut audio_shape_usize = audio_shape_usize;
    let mut audio_data = audio_data.to_vec();
    let trimmed_audio_frames = trim_qwen_audio_features(&mut audio_shape_usize, &mut audio_data);
    let audio_tokens = valid_audio_tokens.min(trimmed_audio_frames).max(1);
    let audio_feature_frames =
        truncate_qwen_audio_features(&mut audio_shape_usize, &mut audio_data, audio_tokens);
    drop(encoder_outputs);
    drop(encoder_session);

    let mut decoder_session = qwen_smoke_session_builder(use_directml)?
        .commit_from_file(decoder_path)
        .map_err(|error| {
            format!("Failed to create {provider_label} Qwen decoder session: {error}")
        })?;
    let decoder_inputs_summary = ort_outlet_summaries(decoder_session.inputs());
    let decoder_outputs_summary = ort_outlet_summaries(decoder_session.outputs());

    let tokenizer_dir = decoder_path
        .parent()
        .ok_or_else(|| "Qwen decoder path does not have a parent directory.".to_string())?
        .join("tokenizer");
    let prompt_ids = build_qwen_default_source_ids(&tokenizer_dir, audio_tokens)?;
    let context_len = prompt_ids.len();
    let max_total_len = 512usize.max(context_len + max_new_tokens + 8);
    let cache_values_per_layer = max_total_len * 8 * 128;
    let mut cache_keys = vec![vec![0.0f32; cache_values_per_layer]; 28];
    let mut cache_values = vec![vec![0.0f32; cache_values_per_layer]; 28];
    let mut generated = Vec::new();
    let mut step_timings = Vec::new();
    let mut total_steps = 0usize;

    let mut prefill_inputs: Vec<(String, ort::session::SessionInputValue)> = Vec::new();
    prefill_inputs.push((
        "input_ids".to_string(),
        ort::session::SessionInputValue::Owned(
            Tensor::<i64>::from_array(([1usize, context_len], prompt_ids.clone()))
                .map_err(|error| {
                    format!("Failed to create Qwen prefill input_ids tensor: {error}")
                })?
                .into_dyn(),
        ),
    ));
    prefill_inputs.push((
        "audio_features".to_string(),
        ort::session::SessionInputValue::Owned(
            Tensor::<f32>::from_array((audio_shape_usize.clone(), audio_data.clone()))
                .map_err(|error| {
                    format!("Failed to create Qwen prefill audio_features tensor: {error}")
                })?
                .into_dyn(),
        ),
    ));
    prefill_inputs.push((
        "attention_mask".to_string(),
        ort::session::SessionInputValue::Owned(
            Tensor::<i64>::from_array(([1usize, context_len], vec![1i64; context_len]))
                .map_err(|error| {
                    format!("Failed to create Qwen prefill attention_mask tensor: {error}")
                })?
                .into_dyn(),
        ),
    ));
    prefill_inputs.push((
        "cache_position".to_string(),
        ort::session::SessionInputValue::Owned(
            Tensor::<i64>::from_array((
                [context_len],
                (0..context_len)
                    .map(|value| value as i64)
                    .collect::<Vec<_>>(),
            ))
            .map_err(|error| {
                format!("Failed to create Qwen prefill cache_position tensor: {error}")
            })?
            .into_dyn(),
        ),
    ));
    for layer in 0..28usize {
        let shape = [1usize, max_total_len, 8usize, 128usize];
        prefill_inputs.push((
            format!("cache_key_{layer}"),
            ort::session::SessionInputValue::from(
                TensorRef::<f32>::from_array_view((shape, &cache_keys[layer][..])).map_err(
                    |error| {
                        format!("Failed to create Qwen prefill cache_key_{layer} tensor: {error}")
                    },
                )?,
            ),
        ));
        prefill_inputs.push((
            format!("cache_value_{layer}"),
            ort::session::SessionInputValue::from(
                TensorRef::<f32>::from_array_view((shape, &cache_values[layer][..])).map_err(
                    |error| {
                        format!("Failed to create Qwen prefill cache_value_{layer} tensor: {error}")
                    },
                )?,
            ),
        ));
    }

    let prefill_started_at = Instant::now();
    let prefill_outputs = decoder_session
        .run(prefill_inputs)
        .map_err(|error| format!("{provider_label} Qwen greedy decoder prefill failed: {error}"))?;
    let prefill_ms = prefill_started_at.elapsed().as_millis();
    step_timings.push(prefill_ms);
    let (logits_shape, logits) = prefill_outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|error| format!("Failed to extract Qwen prefill logits tensor: {error}"))?;
    let mut next_token = qwen_argmax_token_from_logits(&logits, logits_shape, context_len - 1)?;
    for layer in 0..28usize {
        let key_index = 1 + layer * 2;
        let value_index = key_index + 1;
        let (key_shape, key_delta) = prefill_outputs[key_index]
            .try_extract_tensor::<f32>()
            .map_err(|error| {
                format!("Failed to extract Qwen prefill key_delta_{layer}: {error}")
            })?;
        let key_seq_len = key_shape
            .get(1)
            .and_then(|value| usize::try_from(*value).ok())
            .unwrap_or(context_len);
        copy_qwen_cache_delta(&mut cache_keys[layer], 0, &key_delta, key_seq_len);
        let (value_shape, value_delta) = prefill_outputs[value_index]
            .try_extract_tensor::<f32>()
            .map_err(|error| {
            format!("Failed to extract Qwen prefill value_delta_{layer}: {error}")
        })?;
        let value_seq_len = value_shape
            .get(1)
            .and_then(|value| usize::try_from(*value).ok())
            .unwrap_or(context_len);
        copy_qwen_cache_delta(&mut cache_values[layer], 0, &value_delta, value_seq_len);
    }
    drop(prefill_outputs);
    total_steps += 1;

    let im_end_id = load_qwen_token_ids(&tokenizer_dir)?
        .get("<|im_end|>")
        .copied()
        .unwrap_or(151645);
    let endoftext_id = load_qwen_token_ids(&tokenizer_dir)?
        .get("<|endoftext|>")
        .copied()
        .unwrap_or(151643);

    if next_token != im_end_id && next_token != endoftext_id {
        generated.push(next_token);
    }

    let mut cur_len = context_len;
    for step in 1..max_new_tokens {
        if next_token == im_end_id || next_token == endoftext_id || cur_len >= max_total_len {
            break;
        }

        let mut named_inputs: Vec<(String, ort::session::SessionInputValue)> = Vec::new();
        named_inputs.push((
            "input_ids".to_string(),
            ort::session::SessionInputValue::Owned(
                Tensor::<i64>::from_array(([1usize, 1usize], vec![next_token]))
                    .map_err(|error| format!("Failed to create Qwen input_ids tensor: {error}"))?
                    .into_dyn(),
            ),
        ));
        named_inputs.push((
            "audio_features".to_string(),
            ort::session::SessionInputValue::Owned(
                Tensor::<f32>::from_array((audio_shape_usize.clone(), audio_data.clone()))
                    .map_err(|error| {
                        format!("Failed to create Qwen decoder audio_features tensor: {error}")
                    })?
                    .into_dyn(),
            ),
        ));
        named_inputs.push((
            "attention_mask".to_string(),
            ort::session::SessionInputValue::Owned(
                Tensor::<i64>::from_array(([1usize, 1usize], vec![1i64]))
                    .map_err(|error| {
                        format!("Failed to create Qwen attention_mask tensor: {error}")
                    })?
                    .into_dyn(),
            ),
        ));
        named_inputs.push((
            "cache_position".to_string(),
            ort::session::SessionInputValue::Owned(
                Tensor::<i64>::from_array(([1usize], vec![cur_len as i64]))
                    .map_err(|error| {
                        format!("Failed to create Qwen cache_position tensor: {error}")
                    })?
                    .into_dyn(),
            ),
        ));
        for layer in 0..28usize {
            let shape = [1usize, max_total_len, 8usize, 128usize];
            named_inputs.push((
                format!("cache_key_{layer}"),
                ort::session::SessionInputValue::from(
                    TensorRef::<f32>::from_array_view((shape, &cache_keys[layer][..])).map_err(
                        |error| format!("Failed to create Qwen cache_key_{layer} tensor: {error}"),
                    )?,
                ),
            ));
            named_inputs.push((
                format!("cache_value_{layer}"),
                ort::session::SessionInputValue::from(
                    TensorRef::<f32>::from_array_view((shape, &cache_values[layer][..])).map_err(
                        |error| {
                            format!("Failed to create Qwen cache_value_{layer} tensor: {error}")
                        },
                    )?,
                ),
            ));
        }

        let decode_started_at = Instant::now();
        let decoder_outputs = decoder_session.run(named_inputs).map_err(|error| {
            format!("{provider_label} Qwen greedy decoder step {step} failed: {error}")
        })?;
        let step_ms = decode_started_at.elapsed().as_millis();
        step_timings.push(step_ms);
        let (logits_shape, logits) = decoder_outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|error| format!("Failed to extract Qwen decoder logits tensor: {error}"))?;
        let last_time_index = logits_shape
            .get(1)
            .and_then(|value| usize::try_from(*value).ok())
            .and_then(|value| value.checked_sub(1))
            .ok_or_else(|| format!("Invalid Qwen decoder logits shape: {:?}", logits_shape))?;
        next_token = qwen_argmax_token_from_logits(&logits, logits_shape, last_time_index)?;

        for layer in 0..28usize {
            let key_index = 1 + layer * 2;
            let value_index = key_index + 1;
            let (key_shape, key_delta) = decoder_outputs[key_index]
                .try_extract_tensor::<f32>()
                .map_err(|error| format!("Failed to extract Qwen key_delta_{layer}: {error}"))?;
            let key_seq_len = key_shape
                .get(1)
                .and_then(|value| usize::try_from(*value).ok())
                .unwrap_or(1);
            copy_qwen_cache_delta(&mut cache_keys[layer], cur_len, &key_delta, key_seq_len);
            let (value_shape, value_delta) = decoder_outputs[value_index]
                .try_extract_tensor::<f32>()
                .map_err(|error| format!("Failed to extract Qwen value_delta_{layer}: {error}"))?;
            let value_seq_len = value_shape
                .get(1)
                .and_then(|value| usize::try_from(*value).ok())
                .unwrap_or(1);
            copy_qwen_cache_delta(
                &mut cache_values[layer],
                cur_len,
                &value_delta,
                value_seq_len,
            );
        }
        drop(decoder_outputs);
        total_steps += 1;
        cur_len += 1;

        if next_token == im_end_id || next_token == endoftext_id {
            break;
        }
        generated.push(next_token);
    }
    let mut model_inputs = Vec::new();
    model_inputs.extend(
        conv_inputs_summary
            .iter()
            .map(|item| format!("qwen conv {item}")),
    );
    model_inputs.extend(
        encoder_inputs_summary
            .iter()
            .map(|item| format!("qwen encoder {item}")),
    );
    model_inputs.extend(
        decoder_inputs_summary
            .iter()
            .map(|item| format!("qwen decoder {item}")),
    );

    let mut model_outputs = Vec::new();
    model_outputs.extend(
        conv_outputs_summary
            .iter()
            .map(|item| format!("qwen conv {item}")),
    );
    model_outputs.extend(
        encoder_outputs_summary
            .iter()
            .map(|item| format!("qwen encoder {item}")),
    );
    model_outputs.extend(
        decoder_outputs_summary
            .iter()
            .map(|item| format!("qwen decoder {item}")),
    );

    let decoded_text = decode_qwen_token_ids(&tokenizer_dir, &generated)?;
    let cleaned_text = clean_qwen_generated_text(&tokenizer_dir, &generated)?;

    Ok(DirectMlSessionProbeCliResult {
        ok: true,
        message: format!(
            "{provider_label} Qwen3-ASR 0.6B greedy wav smoke completed; wav frames: {}; audio tokens: {audio_tokens}; audio feature frames: {audio_feature_frames}; prompt tokens: {}; generated ids: {}; decoded text: {}; cleaned text: {}; steps: {total_steps}; feature {feature_ms} ms; conv {conv_ms} ms; encoder {encoder_ms} ms; decoder steps ms: {}; total {} ms",
            audio.len(),
            prompt_ids.len(),
            generated.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", "),
            decoded_text,
            cleaned_text,
            step_timings.iter().map(|ms| ms.to_string()).collect::<Vec<_>>().join(", "),
            total_started_at.elapsed().as_millis()
        ),
        model_inputs,
        model_outputs,
        onnx_runtime_build: Some(ort::info().to_string()),
    })
}
fn create_directml_qwen_chain_smoke_session_with_features(
    conv_frontend_path: &Path,
    encoder_path: &Path,
    decoder_path: &Path,
    input_frames: usize,
    input_feature_values: Vec<f32>,
    input_kind: &str,
) -> Result<DirectMlSessionProbeCliResult, String> {
    if !cfg!(windows) {
        return Err("DirectML is only available on Windows.".to_string());
    }
    if input_frames == 0 || input_feature_values.len() != input_frames * 128 {
        return Err(format!(
            "Qwen fbank feature shape is invalid: frames {}, values {}",
            input_frames,
            input_feature_values.len()
        ));
    }

    use ort::{inputs, value::Tensor};

    let total_started_at = Instant::now();
    let mut conv_session = directml_session_builder()?
        .commit_from_file(conv_frontend_path)
        .map_err(|error| {
            format!("Failed to create DirectML Qwen conv_frontend session: {error}")
        })?;
    let conv_inputs_summary = ort_outlet_summaries(conv_session.inputs());
    let conv_outputs_summary = ort_outlet_summaries(conv_session.outputs());

    let conv_started_at = Instant::now();
    let input_features =
        Tensor::<f32>::from_array(([1usize, input_frames, 128], input_feature_values))
            .map_err(|error| format!("Failed to create Qwen input_features tensor: {error}"))?;
    let conv_outputs = conv_session
        .run(inputs!["input_features" => input_features])
        .map_err(|error| format!("DirectML Qwen conv_frontend run failed: {error}"))?;
    let conv_ms = conv_started_at.elapsed().as_millis();
    let (conv_shape, conv_data) = conv_outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|error| format!("Failed to extract Qwen conv output tensor: {error}"))?;
    let conv_shape_usize = conv_shape
        .iter()
        .map(|dim| {
            usize::try_from(*dim).map_err(|_| format!("Invalid Qwen conv output dim: {dim}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let conv_frames = conv_shape_usize.get(1).copied().unwrap_or(0).max(1);
    let valid_audio_tokens = qwen_valid_audio_tokens(input_frames, conv_frames);
    let conv_data = conv_data.to_vec();
    drop(conv_outputs);
    drop(conv_session);

    let mut encoder_session = directml_session_builder()?
        .commit_from_file(encoder_path)
        .map_err(|error| format!("Failed to create DirectML Qwen encoder session: {error}"))?;
    let encoder_inputs_summary = ort_outlet_summaries(encoder_session.inputs());
    let encoder_outputs_summary = ort_outlet_summaries(encoder_session.outputs());
    let encoder_features = Tensor::<f32>::from_array((conv_shape_usize, conv_data))
        .map_err(|error| format!("Failed to create Qwen encoder input_features tensor: {error}"))?;
    let mut feature_attention_mask_values = vec![false; conv_frames];
    for value in feature_attention_mask_values
        .iter_mut()
        .take(valid_audio_tokens.min(conv_frames))
    {
        *value = true;
    }
    let feature_attention_mask =
        Tensor::<bool>::from_array(([1usize, conv_frames], feature_attention_mask_values))
            .map_err(|error| {
                format!("Failed to create Qwen feature_attention_mask tensor: {error}")
            })?;
    let encoder_started_at = Instant::now();
    let encoder_outputs = encoder_session
        .run(inputs![
            "input_features" => encoder_features,
            "feature_attention_mask" => feature_attention_mask,
        ])
        .map_err(|error| format!("DirectML Qwen encoder run failed: {error}"))?;
    let encoder_ms = encoder_started_at.elapsed().as_millis();
    let (audio_shape, audio_data) = encoder_outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|error| format!("Failed to extract Qwen encoder output tensor: {error}"))?;
    let audio_shape_usize = audio_shape
        .iter()
        .map(|dim| {
            usize::try_from(*dim).map_err(|_| format!("Invalid Qwen audio feature dim: {dim}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut audio_shape_usize = audio_shape_usize;
    let mut audio_data = audio_data.to_vec();
    let trimmed_audio_frames = trim_qwen_audio_features(&mut audio_shape_usize, &mut audio_data);
    let audio_tokens = valid_audio_tokens.min(trimmed_audio_frames).max(1);
    let audio_feature_frames =
        truncate_qwen_audio_features(&mut audio_shape_usize, &mut audio_data, audio_tokens);
    drop(encoder_outputs);
    drop(encoder_session);

    let mut decoder_session = directml_session_builder()?
        .commit_from_file(decoder_path)
        .map_err(|error| format!("Failed to create DirectML Qwen decoder session: {error}"))?;
    let decoder_inputs_summary = ort_outlet_summaries(decoder_session.inputs());
    let decoder_outputs_summary = ort_outlet_summaries(decoder_session.outputs());

    let mut named_inputs: Vec<(String, ort::session::SessionInputValue)> = Vec::new();
    named_inputs.push((
        "input_ids".to_string(),
        ort::session::SessionInputValue::Owned(
            Tensor::<i64>::from_array(([1usize, 1usize], vec![151644i64]))
                .map_err(|error| format!("Failed to create Qwen input_ids tensor: {error}"))?
                .into_dyn(),
        ),
    ));
    named_inputs.push((
        "audio_features".to_string(),
        ort::session::SessionInputValue::Owned(
            Tensor::<f32>::from_array((audio_shape_usize, audio_data))
                .map_err(|error| {
                    format!("Failed to create Qwen decoder audio_features tensor: {error}")
                })?
                .into_dyn(),
        ),
    ));
    named_inputs.push((
        "attention_mask".to_string(),
        ort::session::SessionInputValue::Owned(
            Tensor::<i64>::from_array(([1usize, 1usize], vec![1i64]))
                .map_err(|error| format!("Failed to create Qwen attention_mask tensor: {error}"))?
                .into_dyn(),
        ),
    ));
    named_inputs.push((
        "cache_position".to_string(),
        ort::session::SessionInputValue::Owned(
            Tensor::<i64>::from_array(([1usize], vec![0i64]))
                .map_err(|error| format!("Failed to create Qwen cache_position tensor: {error}"))?
                .into_dyn(),
        ),
    ));
    let max_total_len = 512usize.max(audio_tokens + 8);
    for layer in 0..28usize {
        let shape = [1usize, max_total_len, 8usize, 128usize];
        let zeros = vec![0.0f32; max_total_len * 8 * 128];
        named_inputs.push((
            format!("cache_key_{layer}"),
            ort::session::SessionInputValue::Owned(
                Tensor::<f32>::from_array((shape, zeros.clone()))
                    .map_err(|error| {
                        format!("Failed to create Qwen cache_key_{layer} tensor: {error}")
                    })?
                    .into_dyn(),
            ),
        ));
        named_inputs.push((
            format!("cache_value_{layer}"),
            ort::session::SessionInputValue::Owned(
                Tensor::<f32>::from_array((shape, zeros.clone()))
                    .map_err(|error| {
                        format!("Failed to create Qwen cache_value_{layer} tensor: {error}")
                    })?
                    .into_dyn(),
            ),
        ));
    }

    let decoder_started_at = Instant::now();
    let decoder_outputs = decoder_session
        .run(named_inputs)
        .map_err(|error| format!("DirectML Qwen decoder single-step run failed: {error}"))?;
    let decoder_ms = decoder_started_at.elapsed().as_millis();
    let (logits_shape, _logits) = decoder_outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|error| format!("Failed to extract Qwen decoder logits tensor: {error}"))?;
    let logits_shape = logits_shape
        .iter()
        .map(|dim| dim.to_string())
        .collect::<Vec<_>>()
        .join("x");
    drop(decoder_outputs);
    drop(decoder_session);

    let mut model_inputs = Vec::new();
    model_inputs.extend(
        conv_inputs_summary
            .iter()
            .map(|item| format!("qwen conv {item}")),
    );
    model_inputs.extend(
        encoder_inputs_summary
            .iter()
            .map(|item| format!("qwen encoder {item}")),
    );
    model_inputs.extend(
        decoder_inputs_summary
            .iter()
            .map(|item| format!("qwen decoder {item}")),
    );

    let mut model_outputs = Vec::new();
    model_outputs.extend(
        conv_outputs_summary
            .iter()
            .map(|item| format!("qwen conv {item}")),
    );
    model_outputs.extend(
        encoder_outputs_summary
            .iter()
            .map(|item| format!("qwen encoder {item}")),
    );
    model_outputs.extend(
        decoder_outputs_summary
            .iter()
            .map(|item| format!("qwen decoder {item}")),
    );

    Ok(DirectMlSessionProbeCliResult {
        ok: true,
        message: format!(
            "DirectML Qwen3-ASR 0.6B conv->encoder->decoder smoke completed; input kind: {input_kind}; input frames: {input_frames}; conv frames: {conv_frames}; audio tokens: {audio_tokens}; audio feature frames: {audio_feature_frames}; logits shape: {logits_shape}; conv {conv_ms} ms; encoder {encoder_ms} ms; decoder {decoder_ms} ms; total {} ms",
            total_started_at.elapsed().as_millis()
        ),
        model_inputs,
        model_outputs,
        onnx_runtime_build: Some(ort::info().to_string()),
    })
}
fn with_directml_sensevoice_runtime<T>(
    app: &AppHandle,
    candidate: &DirectMlSplitSenseVoiceCandidate,
    task_id: Option<&str>,
    started_at: Instant,
    action: impl FnOnce(&mut DirectMlSenseVoiceRuntime) -> Result<T, String>,
) -> Result<T, String> {
    let state = app.state::<RuntimeState>();
    let mut cached = state
        .directml_sensevoice
        .lock()
        .map_err(|error| error.to_string())?;

    let cache_matches = cached.as_ref().is_some_and(|runtime| {
        runtime.encoder_path == candidate.encoder
            && runtime.ctc_path == candidate.ctc
            && runtime.tokenizer_path == candidate.tokenizer
    });

    if !cache_matches {
        emit_transcription_progress(
            app,
            task_id,
            started_at,
            "transcribing",
            18,
            "Loading DirectML split SenseVoice sessions".to_string(),
            0,
            0,
        );
        let load_started_at = Instant::now();
        let pieces = load_sentencepiece_pieces(&candidate.tokenizer)?;
        let encoder_session = directml_session_builder()?
            .commit_from_file(&candidate.encoder)
            .map_err(|error| {
                format!("Failed to create DirectML split SenseVoice encoder session: {error}")
            })?;
        let ctc_session = directml_session_builder()?
            .commit_from_file(&candidate.ctc)
            .map_err(|error| {
                format!("Failed to create DirectML split SenseVoice CTC session: {error}")
            })?;
        *cached = Some(DirectMlSenseVoiceRuntime {
            encoder_path: candidate.encoder.clone(),
            ctc_path: candidate.ctc.clone(),
            tokenizer_path: candidate.tokenizer.clone(),
            pieces,
            encoder_session,
            ctc_session,
        });
        emit_transcription_progress(
            app,
            task_id,
            started_at,
            "transcribing",
            19,
            format!(
                "DirectML SenseVoice sessions cached in {} ms",
                load_started_at.elapsed().as_millis()
            ),
            0,
            0,
        );
    } else {
        emit_transcription_progress(
            app,
            task_id,
            started_at,
            "transcribing",
            19,
            "Reusing cached DirectML SenseVoice sessions".to_string(),
            0,
            0,
        );
    }

    let runtime = cached
        .as_mut()
        .ok_or_else(|| "DirectML SenseVoice runtime cache was not initialized.".to_string())?;
    action(runtime)
}
fn transcribe_directml_sensevoice_wav(
    app: &AppHandle,
    candidate: &DirectMlSplitSenseVoiceCandidate,
    wav_path: &Path,
    task_id: Option<&str>,
    started_at: Instant,
) -> Result<Vec<TranscriptTextChunk>, String> {
    if !cfg!(windows) {
        return Err("DirectML is only available on Windows.".to_string());
    }
    let missing = split_sensevoice_missing_files(candidate);
    if !missing.is_empty() {
        return Err(format!(
            "DirectML split SenseVoice files are incomplete: {}",
            missing.join(", ")
        ));
    }

    let audio = read_wav_mono_16k_f32(wav_path)?;
    let chunk_samples = 30usize * 16_000;
    let total_chunks = audio.len().div_ceil(chunk_samples).max(1);
    ensure_transcription_not_cancelled(app, task_id)?;
    let mut transcript_chunks = Vec::new();
    let mut total_feature_ms = 0u128;
    let mut total_inference_ms = 0u128;

    with_directml_sensevoice_runtime(app, candidate, task_id, started_at, |runtime| {
        for (chunk_index, chunk_audio) in audio.chunks(chunk_samples).enumerate() {
            let start = chunk_index as f64 * 30.0;
            let end = start + chunk_audio.len() as f64 / 16_000.0;
            let progress = 20 + ((chunk_index as u32 * 70) / total_chunks as u32).min(70) as u8;
            emit_transcription_progress(
                app,
                task_id,
                started_at,
                "transcribing",
                progress,
                format!(
                    "DirectML SenseVoice chunk {}/{}",
                    chunk_index + 1,
                    total_chunks
                ),
                chunk_index,
                total_chunks,
            );

            let feature_started_at = Instant::now();
            let lfr_features = extract_sensevoice_lfr_features(chunk_audio);
            let feature_ms = feature_started_at.elapsed().as_millis();
            total_feature_ms += feature_ms;

            let inference_started_at = Instant::now();
            let (text, _collapsed_ids, _valid_frames) = run_directml_sensevoice_lfr_chunk(
                &mut runtime.encoder_session,
                &mut runtime.ctc_session,
                &runtime.pieces,
                &lfr_features,
            )?;
            let inference_ms = inference_started_at.elapsed().as_millis();
            total_inference_ms += inference_ms;

            emit_transcription_progress(
                app,
                task_id,
                started_at,
                "transcribing",
                progress,
                format!(
                    "DirectML SenseVoice chunk {}/{} finished; feature {} ms, inference {} ms",
                    chunk_index + 1,
                    total_chunks,
                    feature_ms,
                    inference_ms
                ),
                chunk_index + 1,
                total_chunks,
            );

            let text = clean_subtitle_text_one_line(&text);
            if !text.is_empty() {
                transcript_chunks.push(TranscriptTextChunk { text, start, end });
            }
        }
        Ok(())
    })?;

    emit_transcription_progress(
        app,
        task_id,
        started_at,
        "transcribing",
        92,
        format!(
            "DirectML SenseVoice timing: feature {} ms, inference {} ms",
            total_feature_ms, total_inference_ms
        ),
        total_chunks,
        total_chunks,
    );

    Ok(transcript_chunks)
}

fn create_directml_split_sensevoice_sessions_in_child(
    candidate: &DirectMlSplitSenseVoiceCandidate,
) -> Result<DirectMlSessionProbeCliResult, String> {
    create_directml_session_in_child_with_args(
        "--hi-voicer-directml-sensevoice-chain-smoke",
        &[
            candidate.encoder.as_path(),
            candidate.ctc.as_path(),
            candidate.tokenizer.as_path(),
        ],
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
    candidates.push(requested.join("qwen3-asr-0.6b"));
    if let Some(parent) = requested.parent() {
        candidates.push(parent.join("sensevoice-small"));
        candidates.push(parent.join("qwen3-asr-0.6b"));
    }
    if let Some(models_dir) = app_models_dir {
        candidates.push(models_dir.join("sensevoice-small"));
        candidates.push(models_dir.join("qwen3-asr-0.6b"));
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
        .find(|candidate| directml_sensevoice_ready(candidate) || directml_qwen_ready(candidate))
        .cloned()
        .unwrap_or_else(|| PathBuf::from(requested_model_dir))
}

fn create_directml_qwen_sessions_in_child(
    candidate: &DirectMlQwenCandidate,
) -> Result<DirectMlSessionProbeCliResult, String> {
    create_directml_session_in_child_with_args(
        "--hi-voicer-directml-qwen-chain-smoke",
        &[
            &candidate.conv_frontend,
            &candidate.encoder,
            &candidate.decoder,
        ],
        Duration::from_secs(90),
        "DirectML Qwen3-ASR 0.6B conv-to-decoder child probe",
    )
}
fn directml_probe_for_model_dir(
    model_dir: &str,
    app_models_dir: Option<PathBuf>,
) -> DirectMlProbeResult {
    let started_at = Instant::now();
    let model_path = select_directml_probe_model_dir(model_dir, app_models_dir.clone());
    let split_candidate = select_directml_split_sensevoice_candidate(model_dir, app_models_dir);
    let qwen_candidate = qwen_candidate_for_dir(&model_path);
    let engine = read_sherpa_engine(&model_path).ok();
    let qwen_missing_files = qwen_missing_files(&qwen_candidate);
    let qwen_layout_detected = is_qwen_model_layout(&model_path, &qwen_missing_files);
    let mut missing_files = Vec::new();

    let model_id = engine
        .as_ref()
        .map(|engine| engine.model_id.clone())
        .or_else(|| qwen_layout_detected.then(|| "qwen3-asr-0.6b".to_string()));
    let model_name = engine
        .as_ref()
        .map(|engine| engine.model_name.clone())
        .or_else(|| qwen_layout_detected.then(|| "Qwen3-ASR 0.6B".to_string()));
    let is_sensevoice = model_id.as_deref() == Some("sensevoice-small");
    let is_qwen = model_id.as_deref() == Some("qwen3-asr-0.6b");

    if !model_path.exists() {
        missing_files.push("model directory".to_string());
    }
    if is_sensevoice {
        for required in ["engine.json", "model.int8.onnx", "tokens.txt"] {
            if !model_path.join(required).exists() {
                missing_files.push(required.to_string());
            }
        }
    } else if is_qwen {
        missing_files.extend(qwen_missing_files);
    } else if engine.is_none() {
        missing_files.push("engine.json".to_string());
    } else {
        missing_files.push("unsupported DirectML model family".to_string());
    }

    let adapters = query_directml_candidate_adapters();
    let directml_candidate = adapters.iter().any(is_directml_candidate_adapter);
    let model_ready = (is_sensevoice || is_qwen) && missing_files.is_empty();
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

    let split_model_missing_files = if is_sensevoice {
        split_sensevoice_missing_files(&split_candidate)
    } else {
        Vec::new()
    };
    let split_model_ready = is_sensevoice && split_model_missing_files.is_empty();
    let split_model_session_result = if split_model_ready && provider_session_ready {
        create_directml_split_sensevoice_sessions_in_child(&split_candidate)
    } else if is_sensevoice && !provider_session_ready {
        Err(
            "Split SenseVoice session check skipped because the DirectML provider probe failed."
                .to_string(),
        )
    } else if is_sensevoice {
        Err("Split SenseVoice files are incomplete.".to_string())
    } else {
        Err(
            "Split SenseVoice check skipped because the selected model is not SenseVoiceSmall."
                .to_string(),
        )
    };
    let split_model_session_ready = split_model_session_result
        .as_ref()
        .map(|result| result.ok)
        .unwrap_or(false);
    let split_model_session_error = if is_sensevoice {
        split_model_session_result.as_ref().err().cloned()
    } else {
        None
    };
    let split_model_inputs = split_model_session_result
        .as_ref()
        .ok()
        .filter(|_| is_sensevoice)
        .map(|result| result.model_inputs.clone())
        .unwrap_or_default();
    let split_model_outputs = split_model_session_result
        .as_ref()
        .ok()
        .filter(|_| is_sensevoice)
        .map(|result| result.model_outputs.clone())
        .unwrap_or_default();

    let directml_session_result = if is_qwen && model_ready && provider_session_ready {
        create_directml_qwen_sessions_in_child(&qwen_candidate)
    } else if split_model_session_ready {
        split_model_session_result.clone()
    } else if is_sensevoice && model_ready && provider_session_ready {
        create_directml_sensevoice_session_in_child(&model_path.join("model.int8.onnx"))
    } else if !provider_session_ready {
        Err("DirectML session check skipped because the provider probe failed.".to_string())
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
    } else if !missing_files.is_empty() {
        format!(
            "DirectML model files are incomplete: {}",
            missing_files.join(", ")
        )
    } else if !directml_candidate {
        "No usable Windows GPU adapter was detected by the DirectML probe.".to_string()
    } else {
        directml_session_error
            .clone()
            .unwrap_or_else(|| "DirectML model session check failed.".to_string())
    };
    let next_step = if is_qwen && directml_session_ready {
        "DirectML Qwen3-ASR 0.6B chain loads, but keep the stable Sherpa path until feature parity, decoded text quality, and decoder speed are proven on real samples.".to_string()
    } else if is_qwen && provider_session_ready && model_ready {
        "DirectML provider works, but Qwen chain smoke failed; inspect decoder cache shapes or unsupported operators.".to_string()
    } else if split_model_session_ready {
        "DirectML SenseVoice is ready for experimental transcription; compare timing and output quality with CPU before making it the default path.".to_string()
    } else if directml_session_ready {
        "DirectML SenseVoice is ready for experimental transcription; run a real audio/video transcription to compare timing and output quality.".to_string()
    } else if provider_session_ready && split_model_ready {
        "DirectML provider works, but split SenseVoice session creation failed; inspect unsupported operators or try another ONNX Runtime version.".to_string()
    } else if provider_session_ready && model_ready {
        "DirectML provider works, but model session creation failed; use a DirectML-friendly model before enabling transcription.".to_string()
    } else if provider_session_ready {
        "DirectML provider works; select a supported SenseVoiceSmall or Qwen3-ASR 0.6B model directory before probing model acceleration.".to_string()
    } else {
        "Keep using the stable CPU/Sherpa path; DirectML provider session is not reliable on this machine.".to_string()
    };
    DirectMlProbeResult {
        directml_candidate,
        provider_session_ready,
        provider_session_error,
        split_model_ready,
        split_model_dir: if is_sensevoice {
            Some(split_candidate.model_dir.to_string_lossy().to_string())
        } else {
            None
        },
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
    let requested_mode = if request.acceleration_mode == "directml" {
        "directml"
    } else {
        "cpu"
    };
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
    let performance = performance_for_model_and_acceleration(
        transcription_performance("stable"),
        &engine,
        &runtime.mode,
    );
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

fn finish_transcription_result(
    app: &AppHandle,
    request: &TranscribeFileRequest,
    started_at: Instant,
    source_audio_path: &Path,
    sherpa_audio_path: &Path,
    transcript_chunks: Vec<TranscriptTextChunk>,
    empty_error: &str,
    used_acceleration_mode: &str,
    acceleration_fallback_used: bool,
) -> Result<TranscribeFileResult, String> {
    let transcript_chunks = transcript_chunks
        .into_iter()
        .map(|chunk| TranscriptTextChunk {
            text: apply_hotwords(&chunk.text, &request.hotwords),
            start: chunk.start,
            end: chunk.end,
        })
        .collect::<Vec<_>>();
    let text = transcript_text_from_chunks(&transcript_chunks);

    if text.is_empty() {
        return Err(empty_error.to_string());
    }

    ensure_transcription_not_cancelled(app, request.task_id.as_deref())?;
    let duration = wav_duration_seconds(sherpa_audio_path)?.max(1.0);
    let review_audio_path = persist_review_audio(app, source_audio_path, sherpa_audio_path)?;
    let segments = if transcript_chunks.is_empty() {
        build_transcript_segments(&text, duration, &review_audio_path)
    } else {
        build_transcript_segments_from_chunks(&transcript_chunks, &review_audio_path)
    };

    let (output_path, output_paths, output_files) = if request.save_output {
        emit_transcription_progress(
            app,
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
            source_audio_path,
            sherpa_audio_path,
            &text,
            &segments,
        )?
    } else {
        (String::new(), Vec::new(), Vec::new())
    };

    emit_transcription_progress(
        app,
        request.task_id.as_deref(),
        started_at,
        "done",
        100,
        "Transcription complete".to_string(),
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
        used_acceleration_mode: used_acceleration_mode.to_string(),
        acceleration_fallback_used,
    })
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
    clear_transcription_cancelled(&app, request.task_id.as_deref())?;
    ensure_transcription_not_cancelled(&app, request.task_id.as_deref())?;
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
    ensure_transcription_not_cancelled(&app, request.task_id.as_deref())?;
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
    ensure_transcription_not_cancelled(&app, request.task_id.as_deref())?;

    if engine.engine == "faster-whisper" {
        let chunks = match transcribe_faster_whisper_wav(
            &app,
            &engine,
            &sherpa_audio_path,
            request.task_id.as_deref(),
            started_at,
        ) {
            Ok(chunks) => chunks,
            Err(error) => {
                if sherpa_audio_path != audio_path {
                    let _ = fs::remove_file(&sherpa_audio_path);
                }
                return Err(error);
            }
        };
        let result = finish_transcription_result(
            &app,
            &request,
            started_at,
            &audio_path,
            &sherpa_audio_path,
            chunks,
            "Faster-Whisper worker ran, but no transcription text was parsed.",
            if request.acceleration_mode == "directml" {
                "none"
            } else {
                &request.acceleration_mode
            },
            request.acceleration_mode == "directml",
        );
        if sherpa_audio_path != audio_path {
            let _ = fs::remove_file(&sherpa_audio_path);
        }
        return result;
    }

    if request.acceleration_mode == "directml"
        && directml_transcription_supported_for_model(&engine)
    {
        emit_transcription_progress(
            &app,
            request.task_id.as_deref(),
            started_at,
            "transcribing",
            17,
            "Actual acceleration path: DIRECTML experimental SenseVoice".to_string(),
            0,
            0,
        );
        let app_models_dir = app
            .path()
            .app_local_data_dir()
            .ok()
            .map(|data_dir| data_dir.join("models"));
        let candidate =
            select_directml_split_sensevoice_candidate(&request.model_dir, app_models_dir);
        let chunks = transcribe_directml_sensevoice_wav(
            &app,
            &candidate,
            &sherpa_audio_path,
            request.task_id.as_deref(),
            started_at,
        );
        match chunks {
            Ok(chunks) => {
                let result = finish_transcription_result(
                    &app,
                    &request,
                    started_at,
                    &audio_path,
                    &sherpa_audio_path,
                    chunks,
                    "DirectML SenseVoice ran, but no transcription text was decoded.",
                    "directml",
                    false,
                );
                if sherpa_audio_path != audio_path {
                    let _ = fs::remove_file(&sherpa_audio_path);
                }
                return result;
            }
            Err(error) => {
                emit_transcription_progress(
                    &app,
                    request.task_id.as_deref(),
                    started_at,
                    "transcribing",
                    18,
                    format!("DirectML unavailable; falling back to CPU: {error}"),
                    0,
                    0,
                );
            }
        }
    } else if request.acceleration_mode == "directml" {
        emit_transcription_progress(
            &app,
            request.task_id.as_deref(),
            started_at,
            "transcribing",
            18,
            format!(
                "DirectML transcription is not enabled for {}; using optimized Sherpa CPU.",
                engine.model_name
            ),
            0,
            0,
        );
    }

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
    let runtime_performance =
        performance_for_model_and_acceleration(performance, &engine, &runtime.mode);
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
        Err(error) => {
            if sherpa_audio_path != audio_path {
                let _ = fs::remove_file(&sherpa_audio_path);
            }
            return Err(error);
        }
    };
    let result = finish_transcription_result(
        &app,
        &request,
        started_at,
        &audio_path,
        &sherpa_audio_path,
        raw_chunks,
        "Sherpa ran, but no transcription text was parsed.",
        &runtime.mode,
        request.acceleration_mode != runtime.mode,
    );
    if sherpa_audio_path != audio_path {
        let _ = fs::remove_file(&sherpa_audio_path);
    }
    result
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
            used_acceleration_mode: "none".to_string(),
            acceleration_fallback_used: false,
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
    let cleanup_app = app.clone();
    let cleanup_task_id = request.task_id.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || transcribe_file_with_sherpa(app, request))
            .await
            .map_err(|error| error.to_string())?;
    let _ = clear_transcription_cancelled(&cleanup_app, cleanup_task_id.as_deref());
    result
}

#[tauri::command]
fn cancel_transcription(app: AppHandle, request: CancelTranscriptionRequest) -> Result<(), String> {
    mark_transcription_cancelled(&app, &request.task_id)
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
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .manage(RuntimeState {
            settings: Mutex::new(UserSettings::default()),
            recording: Mutex::new(None),
            sherpa_daemon: Mutex::new(None),
            directml_sensevoice: Mutex::new(None),
            sherpa_runtime_install: Mutex::new(()),
            transcription_cancellations: Mutex::new(HashSet::new()),
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
            cancel_transcription,
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
    fn directml_production_transcription_is_limited_to_sensevoice() {
        let sensevoice = InstalledEngineConfig {
            engine: "sherpa-onnx".to_string(),
            model_id: "sensevoice-small".to_string(),
            model_name: "SenseVoiceSmall".to_string(),
            model_dir: String::new(),
            executable: String::new(),
            args: String::new(),
            required_files: Vec::new(),
        };
        let qwen = InstalledEngineConfig {
            model_id: "qwen3-asr-0.6b".to_string(),
            model_name: "Qwen3-ASR 0.6B".to_string(),
            ..sensevoice.clone()
        };

        assert!(directml_transcription_supported_for_model(&sensevoice));
        assert!(!directml_transcription_supported_for_model(&qwen));
        assert!(!sherpa_daemon_supported_for_model(&sensevoice));
        assert!(sherpa_daemon_supported_for_model(&qwen));
    }
    #[test]
    fn qwen_sherpa_uses_measured_thread_profile() {
        let qwen = InstalledEngineConfig {
            engine: "sherpa-onnx".to_string(),
            model_id: "qwen3-asr-0.6b".to_string(),
            model_name: "Qwen3-ASR 0.6B".to_string(),
            model_dir: String::new(),
            executable: String::new(),
            args: String::new(),
            required_files: Vec::new(),
        };
        let paraformer = InstalledEngineConfig {
            model_id: "sherpa-paraformer-zh".to_string(),
            model_name: "Paraformer".to_string(),
            ..qwen.clone()
        };

        assert_eq!(
            performance_for_model_and_acceleration(
                transcription_performance("stable"),
                &qwen,
                "cpu"
            )
            .sherpa_threads,
            6
        );
        assert_eq!(
            performance_for_model_and_acceleration(
                transcription_performance("balanced"),
                &qwen,
                "cpu"
            )
            .sherpa_threads,
            3
        );
        assert_eq!(
            performance_for_model_and_acceleration(transcription_performance("fast"), &qwen, "cpu")
                .sherpa_threads,
            2
        );
        assert_eq!(
            performance_for_model_and_acceleration(
                transcription_performance("stable"),
                &paraformer,
                "cpu"
            )
            .sherpa_threads,
            transcription_performance("stable").sherpa_threads
        );
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
    fn radix2_fft_detects_expected_frequency_bin() {
        let sample_rate = 16_000.0f32;
        let freq = 1_000.0f32;
        let n = 512usize;
        let mut real = (0..n)
            .map(|index| (2.0 * std::f32::consts::PI * freq * index as f32 / sample_rate).sin())
            .collect::<Vec<_>>();
        let mut imag = vec![0.0f32; n];

        fft_radix2_in_place(&mut real, &mut imag);

        let peak_bin = (1..(n / 2))
            .max_by(|left, right| {
                let left_power = real[*left] * real[*left] + imag[*left] * imag[*left];
                let right_power = real[*right] * real[*right] + imag[*right] * imag[*right];
                left_power.total_cmp(&right_power)
            })
            .expect("peak bin");

        assert_eq!(peak_bin, 32);
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
    fn qwen_whisper_fft_matches_reference_dft_power() {
        let frame = (0..400)
            .map(|index| {
                let t = index as f32 / 400.0;
                (2.0 * std::f32::consts::PI * 7.0 * t).sin()
                    + 0.25 * (2.0 * std::f32::consts::PI * 19.0 * t).cos()
            })
            .collect::<Vec<_>>();
        let expected = real_dft_power_spectrum(&frame);
        let actual = whisper_fft_power_spectrum(&frame);

        assert_eq!(actual.len(), expected.len());
        for (left, right) in actual.iter().zip(expected.iter()) {
            let tolerance = 1.0e-3 * right.abs().max(1.0);
            assert!((left - right).abs() < tolerance, "{left} vs {right}");
        }
    }

    #[test]
    fn qwen_fbank_features_have_expected_shape_for_half_second_audio() {
        let audio = vec![0.0f32; 8_000];
        let features = extract_qwen_fbank_features(&audio);

        assert_eq!(features.frames, 50);
        assert_eq!(features.values.len(), features.frames * 128);
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

    fn protobuf_varint(value: u64) -> Vec<u8> {
        let mut value = value;
        let mut bytes = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                break;
            }
        }
        bytes
    }

    fn sentencepiece_test_model(pieces: &[&str]) -> Vec<u8> {
        let mut model = Vec::new();
        for piece in pieces {
            let mut vocab_message = Vec::new();
            vocab_message.push(0x0a);
            vocab_message.extend(protobuf_varint(piece.as_bytes().len() as u64));
            vocab_message.extend(piece.as_bytes());
            vocab_message.push(0x15);
            vocab_message.extend([0, 0, 0, 0]);

            model.push(0x0a);
            model.extend(protobuf_varint(vocab_message.len() as u64));
            model.extend(vocab_message);
        }
        model
    }

    #[test]
    fn parses_sentencepiece_vocab_pieces() {
        let path = std::env::temp_dir().join(format!(
            "hi-voicer-tokenizer-test-{}-{}.model",
            std::process::id(),
            unix_timestamp_millis().unwrap_or(0)
        ));
        fs::write(
            &path,
            sentencepiece_test_model(&["<unk>", "<s>", "</s>", "▁hello", "世界"]),
        )
        .expect("write tokenizer");

        let pieces = load_sentencepiece_pieces(&path).expect("parse tokenizer");

        assert_eq!(pieces[0], "<unk>");
        assert_eq!(pieces[3], "▁hello");
        assert_eq!(pieces[4], "世界");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn ctc_greedy_decode_skips_prompt_blank_and_repeats() {
        let pieces = vec![
            "<blank>".to_string(),
            "<s>".to_string(),
            "</s>".to_string(),
            "▁你".to_string(),
            "好".to_string(),
        ];
        let mut topk = vec![0i32; 1 * 9 * 2];
        for (frame, token) in [1, 2, 3, 3, 0, 4, 4, 0, 3].iter().enumerate() {
            topk[frame * 2] = *token;
        }

        let (ids, text) = ctc_greedy_decode_top1(&topk, &[1, 9, 2], 4, &pieces);

        assert_eq!(ids, vec![4]);
        assert_eq!(text, "好");
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
    fn qwen_candidate_detects_ready_model_layout() {
        let root = std::env::temp_dir().join(format!(
            "hi-voicer-qwen-directml-test-{}-{}",
            std::process::id(),
            unix_timestamp_millis().unwrap_or(0)
        ));
        let tokenizer_dir = root.join("tokenizer");
        fs::create_dir_all(&tokenizer_dir).expect("create tokenizer dir");
        for file in [
            "conv_frontend.onnx",
            "encoder.int8.onnx",
            "decoder.int8.onnx",
        ] {
            fs::write(root.join(file), b"placeholder").expect("write qwen onnx file");
        }
        for file in ["merges.txt", "tokenizer_config.json", "vocab.json"] {
            fs::write(tokenizer_dir.join(file), b"placeholder").expect("write tokenizer file");
        }

        let candidate = qwen_candidate_for_dir(&root);

        let missing = qwen_missing_files(&candidate);

        assert!(missing.is_empty());
        assert!(directml_qwen_ready(&root));
        assert!(is_qwen_model_layout(&root, &missing));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn qwen_audio_token_len_matches_sherpa_chunk_formula() {
        assert_eq!(qwen_feat_to_audio_tokens_len(0, 100), 0);
        assert_eq!(qwen_feat_to_audio_tokens_len(100, 100), 13);
        assert_eq!(qwen_feat_to_audio_tokens_len(2076, 100), 270);
        assert_eq!(qwen_valid_audio_tokens(2076, 273), 270);
    }

    #[test]
    fn qwen_audio_features_trim_trailing_zero_frames() {
        let mut shape = vec![1, 3, 2];
        let mut data = vec![0.1, -0.2, 0.3, 0.4, 0.0, 0.0];

        let valid = trim_qwen_audio_features(&mut shape, &mut data);

        assert_eq!(valid, 2);
        assert_eq!(shape, vec![1, 2, 2]);
        assert_eq!(data, vec![0.1, -0.2, 0.3, 0.4]);
    }

    #[test]
    fn qwen_audio_features_can_be_truncated_to_prompt_tokens() {
        let mut shape = vec![1, 3, 2];
        let mut data = vec![0.1, -0.2, 0.3, 0.4, 0.5, 0.6];

        let valid = truncate_qwen_audio_features(&mut shape, &mut data, 2);

        assert_eq!(valid, 2);
        assert_eq!(shape, vec![1, 2, 2]);
        assert_eq!(data, vec![0.1, -0.2, 0.3, 0.4]);
    }

    #[test]
    fn qwen_default_source_ids_follow_sherpa_prompt_scaffold() {
        let root = std::env::temp_dir().join(format!(
            "hi-voicer-qwen-tokenizer-test-{}-{}",
            std::process::id(),
            unix_timestamp_millis().unwrap_or(0)
        ));
        fs::create_dir_all(&root).expect("create tokenizer dir");
        fs::write(
            root.join("vocab.json"),
            r#"{"system":8948,"user":872,"assistant":77091,"Ċ":198}"#,
        )
        .expect("write qwen vocab");
        fs::write(root.join("merges.txt"), b"").expect("write qwen merges");
        fs::write(
            root.join("tokenizer_config.json"),
            r#"{"added_tokens_decoder":{"151644":{"content":"<|im_start|>"},"151645":{"content":"<|im_end|>"},"151669":{"content":"<|audio_start|>"},"151676":{"content":"<|audio_pad|>"},"151670":{"content":"<|audio_end|>"},"151643":{"content":"<|endoftext|>"}}}"#,
        )
        .expect("write qwen tokenizer config");

        let ids = build_qwen_default_source_ids(&root, 2).expect("build source ids");

        assert_eq!(
            ids,
            vec![
                151644, 8948, 198, 151645, 198, 151644, 872, 198, 151669, 151676, 151676, 151670,
                151645, 198, 151644, 77091, 198,
            ]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn qwen_decode_token_ids_restores_byte_level_text_and_specials() {
        let root = std::env::temp_dir().join(format!(
            "hi-voicer-qwen-decode-test-{}-{}",
            std::process::id(),
            unix_timestamp_millis().unwrap_or(0)
        ));
        fs::create_dir_all(&root).expect("create tokenizer dir");
        fs::write(
            root.join("vocab.json"),
            r#"{"ä½ł":11528,"å¥½":2240,"Ċ":198}"#,
        )
        .expect("write qwen vocab");
        fs::write(root.join("merges.txt"), b"").expect("write qwen merges");
        fs::write(
            root.join("tokenizer_config.json"),
            r#"{"added_tokens_decoder":{"151644":{"content":"<|im_start|>"},"151645":{"content":"<|im_end|>"},"151704":{"content":"<asr_text>"}}}"#,
        )
        .expect("write qwen tokenizer config");

        let decoded =
            decode_qwen_token_ids(&root, &[11528, 2240, 198, 151704]).expect("decode qwen ids");
        let cleaned =
            clean_qwen_generated_text(&root, &[151704, 11528, 2240]).expect("clean qwen ids");

        assert_eq!(decoded, "你好\n<asr_text>");
        assert_eq!(cleaned, "你好");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn qwen_layout_can_be_inferred_from_directory_name_without_engine_json() {
        let root = std::env::temp_dir()
            .join(format!("hi-voicer-qwen-name-test-{}", std::process::id()))
            .join("qwen3-asr-0.6b");
        let tokenizer_dir = root.join("tokenizer");
        fs::create_dir_all(&tokenizer_dir).expect("create tokenizer dir");
        for file in [
            "conv_frontend.onnx",
            "encoder.int8.onnx",
            "decoder.int8.onnx",
        ] {
            fs::write(root.join(file), b"placeholder").expect("write qwen onnx file");
        }
        for file in ["merges.txt", "tokenizer_config.json", "vocab.json"] {
            fs::write(tokenizer_dir.join(file), b"placeholder").expect("write tokenizer file");
        }

        let missing = qwen_missing_files(&qwen_candidate_for_dir(&root));

        assert!(is_qwen_model_layout(&root, &missing));
        let _ = fs::remove_dir_all(root.parent().unwrap_or(&root));
    }

    #[test]
    fn directml_probe_candidates_include_app_qwen_model() {
        let app_models_dir =
            PathBuf::from(r"C:\Users\tester\AppData\Local\com.local.hivoicer\models");
        let candidates = directml_probe_candidate_dirs(
            r"C:\Portable\Hi-Voicer\models",
            Some(app_models_dir.clone()),
        );

        assert!(candidates.contains(&app_models_dir.join("qwen3-asr-0.6b")));
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

        assert_eq!(sherpa_chunk_seconds(&qwen), QWEN_ASR_CHUNK_SECONDS);
        assert_eq!(sherpa_chunk_seconds(&funasr), LLM_ASR_CHUNK_SECONDS);
        assert_eq!(
            sherpa_max_single_pass_seconds(&qwen),
            QWEN_ASR_CHUNK_SECONDS as f64
        );
        assert_eq!(sherpa_chunk_seconds(&paraformer), LONG_AUDIO_CHUNK_SECONDS);
        assert_eq!(
            sherpa_max_single_pass_seconds(&paraformer),
            LONG_AUDIO_THRESHOLD_SECONDS
        );
    }

    #[test]
    fn faster_whisper_engine_config_validates() {
        let root = test_root("faster-whisper-engine-config-validates");
        let worker = root.join(if cfg!(windows) {
            "worker.exe"
        } else {
            "worker"
        });
        fs::write(&worker, b"worker").expect("write worker");
        fs::write(root.join("model.bin"), b"model").expect("write model");
        let config = InstalledEngineConfig {
            engine: "faster-whisper".to_string(),
            model_id: "faster-whisper".to_string(),
            model_name: "Faster-Whisper".to_string(),
            model_dir: root.to_string_lossy().to_string(),
            executable: worker.to_string_lossy().to_string(),
            args: "--device cpu --compute-type int8".to_string(),
            required_files: vec!["model.bin".to_string()],
        };
        fs::write(
            root.join("engine.json"),
            serde_json::to_string(&config).expect("serialize config"),
        )
        .expect("write config");

        let validation = validate_model_dir_path(&root.to_string_lossy());

        assert!(validation.valid, "{}", validation.message);
        assert_eq!(validation.model_name, "Faster-Whisper");
    }

    #[test]
    fn parses_faster_whisper_worker_segments() {
        let raw = r#"{
            "text":"ignored when segments exist",
            "segments":[
                {"start":0.0,"end":1.2,"text":" 第一段 "},
                {"start":1.2,"end":2.4,"text":"第二段"},
                {"start":2.4,"end":2.6,"text":"   "}
            ]
        }"#;

        let chunks = parse_faster_whisper_worker_output(raw, 5.0).expect("parse output");

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text, "第一段");
        assert_eq!(chunks[1].start, 1.2);
    }

    #[test]
    fn builds_faster_whisper_worker_args() {
        let engine = InstalledEngineConfig {
            engine: "faster-whisper".to_string(),
            model_id: "faster-whisper".to_string(),
            model_name: "Faster-Whisper".to_string(),
            model_dir: r"C:\Models\fw".to_string(),
            executable: r"C:\Engines\fw-worker.exe".to_string(),
            args: "--device cuda --compute-type int8_float16 --model {modelDir}".to_string(),
            required_files: Vec::new(),
        };
        let wav_path = PathBuf::from(r"C:\Audio\sample.wav");
        let output_path = PathBuf::from(r"C:\Temp\out.json");

        let args = faster_whisper_args_for_engine(&engine, &wav_path, &output_path).expect("args");

        assert!(args.contains(&"--device".to_string()));
        assert!(args.contains(&"cuda".to_string()));
        assert!(args.contains(&r"C:\Models\fw".to_string()));
        assert!(args.contains(&"--audio".to_string()));
        assert!(args.contains(&wav_path.to_string_lossy().to_string()));
        assert!(args.contains(&"--output-json".to_string()));
        assert!(args.contains(&output_path.to_string_lossy().to_string()));
    }
    #[test]
    fn qwen_runtime_args_force_conservative_generation_limit() {
        let qwen = InstalledEngineConfig {
            engine: "sherpa-onnx".to_string(),
            model_id: "qwen3-asr-0.6b".to_string(),
            model_name: "Qwen3-ASR 0.6B".to_string(),
            model_dir: String::new(),
            executable: String::new(),
            args: "--qwen3-asr-max-new-tokens=128 --num-threads=6".to_string(),
            required_files: Vec::new(),
        };

        let args = sherpa_args_for_engine_runtime(&qwen, Some(3), "cpu").expect("args");

        assert!(args.contains(&format!(
            "--qwen3-asr-max-new-tokens={QWEN_ASR_MAX_NEW_TOKENS}"
        )));
        assert!(args.contains(&"--num-threads=3".to_string()));
    }

    #[test]
    fn qwen_latin_language_drift_can_be_detected_for_diagnostics() {
        let qwen = InstalledEngineConfig {
            engine: "sherpa-onnx".to_string(),
            model_id: "qwen3-asr-0.6b".to_string(),
            model_name: "Qwen3-ASR 0.6B".to_string(),
            model_dir: String::new(),
            executable: String::new(),
            args: String::new(),
            required_files: Vec::new(),
        };
        let paraformer = InstalledEngineConfig {
            model_id: "sherpa-paraformer-zh".to_string(),
            model_name: "Sherpa Paraformer".to_string(),
            ..qwen.clone()
        };

        assert!(!should_keep_sherpa_chunk_text(
            &qwen,
            "Un craciun obtinut in jurul ora si jumatate a fost actionat de explozie"
        ));
        assert!(should_keep_sherpa_chunk_text(
            &qwen,
            "\u{5E7F}\u{897F}\u{58EE}\u{65CF}\u{81EA}\u{6CBB}\u{533A} news report"
        ));
        assert!(should_keep_sherpa_chunk_text(
            &paraformer,
            "Un craciun obtinut in jurul ora si jumatate a fost actionat de explozie"
        ));
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
    fn decodes_gbk_sherpa_websocket_json_text() {
        let bytes = b"{\"text\":\"\xB9\xE3\xCE\xF7\"}";
        let decoded = decode_sherpa_websocket_text(bytes);

        assert_eq!(extract_transcription_text(&decoded), "广西");
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
    fn qwen_silent_chunk_detection_is_conservative() {
        let root = test_root("qwen-silent-chunk-detection-is-conservative");
        let silent_path = root.join("silent.wav");
        let quiet_voice_path = root.join("quiet-voice.wav");
        write_test_wav_seconds(&silent_path, 1);
        write_test_wav_with_sample(&quiet_voice_path, 1, 320);

        assert!(wav_likely_silent_for_qwen(&silent_path).expect("silent wav"));
        assert!(!wav_likely_silent_for_qwen(&quiet_voice_path).expect("quiet voice wav"));
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

    fn write_test_wav_with_sample(path: &Path, seconds: usize, sample: i16) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        for index in 0..(16000 * seconds) {
            let value = if index % 2 == 0 { sample } else { -sample };
            writer.write_sample(value).expect("write sample");
        }
        writer.finalize().expect("finalize wav");
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
