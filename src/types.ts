export type ReadinessState =
  | "starting"
  | "loading-model"
  | "ready"
  | "model-required"
  | "microphone-unavailable"
  | "error";

export type AppPage = "home" | "transcription" | "hotwords" | "settings" | "diagnostics";

export type PasteMode = "direct" | "clipboard";
export type RecordingMode = "hold" | "toggle" | "audioOnly";
export type ExportFormat = "plainText" | "timelineText" | "srt";
export type ThemeMode = "light" | "dark";
export type TranscriptionPerformanceMode = "stable" | "balanced" | "fast";

export interface AppStatus {
  readiness: ReadinessState;
  modelName: string;
  shortcut: string;
  microphoneName: string;
  lastResult: string;
  isRecording: boolean;
  recordingMode: RecordingMode;
}

export interface TranscriptTask {
  id: string;
  fileName: string;
  filePath?: string;
  status: "queued" | "running" | "done" | "failed";
  progress: number;
  outputFormats: Array<"txt" | "srt" | "json">;
  message: string;
  outputPath?: string;
  outputPaths?: string[];
  outputFiles?: TranscriptionOutputFile[];
  startedAt?: string;
  finishedAt?: string;
  elapsedMs?: number;
  completedSegments?: number;
  totalSegments?: number;
}

export interface HotwordRule {
  id: string;
  source: string;
  target: string;
  enabled: boolean;
}

export interface UserSettings {
  shortcut: string;
  selectedModelId: string;
  modelDir: string;
  outputDir: string;
  pasteMode: PasteMode;
  recordingMode: RecordingMode;
  theme: ThemeMode;
  saveRecordings: boolean;
  launchAtStartup: boolean;
  showMiniWindow: boolean;
}

export interface TranscriptHistoryItem {
  id: string;
  text: string;
  createdAt: string;
  outputPath?: string;
  outputPaths: string[];
}

export interface ModelPreset {
  id: string;
  name: string;
  family: "sherpa" | "whisper" | "funasr" | "qwen";
  installKind: "sherpaOnnx" | "engineRequired";
  size: string;
  quality: string;
  memory: string;
  recommendedFor: string;
  license: string;
  downloadUrl: string;
  engineNote: string;
  archiveRoot?: string;
  modelFiles?: ModelFile[];
  sherpaArgs?: string;
}

export interface ModelFile {
  url: string;
  path: string;
}

export interface TranscribeFileResult {
  text: string;
  outputPath: string;
  outputPaths: string[];
  outputFiles: TranscriptionOutputFile[];
}

export interface TranscriptionOutputFile {
  format: ExportFormat;
  label: string;
  path: string;
}

export interface TranscriptionProgress {
  taskId: string;
  stage: "queued" | "transcoding" | "splitting" | "transcribing" | "exporting" | "done" | "failed";
  progress: number;
  message: string;
  completedSegments: number;
  totalSegments: number;
  elapsedMs: number;
}

export interface ModelValidationResult {
  valid: boolean;
  modelName: string;
  message: string;
}

export interface ModelInstallProgress {
  modelId: string;
  message: string;
  completed: number;
  total: number;
}

export interface DiagnosticItem {
  id: string;
  label: string;
  status: "ok" | "warning" | "error";
  detail: string;
}
