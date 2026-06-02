import type { AppStatus, DiagnosticItem, HotwordRule, TranscriptTask, UserSettings } from "../types";

export const initialStatus: AppStatus = {
  readiness: "model-required",
  modelName: "未配置模型",
  shortcut: "CapsLock",
  microphoneName: "默认麦克风",
  lastResult: "模型配置完成后，这里会显示最近一次识别结果。",
  isRecording: false,
  recordingMode: "hold",
};

export const initialTasks: TranscriptTask[] = [
  {
    id: "sample-1",
    fileName: "会议录音示例.wav",
    status: "queued",
    progress: 0,
    outputFormats: ["txt"],
    message: "等待选择文件",
  },
];

export const initialHotwords: HotwordRule[] = [
  { id: "rule-1", source: "陶瑞", target: "Tauri", enabled: true },
  { id: "rule-2", source: "阿萨尔", target: "ASR", enabled: true },
];

export const initialSettings: UserSettings = {
  shortcut: "CapsLock",
  selectedModelId: "sensevoice-small",
  modelDir: "",
  outputDir: "",
  pasteMode: "clipboard",
  recordingMode: "hold",
  accelerationMode: "cpu",
  theme: "light",
  saveRecordings: false,
  launchAtStartup: false,
  showMiniWindow: true,
};

export const initialDiagnostics: DiagnosticItem[] = [
  {
    id: "model",
    label: "模型",
    status: "warning",
    detail: "尚未选择本地模型目录。",
  },
  {
    id: "microphone",
    label: "麦克风",
    status: "ok",
    detail: "使用系统默认麦克风。",
  },
  {
    id: "shortcut",
    label: "快捷键",
    status: "ok",
    detail: "默认快捷键 CapsLock 可用于语音输入。",
  },
];
