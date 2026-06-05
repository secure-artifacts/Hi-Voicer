import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { FileAudio, FolderOpen, ListPlus, Play, SlidersHorizontal, Trash2, Upload } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { listAudioFilesInDirectory, prepareAudioPreview, processAudioFile, selectAudioFiles, selectDirectory } from "../lib/api";
import type { AudioProcessingOptions, AudioProcessingPreset } from "../types";

const presets: Array<{
  id: AudioProcessingPreset;
  title: string;
  description: string;
  options: AudioProcessingOptions;
}> = [
  {
    id: "normalize",
    title: "音量标准化",
    description: "适合声音忽大忽小的短录音。",
    options: { preset: "normalize", normalize: true, trimSilence: false, humReduction: false, voiceFilter: false, noiseReduction: false },
  },
  {
    id: "trimSilence",
    title: "静音裁剪",
    description: "裁掉开头和结尾较长静音。",
    options: { preset: "trimSilence", normalize: false, trimSilence: true, humReduction: false, voiceFilter: false, noiseReduction: false },
  },
  {
    id: "voiceBasic",
    title: "人声基础增强",
    description: "轻度降噪、滤波、标准化，优先保证转写。",
    options: { preset: "voiceBasic", normalize: true, trimSilence: true, humReduction: false, voiceFilter: true, noiseReduction: true },
  },
  {
    id: "humReduction",
    title: "电流声削弱",
    description: "削弱 50Hz/60Hz 低频嗡声和部分倍频。",
    options: { preset: "humReduction", normalize: true, trimSilence: false, humReduction: true, voiceFilter: true, noiseReduction: false },
  },
  {
    id: "lowHighPass",
    title: "语音滤波",
    description: "保留常见人声频段，减少低频震动和尖锐高频。",
    options: { preset: "lowHighPass", normalize: false, trimSilence: false, humReduction: false, voiceFilter: true, noiseReduction: false },
  },
];

type ProcessingStatus = "queued" | "running" | "done" | "failed";

interface ProcessingQueueItem {
  id: string;
  filePath: string;
  status: ProcessingStatus;
  message: string;
  outputPath?: string;
  previewPath?: string;
}

const AUDIO_PROCESSING_HISTORY_KEY = "hi-voicer-audio-processing-history";

