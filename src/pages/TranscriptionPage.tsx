import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Clock, Download, FileAudio, FolderOpen, Gauge, Trash2, Upload } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import { useEffect, useState } from "react";
import {
  listenTranscriptionProgress,
  saveExistingFile,
  selectAudioFiles,
  selectDirectory,
  transcribeFile,
} from "../lib/api";
import type {
  TranscriptTask,
  TranscriptionOutputFile,
  TranscriptionPerformanceMode,
  UserSettings,
} from "../types";

interface TranscriptionPageProps {
  tasks: TranscriptTask[];
  onTasksChange: Dispatch<SetStateAction<TranscriptTask[]>>;
  settings: UserSettings;
}

function fileNameFromPath(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function uniqueTaskId(filePath: string) {
  return `${Date.now()}-${Math.random().toString(16).slice(2)}-${filePath}`;
}

function suggestedNameFromPath(path: string) {
  const fileName = path.split(/[\\/]/).pop() || "hi-voicer-export.txt";
  return fileName.replace(/-\d{10,}(?=(-timeline)?\.)/, "");
}

function directoryFromPath(path: string | undefined) {
  if (!path) {
    return "";
  }

  const index = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return index >= 0 ? path.slice(0, index) : "";
}

function errorMessage(error: unknown, fallback: string) {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string" && error.trim()) {
    return error;
  }
  return fallback;
}

const performanceModes: Array<{
  id: TranscriptionPerformanceMode;
  label: string;
  description: string;
  concurrency: number;
}> = [
  { id: "stable", label: "稳定", description: "一次跑 1 个任务，适合长音频。", concurrency: 1 },
  { id: "balanced", label: "平衡", description: "推荐，同时跑 2 个任务。", concurrency: 2 },
  { id: "fast", label: "速度", description: "最多 3 个任务，适合高配置。", concurrency: 3 },
];

