import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import type {
  AccelerationStatus,
  AccelerationSmokeTestResult,
  AudioProcessingOptions,
  AudioProcessingResult,
  ModelInstallProgress,
  ModelPreset,
  ModelValidationResult,
  NativeAudioDiagnostics,
  TranscriptionPerformanceMode,
  TranscriptionProgress,
  TranscribeFileResult,
  UserSettings,
} from "../types";

const STORAGE_KEY = "hi-voicer-settings";
const pasteModes = ["direct", "clipboard"] as const;
const recordingModes = ["hold", "toggle", "audioOnly"] as const;
const recordingSources = ["microphone", "system", "microphoneAndSystem"] as const;
const accelerationModes = ["cpu", "cuda"] as const;
const themeModes = ["light", "dark"] as const;

function enumValue<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return typeof value === "string" && allowed.includes(value as T) ? (value as T) : fallback;
}

function stringValue(value: unknown, fallback: string): string {
  return typeof value === "string" ? value : fallback;
}

function booleanValue(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

function arrayValue<T>(value: unknown, fallback: T[]): T[] {
  return Array.isArray(value) ? (value as T[]) : fallback;
}

function normalizeSettings(defaultSettings: UserSettings, value: unknown): UserSettings {
  const settings = value && typeof value === "object" ? (value as Partial<UserSettings>) : {};
  return {
    ...defaultSettings,
    ...settings,
    shortcut: stringValue(settings.shortcut, defaultSettings.shortcut),
    selectedModelId: stringValue(settings.selectedModelId, defaultSettings.selectedModelId),
    modelDir: stringValue(settings.modelDir, defaultSettings.modelDir),
    outputDir: stringValue(settings.outputDir, defaultSettings.outputDir),
    pasteMode: enumValue(settings.pasteMode, pasteModes, defaultSettings.pasteMode),
    recordingMode: enumValue(settings.recordingMode, recordingModes, defaultSettings.recordingMode),
    recordingSource: enumValue(settings.recordingSource, recordingSources, defaultSettings.recordingSource),
    accelerationMode: enumValue(settings.accelerationMode, accelerationModes, defaultSettings.accelerationMode),
    hotwords: arrayValue(settings.hotwords, defaultSettings.hotwords),
    termCategories: arrayValue(settings.termCategories, defaultSettings.termCategories),
    theme: enumValue(settings.theme, themeModes, defaultSettings.theme),
    saveRecordings: booleanValue(settings.saveRecordings, defaultSettings.saveRecordings),
    launchAtStartup: booleanValue(settings.launchAtStartup, defaultSettings.launchAtStartup),
    showMiniWindow: booleanValue(settings.showMiniWindow, defaultSettings.showMiniWindow),
  };
}

function saveFallbackSettings(settings: UserSettings): UserSettings {
  return {
    ...settings,
    pasteMode: "direct",
    recordingMode: "hold",
    recordingSource: "microphone",
    accelerationMode: "cpu",
    theme: "light",
    saveRecordings: false,
    launchAtStartup: false,
    showMiniWindow: true,
  };
}

export async function loadSettings(defaultSettings: UserSettings): Promise<UserSettings> {
  try {
    const settings = await invoke<Partial<UserSettings>>("load_settings");
    return normalizeSettings(defaultSettings, settings);
  } catch {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) {
      return defaultSettings;
    }
    try {
      return normalizeSettings(defaultSettings, JSON.parse(raw));
    } catch {
      return defaultSettings;
    }
  }
}

export async function saveSettings(settings: UserSettings): Promise<UserSettings> {
  const normalizedSettings = normalizeSettings(saveFallbackSettings(settings), settings);
  try {
    const saved = await invoke<Partial<UserSettings>>("save_settings", { settings: normalizedSettings });
    return normalizeSettings(normalizedSettings, saved);
  } catch {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(normalizedSettings));
    return normalizedSettings;
  }
}

export async function selectDirectory(): Promise<string | null> {
  try {
    return await invoke<string | null>("select_directory");
  } catch {
    // Fall through to the browser/Tauri plugin path.
  }

  try {
    const selected = await openDialog({
      directory: true,
      multiple: false,
    });
    return typeof selected === "string" ? selected : null;
  } catch {
    const typed = window.prompt("请输入文件夹路径");
    return typed?.trim() || null;
  }
}

export async function selectAudioFiles(): Promise<string[]> {
  try {
    return await invoke<string[]>("select_audio_files");
  } catch {
    const typed = window.prompt("请输入音频文件路径");
    return typed?.trim() ? [typed.trim()] : [];
  }
}