function fileNameFromPath(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function audioPathToSrc(path: string) {
  try {
    return isTauri() ? convertFileSrc(path) : path;
  } catch {
    return path;
  }
}

function queueItemId(path: string) {
  return `${Date.now()}-${Math.random().toString(16).slice(2)}-${fileNameFromPath(path)}`;
}

function normalizeProcessingStatus(status: unknown): ProcessingStatus {
  if (status === "done" || status === "failed" || status === "queued") {
    return status;
  }
  return "queued";
}

function normalizeProcessingHistory(value: unknown): ProcessingQueueItem[] {
  if (!Array.isArray(value)) {
    return [];
  }

  return value.flatMap((item) => {
    if (!item || typeof item !== "object") {
      return [];
    }

    const source = item as Partial<ProcessingQueueItem>;
    if (typeof source.filePath !== "string" || !source.filePath.trim()) {
      return [];
    }

    const status = normalizeProcessingStatus(source.status);
    const outputPath = typeof source.outputPath === "string" && source.outputPath.trim() ? source.outputPath : undefined;
    const previewPath = typeof source.previewPath === "string" && source.previewPath.trim() ? source.previewPath : undefined;
    return [
      {
        id: typeof source.id === "string" && source.id.trim() ? source.id : queueItemId(source.filePath),
        filePath: source.filePath,
        status,
        message: typeof source.message === "string" && source.message.trim() ? source.message : "等待处理",
        outputPath,
        previewPath,
      },
    ];
  });
}

function loadProcessingHistory(): ProcessingQueueItem[] {
  try {
    const raw = window.localStorage.getItem(AUDIO_PROCESSING_HISTORY_KEY);
    return raw ? normalizeProcessingHistory(JSON.parse(raw)) : [];
  } catch {
    return [];
  }
}

function saveProcessingHistory(queue: ProcessingQueueItem[]) {
  try {
    if (queue.length === 0) {
      window.localStorage.removeItem(AUDIO_PROCESSING_HISTORY_KEY);
      return;
    }
    window.localStorage.setItem(AUDIO_PROCESSING_HISTORY_KEY, JSON.stringify(queue));
  } catch {
    // History is only a convenience cache; processing should still work without it.
  }
}

export function AudioProcessingPage() {
  const [selectedPresetId, setSelectedPresetId] = useState<AudioProcessingPreset>("voiceBasic");
  const [queue, setQueue] = useState<ProcessingQueueItem[]>(loadProcessingHistory);
  const [outputDir, setOutputDir] = useState("");
  const [activePreviewId, setActivePreviewId] = useState("");
  const [isProcessing, setIsProcessing] = useState(false);
  const [isDragging, setIsDragging] = useState(false);
  const [message, setMessage] = useState("");
  const previewAudioRefs = useRef<Record<string, HTMLAudioElement | null>>({});
  const preparingPreviewIds = useRef<Set<string>>(new Set());
  const selectedPreset = presets.find((preset) => preset.id === selectedPresetId) ?? presets[2];

  const addFilesToQueue = useCallback((paths: string[]) => {
    const cleanPaths = paths.map((path) => path.trim()).filter(Boolean);
    if (cleanPaths.length === 0) {
      return;
    }

    setQueue((current) => {
      const existing = new Set(current.map((item) => item.filePath));
      const nextItems = cleanPaths
        .filter((path) => !existing.has(path))
        .map((path) => ({
          id: queueItemId(path),
          filePath: path,
          status: "queued" as const,
          message: "等待处理",
        }));
      return [...current, ...nextItems];
    });
    setMessage(`已加入 ${cleanPaths.length} 个文件。`);
  }, []);

  async function handleSelectFiles() {
    try {
      addFilesToQueue(await selectAudioFiles());
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "选择文件失败。");
    }
  }

  async function handleSelectFolder() {
    try {
      const directory = await selectDirectory();
      if (!directory) {
        setMessage("已取消选择文件夹。");
        return;
      }
      const files = await listAudioFilesInDirectory(directory);
      if (files.length === 0) {
        setMessage("该文件夹里没有可处理的音频或视频文件。");
        return;
      }
      addFilesToQueue(files);
      setMessage(`已从文件夹加入 ${files.length} 个文件。`);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "选择文件夹失败。");
    }
  }

  async function handleSelectOutputDir() {
    try {
      const directory = await selectDirectory();
      if (directory) {
        setOutputDir(directory);
        setMessage(`输出目录：${directory}`);
      }
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "选择输出目录失败。");
    }
  }

  function updateQueueItem(id: string, patch: Partial<ProcessingQueueItem>) {
    setQueue((current) => current.map((item) => (item.id === id ? { ...item, ...patch } : item)));
  }

  async function handleProcess() {
    if (queue.length === 0) {
      setMessage("请先选择、拖放或导入文件夹。");
      return;
    }

    setIsProcessing(true);
    setMessage(`正在批量处理 ${queue.length} 个文件...`);
    for (const item of queue) {
      updateQueueItem(item.id, { status: "running", message: `正在处理 ${fileNameFromPath(item.filePath)}...` });
      try {
        const result = await processAudioFile(item.filePath, selectedPreset.options, {
          destinationDir: outputDir || undefined,
        });
        updateQueueItem(item.id, {
          status: "done",
          message: result.message,
          outputPath: result.outputPath,
          previewPath: undefined,
        });
      } catch (error) {
        updateQueueItem(item.id, {
          status: "failed",
          message: error instanceof Error ? error.message : "音频处理失败。",
        });
      }
    }
    setIsProcessing(false);
    setMessage("批量处理完成。");
  }

  function clearQueue() {
    setQueue([]);
    previewAudioRefs.current = {};
    setActivePreviewId("");
    setMessage("处理历史已清空。");
    try {
      window.localStorage.removeItem(AUDIO_PROCESSING_HISTORY_KEY);
    } catch {
      // Ignore unavailable local storage.
    }
  }

  function playPreview(id: string) {
    const audio = previewAudioRefs.current[id];
    if (!audio) {
      return;
    }

    setActivePreviewId(id);
    audio.currentTime = 0;
    void audio.play().catch(() => {
      setMessage("如果没有自动播放，可在该条下方播放器手动播放。");
    });
  }

  const ensureAudioPreview = useCallback((item: ProcessingQueueItem) => {
    if (!item.outputPath || item.previewPath || preparingPreviewIds.current.has(item.id)) {
      return;
    }

    preparingPreviewIds.current.add(item.id);
    void prepareAudioPreview(item.outputPath)
      .then((previewPath) => {
        updateQueueItem(item.id, { previewPath });
      })
      .catch((error) => {
        updateQueueItem(item.id, {
          message: error instanceof Error ? `试听准备失败：${error.message}` : "试听准备失败。",
        });
      })
      .finally(() => {
        preparingPreviewIds.current.delete(item.id);
      });
  }, []);

  useEffect(() => {
    saveProcessingHistory(queue);
    queue.forEach(ensureAudioPreview);
  }, [ensureAudioPreview, queue]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;

    try {
      void getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type === "enter" || event.payload.type === "over") {
            setIsDragging(true);
            return;
          }
          if (event.payload.type === "leave") {
            setIsDragging(false);
            return;
          }
          if (event.payload.type === "drop") {
            setIsDragging(false);
            addFilesToQueue(event.payload.paths);
          }
        })
        .then((nextUnlisten) => {
          if (disposed) {
            nextUnlisten();
          } else {
            unlisten = nextUnlisten;
          }
        })
        .catch(() => {});
    } catch {
      // Browser preview does not expose the Tauri webview drag/drop API.
    }

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [addFilesToQueue]);

  return (
    <div className="page-stack audio-processing-page">
      <section className={`panel audio-drop-panel ${isDragging ? "audio-drop-panel--active" : ""}`}>
        <div className="panel-heading">
          <div>
            <p className="section-label">音频处理</p>
            <h2>轻量处理人物说话录音</h2>
            <p className="panel-hint">可拖放单个或多个文件，也可导入整个文件夹；默认输出到原文件所在目录。</p>
          </div>
          <div className="button-group">
            <button className="secondary-button" type="button" disabled={isProcessing} onClick={() => void handleSelectFiles()}>
              <Upload size={16} />
              选择文件
            </button>
            <button className="secondary-button" type="button" disabled={isProcessing} onClick={() => void handleSelectFolder()}>
              <FolderOpen size={16} />
              选择文件夹
            </button>
            <button className="secondary-button" type="button" disabled={isProcessing} onClick={() => void handleSelectOutputDir()}>
              <FolderOpen size={16} />
              自定义输出目录
            </button>
          </div>
        </div>
        <div className="audio-processing-source">
          <ListPlus size={18} />
          <span>{outputDir ? `自定义输出目录：${outputDir}` : "默认输出到每个原始文件所在目录"}</span>
        </div>
      </section>

      <section className="panel">
        <p className="section-label">处理预设</p>
        <div className="audio-preset-grid">
          {presets.map((preset) => (
            <button
              className={selectedPresetId === preset.id ? "audio-preset audio-preset--active" : "audio-preset"}
              key={preset.id}
              type="button"
              onClick={() => setSelectedPresetId(preset.id)}
            >
              <strong>{preset.title}</strong>
              <span>{preset.description}</span>
            </button>
          ))}
        </div>
        <div className="audio-action-row">
          <button
            className="primary-button audio-process-button"
            type="button"
            disabled={isProcessing || queue.length === 0}
            onClick={() => void handleProcess()}
          >
            <SlidersHorizontal size={16} />
            {isProcessing ? "处理中..." : `批量运行：${selectedPreset.title}`}
          </button>
          <button className="secondary-button" type="button" disabled={isProcessing || queue.length === 0} onClick={clearQueue}>
            <Trash2 size={16} />
            清空历史
          </button>
        </div>
        {message && <p className="model-message">{message}</p>}
      </section>

      <section className="panel audio-queue-panel">
        <div className="panel-heading panel-heading--compact">
          <div>
            <p className="section-label">处理队列</p>
            <p className="panel-hint">已加入 {queue.length} 个文件。</p>
          </div>
        </div>
        {queue.length === 0 ? (
          <div className="audio-queue-empty">
            <FileAudio size={20} />
            <span>拖放文件、选择文件，或选择文件夹加入队列。</span>
          </div>
        ) : (
          <div className="audio-processing-queue">
            {queue.map((item) => (
              <div
                className={`audio-queue-row audio-queue-row--${item.status} ${
                  activePreviewId === item.id ? "audio-queue-row--previewing" : ""
                }`}
                key={item.id}
              >
                <FileAudio size={18} />
                <div>
                  <strong>{item.filePath}</strong>
                  <span>{item.outputPath ? `${item.message}：${item.outputPath}` : item.message}</span>
                </div>
                <button
                  className="secondary-button"
                  type="button"
                  disabled={!item.previewPath}
                  onClick={() => playPreview(item.id)}
                >
                  <Play size={16} />
                  试听
                </button>
                {item.outputPath && (
                  <div className="audio-row-preview">
                    <span>处理结果：{item.outputPath}</span>
                    {item.previewPath ? (
                      <audio
                        controls
                        preload="metadata"
                        ref={(node) => {
                          previewAudioRefs.current[item.id] = node;
                        }}
                        src={audioPathToSrc(item.previewPath)}
                      />
                    ) : (
                      <span>试听准备中...</span>
                    )}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
