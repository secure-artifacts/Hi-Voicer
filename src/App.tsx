import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { AppShell } from "./components/AppShell";
import { initialDiagnostics, initialHotwords, initialSettings, initialStatus, initialTasks, initialTermCategories } from "./data/mockState";
import { findModelPreset } from "./data/modelPresets";
import {
  loadSettings,
  listenMiniToggleRecording,
  listenRecordingError,
  listenRecordingLevel,
  listenRecordingState,
  listenTranscriptionResult,
  openRecordingsFolder,
  requestMainRecordingToggle,
  saveSettings,
  saveTextFile,
  startRecording,
  stopRecording,
  validateModelDir,
} from "./lib/api";
import { normalizeHotwordRules, normalizeTermCategories } from "./lib/termLibrary";
import { DiagnosticsPage } from "./pages/DiagnosticsPage";
import { HomePage } from "./pages/HomePage";
import { HotwordsPage } from "./pages/HotwordsPage";
import { AudioProcessingPage } from "./pages/AudioProcessingPage";
import { SettingsPage } from "./pages/SettingsPage";
import { SubtitleEditorPage } from "./pages/SubtitleEditorPage";
import { TranscriptionPage } from "./pages/TranscriptionPage";
import type {
  AppPage,
  AppStatus,
  ModelValidationResult,
  SubtitleSegment,
  TimelineKind,
  TranscriptHistoryItem,
  TranscriptTask,
  UserSettings,
} from "./types";

const HISTORY_KEY = "hi-voicer-transcript-history";
const HOTWORDS_KEY = "hi-voicer-hotwords";
const MAX_HISTORY_ITEMS = 20;

function loadTranscriptHistory() {
  try {
    const raw = window.localStorage.getItem(HISTORY_KEY);
    return raw ? (JSON.parse(raw) as TranscriptHistoryItem[]) : [];
  } catch {
    return [];
  }
}

function saveTranscriptHistory(history: TranscriptHistoryItem[]) {
  window.localStorage.setItem(HISTORY_KEY, JSON.stringify(history));
}

function loadHotwords(termCategories: UserSettings["termCategories"]) {
  try {
    const raw = window.localStorage.getItem(HOTWORDS_KEY);
    if (!raw) {
      return null;
    }

    const rules = normalizeHotwordRules(JSON.parse(raw), termCategories);
    return rules.length > 0 ? rules : null;
  } catch {
    window.localStorage.removeItem(HOTWORDS_KEY);
    return null;
  }
}

function saveHotwords(rules: UserSettings["hotwords"]) {
  window.localStorage.setItem(HOTWORDS_KEY, JSON.stringify(rules));
}

function getInitialWindowLabel() {
  try {
    const label = getCurrentWindow().label;
    document.documentElement.dataset.window = label;
    return label;
  } catch {
    document.documentElement.dataset.window = "main";
    return "main";
  }
}