function formatElapsed(milliseconds: number) {
  const totalSeconds = Math.max(0, Math.floor(milliseconds / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) {
    return `${hours}:${minutes.toString().padStart(2, "0")}:${seconds.toString().padStart(2, "0")}`;
  }
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

export function TranscriptionPage({
  tasks,
  onTasksChange,
  settings,
}: TranscriptionPageProps) {
  const [isSelecting, setIsSelecting] = useState(false);
  const [isDragging, setIsDragging] = useState(false);
  const [performanceMode, setPerformanceMode] = useState<TranscriptionPerformanceMode>("balanced");
  const [exportDir, setExportDir] = useState("");
  const [now, setNow] = useState(() => Date.now());
  const hasActiveTasks = isSelecting || tasks.some((task) => task.status === "running");
  const downloadableTasks = tasks.filter((task) => task.outputFiles && task.outputFiles.length > 0);

  function updateTask(id: string, patch: Partial<TranscriptTask>) {
    onTasksChange((current) => current.map((task) => (task.id === id ? { ...task, ...patch } : task)));
  }

  function clearTasks() {
    if (hasActiveTasks) {
      return;
    }
    onTasksChange([]);
  }

  async function runTask(task: TranscriptTask) {
    const startedAt = new Date().toISOString();
    updateTask(task.id, {
      status: "running",
      progress: 2,
      message: "正在准备转录",
      startedAt,
      finishedAt: undefined,
      elapsedMs: undefined,
      outputPath: undefined,
      outputPaths: undefined,
      outputFiles: undefined,
    });

    try {
      const result = await transcribeFile(task.filePath || task.fileName, settings, {
        taskId: task.id,
        performanceMode,
      });
      const finishedAt = new Date().toISOString();
      updateTask(task.id, {
        status: "done",
        progress: 100,
        message: "已生成临时结果，选择需要的格式保存到音频文件夹。",
        outputPath: result.outputPath,
        outputPaths: result.outputPaths,
        outputFiles: result.outputFiles,
        finishedAt,
        elapsedMs: Date.parse(finishedAt) - Date.parse(startedAt),
      });
    } catch (error) {
      const finishedAt = new Date().toISOString();
      updateTask(task.id, {
        status: "failed",
        progress: 100,
        message: errorMessage(error, "转录失败"),
        finishedAt,
        elapsedMs: Date.parse(finishedAt) - Date.parse(startedAt),
      });
    }
  }

  async function transcribePaths(paths: string[]) {
    const audioPaths = paths.filter(Boolean);
    if (audioPaths.length === 0) {
      return;
    }

    setIsSelecting(true);
    try {
      const queuedTasks = audioPaths.map((filePath) => ({
        id: uniqueTaskId(filePath),
        fileName: fileNameFromPath(filePath),
        filePath,
        status: "queued" as const,
        progress: 0,
        outputFormats: ["txt", "srt"] as Array<"txt" | "srt">,
        message: "等待转录",
      }));
      onTasksChange((current) => [...queuedTasks, ...current.filter((item) => item.id !== "sample-1")]);

      let nextIndex = 0;
      const selectedMode = performanceModes.find((mode) => mode.id === performanceMode) ?? performanceModes[1];
      const workerCount = Math.min(selectedMode.concurrency, queuedTasks.length);
      const workers = Array.from({ length: workerCount }, async () => {
        while (nextIndex < queuedTasks.length) {
          const task = queuedTasks[nextIndex];
          nextIndex += 1;
          await runTask(task);
        }
      });

      await Promise.all(workers);
    } finally {
      setIsSelecting(false);
      setIsDragging(false);
    }
  }

  async function handleSelectFiles() {
    const files = await selectAudioFiles();
    await transcribePaths(files);
  }

  async function handleDownloadOutput(task: TranscriptTask, file: TranscriptionOutputFile) {
    updateTask(task.id, { message: `正在导出 ${file.label}...` });
    try {
      const savedPath = await saveExistingFile(
        file.path,
        suggestedNameFromPath(file.path),
        exportDir || directoryFromPath(task.filePath),
      );
      updateTask(task.id, {
        message: savedPath ? `已导出：${savedPath}` : "已取消导出。",
      });
    } catch (error) {
      updateTask(task.id, {
        message: errorMessage(error, "导出失败"),
      });
    }
  }

  async function handleSelectExportDir() {
    const selected = await selectDirectory();
    if (selected) {
      setExportDir(selected);
    }
  }

  async function handleBatchDownload() {
    if (downloadableTasks.length === 0) {
      return;
    }

    let savedCount = 0;
    for (const task of downloadableTasks) {
      for (const file of task.outputFiles ?? []) {
        const savedPath = await saveExistingFile(
          file.path,
          suggestedNameFromPath(file.path),
          exportDir || directoryFromPath(task.filePath),
        );
        if (savedPath) {
          savedCount += 1;
        }
      }
    }

    onTasksChange((current) =>
      current.map((task) =>
        task.outputFiles && task.outputFiles.length > 0
          ? { ...task, message: `批量导出完成：已保存 ${savedCount} 个文件。` }
          : task,
      ),
    );
  }

  useEffect(() => {
    const interval = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(interval);
  }, []);

  useEffect(() => {
    let unlisten = () => {};
    let disposed = false;

    void listenTranscriptionProgress((progress) => {
      if (disposed) {
        return;
      }
      updateTask(progress.taskId, {
        status: progress.stage === "failed" ? "failed" : progress.stage === "done" ? "done" : "running",
        progress: Math.round(progress.progress),
        message: progress.message,
        ...(progress.totalSegments > 0
          ? {
              completedSegments: progress.completedSegments,
              totalSegments: progress.totalSegments,
            }
          : {}),
        elapsedMs: progress.elapsedMs,
      });
    }).then((nextUnlisten) => {
      unlisten = nextUnlisten;
      if (disposed) {
        unlisten();
      }
    });

    return () => {
      disposed = true;
      unlisten();
    };
  }, []);

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
            void transcribePaths(event.payload.paths);
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
  }, [settings]);

  return (
    <div className="page-stack">
      <section className={`drop-zone ${isDragging ? "drop-zone--active" : ""}`}>
        <Upload size={24} />
        <h2>拖入音频或视频文件</h2>
        <p>支持 WAV、MP3、M4A、MP4 等常见格式；非 WAV 会自动转成 16kHz 单声道音频。</p>
        <div className="performance-mode">
          <span>
            <Gauge size={15} />
            转录性能
          </span>
          <div className="performance-mode__options">
            {performanceModes.map((mode) => (
              <button
                className={performanceMode === mode.id ? "performance-pill performance-pill--active" : "performance-pill"}
                disabled={isSelecting}
                key={mode.id}
                type="button"
                title={mode.description}
                onClick={() => setPerformanceMode(mode.id)}
              >
                {mode.label}
              </button>
            ))}
          </div>
        </div>
        <button className="primary-button" type="button" disabled={isSelecting} onClick={() => void handleSelectFiles()}>
          <FileAudio size={16} />
          {isSelecting ? "处理中..." : "选择文件"}
        </button>
      </section>

      <section className="panel">
        <div className="panel-heading panel-heading--compact">
          <div>
            <p className="section-label">任务队列</p>
            <p className="panel-hint">{exportDir ? `保存目录：${exportDir}` : "保存目录：原音频所在文件夹"}</p>
          </div>
          <div className="toolbar-actions">
            <button className="secondary-button" type="button" onClick={() => void handleSelectExportDir()}>
              <FolderOpen size={16} />
              选择保存目录
            </button>
            <button className="secondary-button" type="button" disabled={!exportDir} onClick={() => setExportDir("")}>
              使用音频目录
            </button>
            <button
              className="secondary-button"
              type="button"
              disabled={downloadableTasks.length === 0}
              onClick={() => void handleBatchDownload()}
            >
              <Download size={16} />
              批量下载
            </button>
            <button
              className="secondary-button"
              type="button"
              disabled={hasActiveTasks || tasks.length === 0}
              title={hasActiveTasks ? "转录进行中，暂不能清空任务" : "清空任务队列"}
              onClick={clearTasks}
            >
              <Trash2 size={16} />
              清空任务
            </button>
          </div>
        </div>
        {tasks.length === 0 ? (
          <p className="empty-state">还没有任务，选择音频或把文件拖到上方即可开始。</p>
        ) : (
          <div className="task-list">
            {tasks.map((task) => (
              <div className={`task-row task-row--${task.status}`} key={task.id}>
                <div>
                  <strong>{task.fileName}</strong>
                  <p>{task.message}</p>
                  <div className="task-progress" aria-label={`${task.fileName} 进度 ${task.progress}%`}>
                    <span style={{ width: `${Math.max(0, Math.min(100, task.progress))}%` }} />
                  </div>
                  <div className="task-meta">
                    <span>
                      <Clock size={13} />
                      {task.finishedAt && task.elapsedMs !== undefined
                        ? `用时 ${formatElapsed(task.elapsedMs)}`
                        : task.startedAt
                          ? `已用时 ${formatElapsed(now - Date.parse(task.startedAt))}`
                          : "等待开始"}
                    </span>
                    {task.totalSegments !== undefined && task.totalSegments > 0 && (
                      <span>
                        分段 {task.completedSegments ?? 0}/{task.totalSegments}
                      </span>
                    )}
                  </div>
                  {task.outputFiles && task.outputFiles.length > 0 && (
                    <div className="task-export-list">
                      {task.outputFiles.map((file) => (
                        <button
                          className="secondary-button"
                          key={file.format}
                          type="button"
                          onClick={() => void handleDownloadOutput(task, file)}
                        >
                          <Download size={15} />
                          {file.label}
                        </button>
                      ))}
                    </div>
                  )}
                  {task.outputPath && <small>临时结果已准备好，点击上方格式会保存到原音频所在文件夹。</small>}
                </div>
                <span>{task.progress}%</span>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
