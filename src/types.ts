export type ReadinessState =
  | "starting"
  | "loading-model"
  | "ready"
  | "model-required"
  | "microphone-unavailable"
  | "error";

export type AppPage = "home" | "transcription" | "hotwords" | "settings" | "diagnostics";

export type PasteMode = "direct" | "clipboard";

export interface AppStatus {
  readiness: ReadinessState;
  modelName: string;
  shortcut: string;
  microphoneName: string;
  lastResult: string;
  isRecording: boolean;
}

export interface TranscriptTask {
  id: string;
  fileName: string;
  status: "queued" | "running" | "done" | "failed";
  progress: number;
  outputFormats: Array<"txt" | "srt" | "json">;
  message: string;
}

export interface HotwordRule {
  id: string;
  source: string;
  target: string;
  enabled: boolean;
}

export interface UserSettings {
  shortcut: string;
  modelDir: string;
  outputDir: string;
  pasteMode: PasteMode;
  saveRecordings: boolean;
  launchAtStartup: boolean;
}

export interface DiagnosticItem {
  id: string;
  label: string;
  status: "ok" | "warning" | "error";
  detail: string;
}