export default function App() {
  const [windowLabel] = useState(getInitialWindowLabel);
  const [currentPage, setCurrentPage] = useState<AppPage>("home");
  const [settings, setSettings] = useState<UserSettings>(initialSettings);
  const [lastResult, setLastResult] = useState(initialStatus.lastResult);
  const [isRecording, setIsRecording] = useState(false);
  const [recordingLevel, setRecordingLevel] = useState(0);
  const [transcriptHistory, setTranscriptHistory] = useState<TranscriptHistoryItem[]>(() => loadTranscriptHistory());
  const [transcriptionTasks, setTranscriptionTasks] = useState<TranscriptTask[]>(initialTasks);
  const [subtitleProject, setSubtitleProject] = useState<{
    fileName: string;
    sourceAudioPath: string;
    text: string;
    segments: SubtitleSegment[];
    timelineKind?: TimelineKind;
  } | null>(null);
  const miniDragRef = useRef({ x: 0, y: 0, dragging: false });
  const settingsSaveVersionRef = useRef(0);
  const [modelValidation, setModelValidation] = useState<ModelValidationResult>({
    valid: false,
    modelName: "",
    message: "尚未配置离线模型。",
  });

  useEffect(() => {
    if (windowLabel !== "main") {
      return;
    }
    void loadSettings(initialSettings).then((loadedSettings) => {
      const termCategories = normalizeTermCategories(loadedSettings.termCategories);
      const hotwords = loadHotwords(termCategories);
      const nextSettings = {
        ...loadedSettings,
        recordingSource: loadedSettings.recordingSource ?? "microphone",
        termCategories,
        hotwords: hotwords ?? normalizeHotwordRules(loadedSettings.hotwords ?? initialHotwords, termCategories),
      };
      setSettings(nextSettings);
      void refreshModelValidation(nextSettings);
    });
  }, [windowLabel]);

  useEffect(() => {
    document.documentElement.dataset.theme = settings.theme;
  }, [settings.theme]);

  async function refreshModelValidation(nextSettings: UserSettings) {
    const validation = await validateModelDir(nextSettings.modelDir);
    setModelValidation(validation);
    if (nextSettings.modelDir && !validation.valid) {
      setLastResult(validation.message);
    }
  }

  function commitSettings(nextSettings: UserSettings) {
    const version = settingsSaveVersionRef.current + 1;
    settingsSaveVersionRef.current = version;
    setSettings(nextSettings);
    void saveSettings(nextSettings).then((savedSettings) => {
      if (settingsSaveVersionRef.current !== version) {
        return;
      }
      setSettings(savedSettings);
      void refreshModelValidation(savedSettings);
    });
  }

  function handleSettingsChange(nextSettings: UserSettings) {
    commitSettings(nextSettings);
  }

  function updateSettings(updater: (current: UserSettings) => UserSettings) {
    setSettings((current) => {
      const nextSettings = updater(current);
      const version = settingsSaveVersionRef.current + 1;
      settingsSaveVersionRef.current = version;
      void saveSettings(nextSettings).then((savedSettings) => {
        if (settingsSaveVersionRef.current !== version) {
          return;
        }
        setSettings(savedSettings);
        void refreshModelValidation(savedSettings);
      });
      return nextSettings;
    });
  }

  function handleHotwordsChange(hotwords: UserSettings["hotwords"]) {
    saveHotwords(hotwords);
    updateSettings((current) => ({ ...current, hotwords }));
  }

  function handleTermCategoriesChange(termCategories: UserSettings["termCategories"]) {
    updateSettings((current) => ({ ...current, termCategories }));
  }

  function handleAddTermRule(rule: UserSettings["hotwords"][number]) {
    updateSettings((current) => {
      const hotwords = [rule, ...current.hotwords];
      saveHotwords(hotwords);
      return { ...current, hotwords };
    });
  }

  function clearTranscriptHistory() {
    setTranscriptHistory([]);
    saveTranscriptHistory([]);
    setLastResult("录制文字历史已清空。");
  }

  function handleRecordingModeChange(recordingMode: UserSettings["recordingMode"]) {
    handleSettingsChange({ ...settings, recordingMode });
  }

  function appendTranscriptHistory(result: { text: string; outputPath?: string; outputPaths?: string[] }) {
    const text = result.text.trim();
    if (!text) {
      return;
    }

    setTranscriptHistory((current) => {
      const next = [
        {
          id: `${Date.now()}-${Math.random().toString(16).slice(2)}`,
          text,
          createdAt: new Date().toISOString(),
          outputPath: result.outputPath,
          outputPaths: result.outputPaths ?? [],
        },
        ...current,
      ].slice(0, MAX_HISTORY_ITEMS);
      saveTranscriptHistory(next);
      return next;
    });
  }

  async function handleOpenRecordingsFolder() {
    try {
      const folder = await openRecordingsFolder();
      setLastResult(`已打开录音文件夹：${folder}`);
    } catch (error) {
      setLastResult(error instanceof Error ? error.message : "打开录音文件夹失败。");
    }
  }

  async function handleToggleRecording() {
    const isAudioOnly = settings.recordingMode === "audioOnly";
    if (!isAudioOnly && !modelValidation.valid) {
      setLastResult(modelValidation.message || "请先到设置里下载并配置离线模型。");
      setCurrentPage("settings");
      return;
    }

    if (!isRecording) {
      try {
        await startRecording();
        setIsRecording(true);
        setLastResult(isAudioOnly ? "正在纯录音，停止后会保存音频文件。" : "正在录音，停止后开始识别。");
      } catch (error) {
        setLastResult(error instanceof Error ? error.message : "录音启动失败。");
      }
      return;
    }

    try {
      setIsRecording(false);
      setLastResult(isAudioOnly ? "正在保存录音..." : "正在识别...");
      const result = await stopRecording();
      setLastResult(result.text);
      appendTranscriptHistory(result);
    } catch (error) {
      setLastResult(error instanceof Error ? error.message : "录音结束失败。");
    }
  }

  useEffect(() => {
    let disposed = false;
    let unlistenRecording = () => {};
    let unlistenRecordingError = () => {};
    let unlistenLevel = () => {};
    let unlistenResult = () => {};
    let unlistenMiniToggle = () => {};

    void listenRecordingState((nextIsRecording) => {
      if (!disposed) {
        setIsRecording(nextIsRecording);
      }
    }).then((unlisten) => {
      unlistenRecording = unlisten;
      if (disposed) {
        unlistenRecording();
      }
    });

    void listenRecordingLevel((level) => {
      if (!disposed) {
        setRecordingLevel(level);
      }
    }).then((unlisten) => {
      unlistenLevel = unlisten;
      if (disposed) {
        unlistenLevel();
      }
    });

    void listenRecordingError((message) => {
      if (!disposed) {
        setIsRecording(false);
        setLastResult(message || "录音失败。");
      }
    }).then((unlisten) => {
      unlistenRecordingError = unlisten;
      if (disposed) {
        unlistenRecordingError();
      }
    });

    if (windowLabel === "main") {
      void listenTranscriptionResult((result) => {
        if (!disposed) {
          setLastResult(result.text);
          appendTranscriptHistory(result);
        }
      }).then((unlisten) => {
        unlistenResult = unlisten;
        if (disposed) {
          unlistenResult();
        }
      });

      void listenMiniToggleRecording(() => {
        if (!disposed) {
          void handleToggleRecording();
        }
      }).then((unlisten) => {
        unlistenMiniToggle = unlisten;
        if (disposed) {
          unlistenMiniToggle();
        }
      });
    }

    return () => {
      disposed = true;
      unlistenRecording();
      unlistenRecordingError();
      unlistenLevel();
      unlistenResult();
      unlistenMiniToggle();
    };
  }, [windowLabel, isRecording, settings, modelValidation]);

  const selectedModel = findModelPreset(settings.selectedModelId);
  const status: AppStatus = {
    ...initialStatus,
    readiness: modelValidation.valid || settings.recordingMode === "audioOnly" ? "ready" : "model-required",
    shortcut: settings.shortcut,
    modelName: modelValidation.valid ? modelValidation.modelName || selectedModel?.name || "本地模型" : "未配置模型",
    lastResult,
    isRecording,
    recordingMode: settings.recordingMode,
  };
  const diagnostics = initialDiagnostics.map((item) =>
    item.id === "model"
      ? {
          ...item,
          status: modelValidation.valid ? ("ok" as const) : ("warning" as const),
          detail:
            settings.recordingMode === "audioOnly" && !modelValidation.valid
              ? "纯录音模式不需要模型；识别模式仍需配置模型。"
              : modelValidation.message,
        }
      : item,
  );
  const waveHeights = [0.72, 1.1, 1.45, 1.05, 0.82].map((factor) =>
    Math.max(12, Math.round(14 + recordingLevel * factor * 30)),
  );

  if (windowLabel === "wave") {
    return (
      <div className="desktop-wave-window" aria-live="polite">
        {waveHeights.map((height, index) => (
          <span key={index} style={{ height: `${height}px` }} />
        ))}
      </div>
    );
  }

  if (windowLabel === "mini") {
    return (
      <button
        className={isRecording ? "mini-window mini-window--recording" : "mini-window"}
        type="button"
        title={isRecording ? "停止录制" : "开始录制"}
        onPointerDown={(event) => {
          miniDragRef.current = { x: event.clientX, y: event.clientY, dragging: false };
        }}
        onPointerMove={(event) => {
          const drag = miniDragRef.current;
          if (!drag.dragging && Math.hypot(event.clientX - drag.x, event.clientY - drag.y) > 4) {
            drag.dragging = true;
            void getCurrentWindow().startDragging();
          }
        }}
        onClick={(event) => {
          if (miniDragRef.current.dragging) {
            event.preventDefault();
            miniDragRef.current.dragging = false;
            return;
          }
          void requestMainRecordingToggle();
        }}
      >
        <span>{isRecording ? "停" : "录"}</span>
      </button>
    );
  }

  return (
    <AppShell status={status} currentPage={currentPage} onPageChange={setCurrentPage}>
      {currentPage === "home" && (
        <HomePage
          status={status}
          onOpenSettings={() => setCurrentPage("settings")}
          onOpenRecordingsFolder={() => void handleOpenRecordingsFolder()}
          onToggleRecording={() => void handleToggleRecording()}
          onRecordingModeChange={handleRecordingModeChange}
          recordingLevel={recordingLevel}
          transcriptHistory={transcriptHistory}
          onCopyTranscript={(text) => {
            void navigator.clipboard.writeText(text);
            setLastResult("已复制历史文字。");
          }}
          onDownloadTranscript={(item) => {
            void saveTextFile(`hi-voicer-${item.createdAt.replace(/[:.]/g, "-")}.txt`, item.text).then((path) => {
              setLastResult(path ? `已保存历史文字：${path}` : "已取消保存历史文字。");
            });
          }}
          onClearTranscriptHistory={clearTranscriptHistory}
        />
      )}
      {currentPage === "transcription" && (
        <TranscriptionPage
          tasks={transcriptionTasks}
          onTasksChange={setTranscriptionTasks}
          settings={settings}
          onOpenSubtitleEditor={(task) => {
            setSubtitleProject({
              fileName: task.fileName,
              sourceAudioPath: task.sourceAudioPath || task.filePath || "",
              text: task.text || "",
              segments: task.segments ?? [],
              timelineKind: task.timelineKind,
            });
            setCurrentPage("subtitles");
          }}
        />
      )}
      {currentPage === "subtitles" && (
        <SubtitleEditorPage
          project={subtitleProject}
          onProjectChange={setSubtitleProject}
          termCategories={settings.termCategories}
          onAddTermRule={handleAddTermRule}
        />
      )}
      {currentPage === "hotwords" && (
        <HotwordsPage
          rules={settings.hotwords}
          categories={settings.termCategories}
          onRulesChange={handleHotwordsChange}
          onCategoriesChange={handleTermCategoriesChange}
        />
      )}
      {currentPage === "audio-processing" && <AudioProcessingPage />}
      {currentPage === "settings" && (
        <SettingsPage
          settings={settings}
          onOpenRecordingsFolder={() => void handleOpenRecordingsFolder()}
          onSettingsChange={handleSettingsChange}
        />
      )}
      {currentPage === "diagnostics" && (
        <DiagnosticsPage items={diagnostics} modelReady={modelValidation.valid} settings={settings} />
      )}
    </AppShell>
  );
}
