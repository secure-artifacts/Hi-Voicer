export type ReadinessState =
  | "starting"
  | "loading-model"
  | "ready"
  | "model-required"
  | "microphone-unavailable"
  | "error";

export type AppPage = "home" | "transcription" | "subtitles" | "hotwords" | "audio-processing" | "settings" | "diagnostics";

export type PasteMode = "direct" | "clipboard";
export type RecordingMode = "hold" | "toggle" | "audioOnly";
export type RecordingSource = "microphone" | "system" | "microphoneAndSystem";
export type AccelerationMode = "cpu" | "cuda";
export type ExportFormat = "plainText" | "timelineText" | "timelineTxt" | "srt" | "resolveMarkers";
export type ThemeMode = "light" | "dark";
export type TranscriptionPerformanceMode = "stable" | "balanced" | "fast";
export type TimelineKind = "estimated" | "model";
export type AudioProcessingPreset = "normalize" | "trimSilence" | "voiceBasic" | "humReduction" | "lowHighPass";
export type AudioOutputFormat = "wav" | "mp3" | "m4a" | "aac" | "flac" | "ogg" | "opus";
export type AudioMergeMode = "copy" | "reencode";

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
  outputFormats: Array<"txt" | "timelineTxt" | "srt" | "json" | "edl">;
  message: string;
  outputPath?: string;
  outputPaths?: string[];
  outputFiles?: TranscriptionOutputFile[];
  startedAt?: string;
  finishedAt?: string;
  elapsedMs?: number;
  completedSegments?: number;
  totalSegments?: number;
  text?: string;
  segments?: SubtitleSegment[];
  timelineKind?: TimelineKind;
  sourceAudioPath?: string;
}

export interface HotwordRule {
  id: string;
  source: string;
  target: string;
  enabled: boolean;
  categoryId?: string;
  hitCount?: number;
  lastUsedAt?: string;
}

export interface TermCategory {
  id: string;
  name: string;
  order: number;
}

export interface UserSettings {
  shortcut: string;
  selectedModelId: string;
  modelDir: string;
  inputModelId: string;
  inputModelDir: string;
  transcriptionModelId: string;
  transcriptionModelDir: string;
  outputDir: string;
  pasteMode: PasteMode;
  recordingMode: RecordingMode;
  recordingSource: RecordingSource;
  accelerationMode: AccelerationMode;
  hotwords: HotwordRule[];
  termCategories: TermCategory[];
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
  segments: SubtitleSegment[];
  timelineKind: TimelineKind;
  sourceAudioPath: string;
}

export interface TranscriptionOutputFile {
  format: ExportFormat;
  label: string;
  path: string;
}

export interface SubtitleSegment {
  id: string;
  index: number;
  start: number;
  end: number;
  text: string;
  sourceAudioPath: string;
}

export interface AudioProcessingOptions {
  preset: AudioProcessingPreset;
  normalize: boolean;
  trimSilence: boolean;
  humReduction: boolean;
  voiceFilter: boolean;
  noiseReduction: boolean;
}

export interface AudioProcessingResult {
  outputPath: string;
  message: string;
}

export interface ProbeMediaFrameRateResult {
  fps: number;
  source: "video" | "fallback";
  message: string;
}

export interface AudioWaveformResult {
  waveformPath: string;
  durationSeconds: number;
  message: string;
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

export interface AccelerationStatus {
  selectedMode: AccelerationMode;
  effectiveMode: AccelerationMode;
  cudaAvailable: boolean;
  cudaDeviceSummary?: string | null;
  cudaDetectionError?: string | null;
  cpuRuntimeInstalled: boolean;
  cudaRuntimeInstalled: boolean;
  cudaDisabledReason?: string | null;
  message: string;
}

export interface AccelerationSmokeTestResult {
  requestedMode: AccelerationMode;
  usedMode: AccelerationMode;
  fallbackUsed: boolean;
  elapsedMs: number;
  transcriptPreview: string;
  message: string;
}

export interface NativeAudioDiagnostics {
  microphoneAvailable: boolean;
  microphoneName?: string | null;
  microphoneDetail?: string | null;
  systemAudioAvailable: boolean;
  systemAudioName?: string | null;
  systemAudioDetail?: string | null;
  ffmpegInstalled: boolean;
  ffmpegPath?: string | null;
  ffmpegDetail?: string | null;
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