export async function listAudioFilesInDirectory(directoryPath: string): Promise<string[]> {
  return await invoke<string[]>("list_audio_files_in_directory", {
    request: {
      directoryPath,
    },
  });
}

export async function prepareAudioPreview(audioPath: string): Promise<string> {
  return await invoke<string>("prepare_audio_preview", {
    request: {
      audioPath,
    },
  });
}

export async function openExternalUrl(url: string): Promise<void> {
  try {
    await openUrl(url);
  } catch {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

export async function openRecordingsFolder(): Promise<string> {
  return await invoke<string>("open_recordings_dir");
}

export async function saveTextFile(suggestedName: string, contents: string): Promise<string | null> {
  try {
    return await invoke<string | null>("save_text_file", {
      request: {
        suggestedName,
        contents,
      },
    });
  } catch {
    const url = URL.createObjectURL(new Blob([contents], { type: "text/plain;charset=utf-8" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = suggestedName;
    link.click();
    URL.revokeObjectURL(url);
    return suggestedName;
  }
}

export async function saveExistingFile(
  sourcePath: string,
  suggestedName: string,
  destinationDir?: string,
): Promise<string | null> {
  return await invoke<string | null>("save_existing_file", {
    request: {
      sourcePath,
      suggestedName,
      destinationDir,
    },
  });
}

export async function installModel(model: ModelPreset): Promise<string> {
  if (model.installKind === "engineRequired") {
    throw new Error(model.engineNote);
  }

  return await invoke<string>("install_model", {
    model: {
      id: model.id,
      name: model.name,
      installKind: model.installKind,
      downloadUrl: model.downloadUrl,
      archiveRoot: model.archiveRoot,
      modelFiles: model.modelFiles ?? [],
      sherpaArgs: model.sherpaArgs ?? "",
    },
  });
}

export async function listenModelInstallProgress(
  handler: (progress: ModelInstallProgress) => void,
): Promise<() => void> {
  try {
    return await listen<ModelInstallProgress>("model-install-progress", (event) => handler(event.payload));
  } catch {
    return () => {};
  }
}

export async function listenRecordingState(handler: (isRecording: boolean) => void): Promise<() => void> {
  try {
    return await listen<{ isRecording: boolean }>("recording-state", (event) => handler(event.payload.isRecording));
  } catch {
    return () => {};
  }
}

export async function listenRecordingLevel(handler: (level: number) => void): Promise<() => void> {
  try {
    return await listen<{ level: number }>("recording-level", (event) => handler(event.payload.level));
  } catch {
    return () => {};
  }
}

export async function listenRecordingError(handler: (message: string) => void): Promise<() => void> {
  try {
    return await listen<{ message: string }>("recording-error", (event) => handler(event.payload.message));
  } catch {
    return () => {};
  }
}

export async function listenTranscriptionResult(
  handler: (result: TranscribeFileResult) => void,
): Promise<() => void> {
  try {
    return await listen<TranscribeFileResult>("transcription-result", (event) => handler(event.payload));
  } catch {
    return () => {};
  }
}

export async function listenTranscriptionProgress(
  handler: (progress: TranscriptionProgress) => void,
): Promise<() => void> {
  try {
    return await listen<TranscriptionProgress>("transcription-progress", (event) => handler(event.payload));
  } catch {
    return () => {};
  }
}

export async function listenMiniToggleRecording(handler: () => void): Promise<() => void> {
  try {
    return await listen("mini-toggle-recording", handler, { target: { kind: "Window", label: "main" } });
  } catch {
    return () => {};
  }
}

export async function requestMainRecordingToggle(): Promise<void> {
  try {
    await emitTo({ kind: "Window", label: "main" }, "mini-toggle-recording");
  } catch {
    await emitTo("main", "mini-toggle-recording");
  }
}

export async function validateModelDir(modelDir: string): Promise<ModelValidationResult> {
  if (!modelDir.trim()) {
    return {
      valid: false,
      modelName: "",
      message: "尚未配置离线模型。",
    };
  }

  try {
    return await invoke<ModelValidationResult>("validate_model_dir", { request: { modelDir } });
  } catch {
    return {
      valid: true,
      modelName: "本地模型",
      message: "浏览器预览模式无法校验本地模型目录。",
    };
  }
}

export async function getAccelerationStatus(accelerationMode: UserSettings["accelerationMode"]): Promise<AccelerationStatus> {
  try {
    return await invoke<AccelerationStatus>("get_acceleration_status", { request: { accelerationMode } });
  } catch {
    return {
      selectedMode: accelerationMode,
      effectiveMode: accelerationMode === "cuda" ? "cpu" : "cpu",
      cudaAvailable: false,
      cudaDeviceSummary: null,
      cudaDetectionError: "浏览器预览模式无法运行 nvidia-smi。",
      cpuRuntimeInstalled: false,
      cudaRuntimeInstalled: false,
      cudaDisabledReason: null,
      message:
        accelerationMode === "cuda"
          ? "浏览器预览模式无法检测 CUDA；实际转录会在桌面端检测并自动回退 CPU。"
          : "当前选择 CPU，兼容性最高。",
    };
  }
}

export async function prepareAccelerationRuntime(accelerationMode: UserSettings["accelerationMode"]): Promise<AccelerationStatus> {
  try {
    return await invoke<AccelerationStatus>("prepare_acceleration_runtime", { request: { accelerationMode } });
  } catch (error) {
    return {
      selectedMode: accelerationMode,
      effectiveMode: "cpu",
      cudaAvailable: false,
      cudaDeviceSummary: null,
      cudaDetectionError: error instanceof Error ? error.message : "加速运行时准备失败。",
      cpuRuntimeInstalled: false,
      cudaRuntimeInstalled: false,
      cudaDisabledReason: null,
      message: error instanceof Error ? error.message : "加速运行时准备失败；转录时会回退 CPU。",
    };
  }
}

export async function runAccelerationSmokeTest(settings: UserSettings): Promise<AccelerationSmokeTestResult> {
  if (!settings.modelDir) {
    throw new Error("请先配置离线模型。");
  }

  return await invoke<AccelerationSmokeTestResult>("run_acceleration_smoke_test", {
    request: {
      modelDir: settings.modelDir,
      accelerationMode: settings.accelerationMode ?? "cpu",
    },
  });
}

export async function getNativeAudioDiagnostics(): Promise<NativeAudioDiagnostics> {
  try {
    return await invoke<NativeAudioDiagnostics>("get_native_audio_diagnostics");
  } catch {
    return {
      microphoneAvailable: false,
      microphoneName: null,
      microphoneDetail: "浏览器预览模式无法检测本机麦克风设备。",
      systemAudioAvailable: false,
      systemAudioName: null,
      systemAudioDetail: "浏览器预览模式无法检测 Windows 系统声音 loopback。",
      ffmpegInstalled: false,
      ffmpegPath: null,
      ffmpegDetail: "浏览器预览模式无法检测本地 ffmpeg 运行时。",
      message: "请在桌面应用中运行本机音频环境诊断。",
    };
  }
}

export async function transcribeFile(
  audioPath: string,
  settings: UserSettings,
  options: { saveOutput?: boolean; outputFormat?: string; taskId?: string; performanceMode?: TranscriptionPerformanceMode } = {},
): Promise<TranscribeFileResult> {
  if (!settings.modelDir) {
    throw new Error("请先在设置里下载并配置离线模型。");
  }

  return await invoke<TranscribeFileResult>("transcribe_file", {
    request: {
      audioPath,
      modelDir: settings.modelDir,
      outputFormat: options.outputFormat ?? "plainText",
      saveOutput: options.saveOutput ?? true,
      taskId: options.taskId,
      performanceMode: options.performanceMode ?? "balanced",
      accelerationMode: settings.accelerationMode ?? "cpu",
      hotwords: settings.hotwords ?? [],
    },
  });
}

export async function exportAudioSegment(
  sourceAudioPath: string,
  startSeconds: number,
  endSeconds: number,
  options: { destinationDir?: string; suggestedName?: string } = {},
): Promise<string> {
  return await invoke<string>("export_audio_segment", {
    request: {
      sourceAudioPath,
      startSeconds,
      endSeconds,
      destinationDir: options.destinationDir,
      suggestedName: options.suggestedName,
    },
  });
}

export async function processAudioFile(
  audioPath: string,
  options: AudioProcessingOptions,
  output: { destinationDir?: string } = {},
): Promise<AudioProcessingResult> {
  return await invoke<AudioProcessingResult>("process_audio_file", {
    request: {
      audioPath,
      options,
      destinationDir: output.destinationDir,
    },
  });
}

export async function startRecording(): Promise<string> {
  return await invoke<string>("start_recording");
}

export async function stopRecording(): Promise<TranscribeFileResult> {
  return await invoke<TranscribeFileResult>("stop_recording");
}
