import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import type {
  ModelInstallProgress,
  ModelPreset,
  ModelValidationResult,
  TranscriptionPerformanceMode,
  TranscriptionProgress,
  TranscribeFileResult,
  UserSettings,
} from "../types";

const STORAGE_KEY = "hi-voicer-settings";

export async function loadSettings(defaultSettings: UserSettings): Promise<UserSettings> {
  try {
    const settings = await invoke<Partial<UserSettings>>("load_settings");
    return { ...defaultSettings, ...settings };
  } catch {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return raw ? { ...defaultSettings, ...JSON.parse(raw) } : defaultSettings;
  }
}

export async function saveSettings(settings: UserSettings): Promise<UserSettings> {
  try {
    const saved = await invoke<Partial<UserSettings>>("save_settings", { settings });
    return { ...settings, ...saved };
  } catch {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
    return settings;
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
    },
  });
}

export async function startRecording(): Promise<string> {
  return await invoke<string>("start_recording");
}

export async function stopRecording(): Promise<TranscribeFileResult> {
  return await invoke<TranscribeFileResult>("stop_recording");
}
