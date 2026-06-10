import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  ArrowDown,
  ArrowUp,
  FileAudio,
  FolderOpen,
  ListPlus,
  Maximize2,
  Merge,
  Play,
  Scissors,
  SlidersHorizontal,
  Trash2,
  Upload,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type {
  CSSProperties,
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
  SyntheticEvent,
} from "react";
import {
  clipAudioSegments,
  convertAudioFile,
  listAudioFilesInDirectory,
  mergeAudioFiles,
  openOutputFolder,
  prepareAudioPreview,
  prepareAudioWaveform,
  probeMediaFrameRate,
  processAudioFile,
  selectAudioFiles,
  selectDirectory,
  splitAudioFile,
} from "../lib/api";
import type { AudioMergeMode, AudioOutputFormat, AudioProcessingOptions, AudioProcessingPreset } from "../types";

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

const outputFormats: Array<{ id: AudioOutputFormat; label: string }> = [
  { id: "wav", label: "WAV" },
  { id: "mp3", label: "MP3" },
  { id: "m4a", label: "M4A" },
  { id: "aac", label: "AAC" },
  { id: "flac", label: "FLAC" },
  { id: "ogg", label: "OGG" },
  { id: "opus", label: "OPUS" },
];

const tools: Array<{ id: AudioTool; label: string; description: string }> = [
  { id: "enhance", label: "降噪/增强", description: "标准化、滤波、静音裁剪和基础降噪。" },
  { id: "convert", label: "格式转换", description: "音频互转，视频提取音频。" },
  { id: "clip", label: "音频剪辑", description: "秒级/帧级片段截取和批量切分。" },
  { id: "merge", label: "音频合并", description: "按顺序合并，支持无需重编码或重新编码。" },
];

type AudioTool = "enhance" | "convert" | "clip" | "merge";
type ProcessingStatus = "queued" | "running" | "done" | "failed";
type ClipMode = "multi" | "split";
type ClipExportMode = "separate" | "merged";
type SplitUnit = "seconds" | "frames";
type TimelineDragTarget = "start" | "end" | "playhead";

interface ProcessingQueueItem {
  id: string;
  filePath: string;
  status: ProcessingStatus;
  message: string;
  outputPath?: string;
  outputPaths?: string[];
  previewPath?: string;
}

interface ClipSegment {
  id: string;
  startSeconds: number;
  endSeconds: number;
}

const clipSegmentColors = ["#2dd4bf", "#f59e0b", "#8b5cf6", "#38bdf8", "#f472b6", "#84cc16"];
const AUDIO_PROCESSING_HISTORY_KEY = "hi-voicer-audio-processing-history";
const MIN_TIMELINE_WINDOW_SECONDS = 2;

function fileNameFromPath(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function stemFromPath(path: string) {
  return fileNameFromPath(path).replace(/\.[^.]+$/, "") || "audio";
}

function queueItemId(path: string) {
  return `${Date.now()}-${Math.random().toString(16).slice(2)}-${fileNameFromPath(path)}`;
}

function audioPathToSrc(path: string) {
  try {
    return isTauri() ? convertFileSrc(path) : path;
  } catch {
    return path;
  }
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

    const outputPath = typeof source.outputPath === "string" && source.outputPath.trim() ? source.outputPath : undefined;
    const rawOutputPaths = Array.isArray(source.outputPaths) ? source.outputPaths.filter((path) => typeof path === "string") : undefined;
    const outputPaths =
      rawOutputPaths && rawOutputPaths.length > 10
        ? [...rawOutputPaths.slice(0, 10), `... 另有 ${rawOutputPaths.length - 10} 个文件`]
        : rawOutputPaths;
    const previewPath = typeof source.previewPath === "string" && source.previewPath.trim() ? source.previewPath : undefined;
    return [
      {
        id: typeof source.id === "string" && source.id.trim() ? source.id : queueItemId(source.filePath),
        filePath: source.filePath,
        status: normalizeProcessingStatus(source.status),
        message: typeof source.message === "string" && source.message.trim() ? source.message : "等待处理",
        outputPath,
        outputPaths,
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

function formatSeconds(value: number) {
  return Number.isFinite(value) ? Math.max(0, value).toFixed(3) : "0.000";
}

function formatClockTime(seconds: number) {
  const totalMilliseconds = Math.max(0, Math.round((Number.isFinite(seconds) ? seconds : 0) * 1000));
  const hours = Math.floor(totalMilliseconds / 3_600_000);
  const minutes = Math.floor((totalMilliseconds % 3_600_000) / 60_000);
  const wholeSeconds = Math.floor((totalMilliseconds % 60_000) / 1000);
  const milliseconds = totalMilliseconds % 1000;
  return `${hours.toString().padStart(2, "0")}:${minutes.toString().padStart(2, "0")}:${wholeSeconds
    .toString()
    .padStart(2, "0")}.${milliseconds.toString().padStart(3, "0")}`;
}

function formatTimecode(seconds: number, fps: number) {
  const safeFps = Math.max(1, Math.round(fps || 25));
  const totalFrames = Math.max(0, Math.round(seconds * safeFps));
  const frames = totalFrames % safeFps;
  const totalSeconds = Math.floor(totalFrames / safeFps);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const secs = totalSeconds % 60;
  return `${hours.toString().padStart(2, "0")}:${minutes.toString().padStart(2, "0")}:${secs
    .toString()
    .padStart(2, "0")}:${frames.toString().padStart(2, "0")}`;
}

function parseTimecode(value: string, fps: number) {
  const safeFps = Math.max(1, Math.round(fps || 25));
  const parts = value.split(":").map((part) => Number(part));
  if (parts.length !== 4 || parts.some((part) => !Number.isFinite(part) || part < 0)) {
    return null;
  }
  const [hours, minutes, seconds, frames] = parts;
  if (minutes >= 60 || seconds >= 60 || frames >= safeFps) {
    return null;
  }
  return hours * 3600 + minutes * 60 + seconds + frames / safeFps;
}

function parseClockTime(value: string, fps: number) {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  if (/^\d+(\.\d+)?$/.test(trimmed)) {
    return Number(trimmed);
  }

  const parts = trimmed.split(":");
  if (parts.length === 4) {
    return parseTimecode(trimmed, fps);
  }
  if (parts.length < 2 || parts.length > 3) {
    return null;
  }

  const [rawHours, rawMinutes, rawSeconds] = parts.length === 3 ? parts : ["0", parts[0], parts[1]];
  const hours = Number(rawHours);
  const minutes = Number(rawMinutes);
  const seconds = Number(rawSeconds);
  if (
    !Number.isFinite(hours) ||
    !Number.isFinite(minutes) ||
    !Number.isFinite(seconds) ||
    hours < 0 ||
    minutes < 0 ||
    seconds < 0 ||
    minutes >= 60 ||
    seconds >= 60
  ) {
    return null;
  }

  return hours * 3600 + minutes * 60 + seconds;
}

function createClipSegment(startSeconds = 0, endSeconds = 10): ClipSegment {
  return {
    id: `clip-${Date.now()}-${Math.random().toString(16).slice(2)}`,
    startSeconds,
    endSeconds,
  };
}

function suggestedClipName(sourcePath: string, index: number, outputFormat: AudioOutputFormat) {
  return `${stemFromPath(sourcePath)}-clip-${index.toString().padStart(3, "0")}.${outputFormat}`;
}

function isLikelySupportedMediaPath(path: string) {
  return /\.(wav|mp3|m4a|aac|flac|ogg|opus|wma|mp4|mov|mkv|webm|avi)$/i.test(path.trim());
}

function displayOutputPaths(paths?: string[]) {
  if (!paths || paths.length <= 10) {
    return paths;
  }
  return [...paths.slice(0, 10), `... 另有 ${paths.length - 10} 个文件`];
}

function isVideoPath(path: string) {
  return /\.(mp4|mov|mkv|webm|avi)$/i.test(path.trim());
}

function clampSeconds(value: number, duration: number) {
  const max = Number.isFinite(duration) && duration > 0 ? duration : Number.MAX_SAFE_INTEGER;
  return Math.min(max, Math.max(0, Number.isFinite(value) ? value : 0));
}

export function AudioProcessingPage() {
  const [activeTool, setActiveTool] = useState<AudioTool>("enhance");
  const [selectedPresetId, setSelectedPresetId] = useState<AudioProcessingPreset>("voiceBasic");
  const [queue, setQueue] = useState<ProcessingQueueItem[]>(loadProcessingHistory);
  const [outputDir, setOutputDir] = useState("");
  const [activePreviewId, setActivePreviewId] = useState("");
  const [isProcessing, setIsProcessing] = useState(false);
  const [isDragging, setIsDragging] = useState(false);
  const [message, setMessage] = useState("");
  const [convertFormat, setConvertFormat] = useState<AudioOutputFormat>("mp3");
  const [clipFormat, setClipFormat] = useState<AudioOutputFormat>("wav");
  const [mergeFormat, setMergeFormat] = useState<AudioOutputFormat>("wav");
  const [mergeMode, setMergeMode] = useState<AudioMergeMode>("reencode");
  const [clipMode, setClipMode] = useState<ClipMode>("multi");
  const [clipExportMode, setClipExportMode] = useState<ClipExportMode>("separate");
  const [splitUnit, setSplitUnit] = useState<SplitUnit>("seconds");
  const [splitValue, setSplitValue] = useState(60);
  const [clipSegments, setClipSegments] = useState<ClipSegment[]>(() => [createClipSegment()]);
  const [activeClipSegmentId, setActiveClipSegmentId] = useState(() => clipSegments[0]?.id ?? "");
  const [frameRate, setFrameRate] = useState({ fps: 25, source: "fallback", message: "尚未检测，默认 25fps。" });
  const [clipPreviewSrc, setClipPreviewSrc] = useState("");
  const [waveformSrc, setWaveformSrc] = useState("");
  const [clipDuration, setClipDuration] = useState(0);
  const [clipCurrentTime, setClipCurrentTime] = useState(0);
  const [timelineZoom, setTimelineZoom] = useState(1);
  const [timelineWindowStart, setTimelineWindowStart] = useState(0);
  const [clipPreviewMessage, setClipPreviewMessage] = useState("");
  const clipMediaRef = useRef<HTMLMediaElement | null>(null);
  const timelineViewportRef = useRef<HTMLDivElement | null>(null);
  const timelineRef = useRef<HTMLDivElement | null>(null);
  const selectionPlaybackEndRef = useRef<number | null>(null);
  const previewAudioRefs = useRef<Record<string, HTMLAudioElement | null>>({});
  const preparingPreviewIds = useRef<Set<string>>(new Set());
  const selectedPreset = presets.find((preset) => preset.id === selectedPresetId) ?? presets[2];
  const activeClipSegment = clipSegments.find((segment) => segment.id === activeClipSegmentId) ?? clipSegments[0];
  const clipSourcePath = queue[0]?.filePath ?? "";
  const timelineDuration = Math.max(clipDuration, activeClipSegment?.endSeconds ?? 10, 1);
  const timelineMaxZoom = Math.max(1, Math.min(120, Math.ceil(timelineDuration / MIN_TIMELINE_WINDOW_SECONDS)));
  const activeTimelineZoom = Math.min(Math.max(1, timelineZoom), timelineMaxZoom);
  const timelineWindowDuration = Math.max(MIN_TIMELINE_WINDOW_SECONDS, Math.min(timelineDuration, timelineDuration / activeTimelineZoom));
  const safeTimelineWindowStart = Math.min(Math.max(0, timelineWindowStart), Math.max(0, timelineDuration - timelineWindowDuration));
  const timelineWindowEnd = Math.min(timelineDuration, safeTimelineWindowStart + timelineWindowDuration);
  const visibleClipSegments = clipSegments.map((segment, index) => ({ segment, index }));

  const addFilesToQueue = useCallback((paths: string[]) => {
    if (isProcessing) {
      setMessage("正在处理时不能继续加入文件。");
      return;
    }
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
  }, [isProcessing]);

  const addPathsToQueue = useCallback(
    async (paths: string[]) => {
      const collected: string[] = [];
      for (const path of paths.map((item) => item.trim()).filter(Boolean)) {
        if (isLikelySupportedMediaPath(path)) {
          collected.push(path);
          continue;
        }
        try {
          const folderFiles = await listAudioFilesInDirectory(path);
          collected.push(...folderFiles);
        } catch {
          collected.push(path);
        }
      }
      addFilesToQueue(collected);
    },
    [addFilesToQueue],
  );

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

  function updateClipSegment(id: string, patch: Partial<ClipSegment>) {
    setClipSegments((current) =>
      current.map((segment) => {
        if (segment.id !== id) {
          return segment;
        }
        const next = { ...segment, ...patch };
        return { ...next, endSeconds: Math.max(next.startSeconds + 0.001, next.endSeconds) };
      }),
    );
  }

  function setClipSegmentBoundary(id: string, boundary: "start" | "end", seconds: number) {
    const nextSeconds = clampSeconds(seconds, timelineDuration);
    setClipSegments((current) =>
      current.map((segment) => {
        if (segment.id !== id) {
          return segment;
        }

        const duration = Math.max(0.001, segment.endSeconds - segment.startSeconds);
        if (boundary === "start") {
          if (nextSeconds < segment.endSeconds) {
            return { ...segment, startSeconds: nextSeconds };
          }
          const endSeconds = Math.min(timelineDuration, nextSeconds + duration);
          return {
            ...segment,
            startSeconds: Math.max(0, Math.min(nextSeconds, endSeconds - 0.001)),
            endSeconds: Math.max(nextSeconds + 0.001, endSeconds),
          };
        }

        if (nextSeconds > segment.startSeconds) {
          return { ...segment, endSeconds: nextSeconds };
        }
        const startSeconds = Math.max(0, nextSeconds - duration);
        return {
          ...segment,
          startSeconds: Math.min(startSeconds, Math.max(0, nextSeconds - 0.001)),
          endSeconds: Math.max(nextSeconds, startSeconds + 0.001),
        };
      }),
    );
  }

  function setActiveClipBoundary(boundary: "start" | "end", seconds: number) {
    if (activeClipSegment) {
      setClipSegmentBoundary(activeClipSegment.id, boundary, seconds);
    }
  }

  function clipSegmentStyle(segment: ClipSegment, index: number): CSSProperties {
    const color = clipSegmentColors[index % clipSegmentColors.length];
    const start = clampSeconds(segment.startSeconds, timelineDuration);
    const end = clampSeconds(segment.endSeconds, timelineDuration);
    return {
      left: `${(start / timelineDuration) * 100}%`,
      width: `${Math.max(0.2, ((end - start) / timelineDuration) * 100)}%`,
      "--clip-segment-color": color,
    } as CSSProperties;
  }

  function timelinePositionPercent(seconds: number) {
    return (clampSeconds(seconds, timelineDuration) / timelineDuration) * 100;
  }

  function waveformImageStyle(): CSSProperties {
    return {
      "--timeline-width": `${activeTimelineZoom * 100}%`,
    } as CSSProperties;
  }

  function secondsFromTimelineEvent(event: ReactPointerEvent<HTMLElement> | ReactMouseEvent<HTMLElement>) {
    return secondsFromTimelineClientX(event.clientX);
  }

  function secondsFromTimelineClientX(clientX: number) {
    const node = timelineRef.current;
    if (!node) {
      return 0;
    }
    const rect = node.getBoundingClientRect();
    const ratio = rect.width > 0 ? (clientX - rect.left) / rect.width : 0;
    return clampSeconds(ratio * timelineDuration, timelineDuration);
  }

  function seekClipPreview(seconds: number) {
    const nextSeconds = clampSeconds(seconds, timelineDuration);
    setClipCurrentTime(nextSeconds);
    if (clipMediaRef.current) {
      clipMediaRef.current.currentTime = nextSeconds;
    }
  }

  function handleTimelinePointerDown(target: TimelineDragTarget, event: ReactPointerEvent<HTMLElement>) {
    if (target === "playhead" && clipMode !== "split" && event.ctrlKey) {
      handleCreateSelectionPointerDown(event);
      return;
    }
    if (isProcessing || !activeClipSegment) {
      return;
    }
    event.preventDefault();
    const apply = (clientX: number) => {
      const node = timelineRef.current;
      if (!node) {
        return;
      }
      const rect = node.getBoundingClientRect();
      const ratio = rect.width > 0 ? (clientX - rect.left) / rect.width : 0;
      const seconds = clampSeconds(ratio * timelineDuration, timelineDuration);
      if (target === "playhead") {
        seekClipPreview(seconds);
      } else {
        setActiveClipBoundary(target, seconds);
      }
    };

    apply(event.clientX);
    const onPointerMove = (moveEvent: PointerEvent) => apply(moveEvent.clientX);
    const onPointerUp = () => {
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
    };
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp, { once: true });
  }

  function handleCreateSelectionPointerDown(event: ReactPointerEvent<HTMLElement>) {
    if (isProcessing) {
      return;
    }
    event.preventDefault();
    const anchorSeconds = secondsFromTimelineClientX(event.clientX);
    const initialStart = Math.min(anchorSeconds, Math.max(0, timelineDuration - 0.001));
    const initialEnd = Math.min(timelineDuration, initialStart + 0.001);
    const nextSegment = createClipSegment(initialStart, initialEnd);
    setClipSegments((current) => [...current, nextSegment]);
    setActiveClipSegmentId(nextSegment.id);
    seekClipPreview(anchorSeconds);

    const apply = (clientX: number) => {
      const nextSeconds = secondsFromTimelineClientX(clientX);
      const rawStart = Math.min(anchorSeconds, nextSeconds);
      const rawEnd = Math.max(anchorSeconds, nextSeconds);
      const startSeconds = Math.min(rawStart, Math.max(0, timelineDuration - 0.001));
      const endSeconds = Math.min(timelineDuration, Math.max(rawEnd, startSeconds + 0.001));
      setClipSegments((current) =>
        current.map((segment) =>
          segment.id === nextSegment.id
            ? {
                ...segment,
                startSeconds,
                endSeconds: Math.min(timelineDuration, endSeconds),
              }
            : segment,
        ),
      );
    };

    const onPointerMove = (moveEvent: PointerEvent) => apply(moveEvent.clientX);
    const onPointerUp = (upEvent: PointerEvent) => {
      apply(upEvent.clientX);
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
    };
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp, { once: true });
  }

  function markBoundaryFromPlayhead(boundary: "start" | "end") {
    setActiveClipBoundary(boundary, clipCurrentTime);
  }

  function playActiveSelection() {
    if (!clipMediaRef.current || !activeClipSegment) {
      return;
    }
    clipMediaRef.current.currentTime = activeClipSegment.startSeconds;
    selectionPlaybackEndRef.current = activeClipSegment.endSeconds;
    setClipCurrentTime(activeClipSegment.startSeconds);
    void clipMediaRef.current.play().catch(() => {
      selectionPlaybackEndRef.current = null;
      setClipPreviewMessage("如果没有自动播放，可用播放器控件手动播放。");
    });
  }

  function handleClipMediaTimeUpdate(event: SyntheticEvent<HTMLMediaElement>) {
    const current = event.currentTarget.currentTime;
    setClipCurrentTime(current);
    const selectionEnd = selectionPlaybackEndRef.current;
    if (selectionEnd !== null && current >= selectionEnd) {
      selectionPlaybackEndRef.current = null;
      event.currentTarget.pause();
    }
  }

  function clearSelectionPlaybackLimit() {
    selectionPlaybackEndRef.current = null;
  }

  function addClipSegment() {
    setClipSegments((current) => {
      const previous = current[current.length - 1];
      const start = previous ? previous.endSeconds : 0;
      const nextSegment = createClipSegment(start, start + 10);
      setActiveClipSegmentId(nextSegment.id);
      return [...current, nextSegment];
    });
  }

  function addClipSegmentAtPlayhead() {
    const start = clampSeconds(clipCurrentTime, timelineDuration);
    const defaultLength = Math.min(10, Math.max(1, timelineDuration / 20));
    const end = Math.min(timelineDuration, start + defaultLength);
    const safeStart = end > start ? start : Math.max(0, timelineDuration - defaultLength);
    const nextSegment = createClipSegment(safeStart, Math.max(safeStart + 0.001, end));
    setClipSegments((current) => [...current, nextSegment]);
    setActiveClipSegmentId(nextSegment.id);
  }

  function setTimelineZoomAroundAnchor(nextZoom: number, anchorSeconds: number, anchorRatio: number) {
    const clampedZoom = Math.min(Math.max(1, nextZoom), timelineMaxZoom);
    const nextWindowDuration = Math.max(
      MIN_TIMELINE_WINDOW_SECONDS,
      Math.min(timelineDuration, timelineDuration / clampedZoom),
    );
    const anchor = clampSeconds(anchorSeconds, timelineDuration);
    const ratio = Math.min(1, Math.max(0, Number.isFinite(anchorRatio) ? anchorRatio : 0.5));
    const maxStart = Math.max(0, timelineDuration - nextWindowDuration);
    const nextStart = Math.min(maxStart, Math.max(0, anchor - nextWindowDuration * ratio));
    setTimelineZoom(clampedZoom);
    setTimelineWindowStart(nextStart);
    requestAnimationFrame(() => scrollTimelineViewportToStart(nextStart, nextWindowDuration));
  }

  function setTimelineZoomAroundPlayhead(nextZoom: number) {
    const anchor = clampSeconds(clipMediaRef.current?.currentTime ?? clipCurrentTime, timelineDuration);
    const currentRatio =
      timelineWindowDuration > 0 ? Math.min(1, Math.max(0, (anchor - safeTimelineWindowStart) / timelineWindowDuration)) : 0.5;
    setTimelineZoomAroundAnchor(nextZoom, anchor, currentRatio);
  }

  function showFullTimeline() {
    setTimelineZoom(1);
    setTimelineWindowStart(0);
    requestAnimationFrame(() => scrollTimelineViewportToStart(0, timelineDuration));
  }

  function centerTimelineOnPlayhead() {
    const anchor = clampSeconds(clipMediaRef.current?.currentTime ?? clipCurrentTime, timelineDuration);
    scrollTimelineToStart(Math.max(0, anchor - timelineWindowDuration / 2));
  }

  function scrollTimelineViewportToStart(startSeconds: number, windowDuration = timelineWindowDuration) {
    const viewport = timelineViewportRef.current;
    if (!viewport) {
      return;
    }
    const maxScroll = Math.max(0, viewport.scrollWidth - viewport.clientWidth);
    const maxStart = Math.max(0, timelineDuration - windowDuration);
    viewport.scrollLeft = maxStart > 0 ? (Math.min(Math.max(0, startSeconds), maxStart) / maxStart) * maxScroll : 0;
  }

  function scrollTimelineToStart(startSeconds: number) {
    const nextStart = Math.min(Math.max(0, startSeconds), Math.max(0, timelineDuration - timelineWindowDuration));
    setTimelineWindowStart(nextStart);
    scrollTimelineViewportToStart(nextStart);
  }

  function handleTimelineViewportScroll() {
    const viewport = timelineViewportRef.current;
    if (!viewport) {
      return;
    }
    const maxScroll = Math.max(0, viewport.scrollWidth - viewport.clientWidth);
    const maxStart = Math.max(0, timelineDuration - timelineWindowDuration);
    const nextStart = maxScroll > 0 ? (viewport.scrollLeft / maxScroll) * maxStart : 0;
    setTimelineWindowStart(nextStart);
  }

  function removeClipSegment(id: string) {
    setClipSegments((current) => {
      if (current.length <= 1) {
        return current;
      }
      const next = current.filter((segment) => segment.id !== id);
      if (activeClipSegmentId === id) {
        setActiveClipSegmentId(next[0]?.id ?? "");
      }
      return next;
    });
  }

  function moveClipSegment(id: string, direction: -1 | 1) {
    setClipSegments((current) => {
      const index = current.findIndex((segment) => segment.id === id);
      const nextIndex = index + direction;
      if (index < 0 || nextIndex < 0 || nextIndex >= current.length) {
        return current;
      }
      const next = [...current];
      [next[index], next[nextIndex]] = [next[nextIndex], next[index]];
      return next;
    });
  }

  function moveQueueItem(id: string, direction: -1 | 1) {
    setQueue((current) => {
      const index = current.findIndex((item) => item.id === id);
      const nextIndex = index + direction;
      if (index < 0 || nextIndex < 0 || nextIndex >= current.length) {
        return current;
      }
      const next = [...current];
      [next[index], next[nextIndex]] = [next[nextIndex], next[index]];
      return next;
    });
  }

  async function handleOpenOutputFolder(item: ProcessingQueueItem) {
    const path = item.outputPath || item.outputPaths?.find((outputPath) => !outputPath.startsWith("..."));
    if (!path) {
      return;
    }
    try {
      const openedDir = await openOutputFolder(path);
      setMessage(`已打开输出目录：${openedDir}`);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "打开输出目录失败。");
    }
  }

  async function processEnhance(item: ProcessingQueueItem) {
    const result = await processAudioFile(item.filePath, selectedPreset.options, {
      destinationDir: outputDir || undefined,
    });
    return { outputPath: result.outputPath, outputPaths: [result.outputPath], message: result.message };
  }

  async function processConvert(item: ProcessingQueueItem) {
    const result = await convertAudioFile(item.filePath, convertFormat, {
      destinationDir: outputDir || undefined,
    });
    return { outputPath: result.outputPath, outputPaths: [result.outputPath], message: result.message };
  }

  async function processClip(item: ProcessingQueueItem) {
    if (clipMode === "split") {
      const itemFps = splitUnit === "frames" ? await probeMediaFrameRate(item.filePath) : frameRate;
      const seconds = splitUnit === "frames" ? splitValue / Math.max(1, itemFps.fps) : splitValue;
      const paths = await splitAudioFile(item.filePath, seconds, clipFormat, {
        destinationDir: outputDir || undefined,
      });
      return { outputPath: paths[0], outputPaths: paths, message: `已切分 ${paths.length} 段。` };
    }

    const segments = clipSegments.map((segment, index) => ({
      startSeconds: segment.startSeconds,
      endSeconds: segment.endSeconds,
      suggestedName: clipExportMode === "separate" ? suggestedClipName(item.filePath, index + 1, clipFormat) : undefined,
    }));
    const mergeSegments = clipExportMode === "merged";
    const paths = await clipAudioSegments(item.filePath, segments, clipFormat, {
      destinationDir: outputDir || undefined,
      mergeSegments,
      suggestedName: mergeSegments ? `${stemFromPath(item.filePath)}-clips-merged.${clipFormat}` : undefined,
    });
    return {
      outputPath: paths[0],
      outputPaths: paths,
      message: mergeSegments ? "多段剪辑已合并导出。" : `已导出 ${paths.length} 段音频。`,
    };
  }

  async function handleProcess() {
    if (queue.length === 0) {
      setMessage("请先选择、拖放文件，或导入文件夹。");
      return;
    }
    if (activeTool === "merge" && queue.length < 2) {
      setMessage("音频合并至少需要 2 个文件。");
      return;
    }

    setIsProcessing(true);
    setMessage(activeTool === "merge" ? "正在合并音频..." : `正在处理 ${queue.length} 个文件...`);

    if (activeTool === "merge") {
      setQueue((current) => current.map((item) => ({ ...item, status: "running", message: "正在参与合并..." })));
      try {
        const outputPath = await mergeAudioFiles(
          queue.map((item) => item.filePath),
          mergeMode,
          mergeFormat,
          {
            destinationDir: outputDir || undefined,
            suggestedName: `merged-audio.${mergeFormat}`,
          },
        );
        setQueue((current) =>
          current.map((item, index) => ({
            ...item,
            status: "done",
            message: index === 0 ? `合并完成：${outputPath}` : "已参与合并",
            outputPath: index === 0 ? outputPath : item.outputPath,
            outputPaths: index === 0 ? [outputPath] : item.outputPaths,
          })),
        );
      } catch (error) {
        setQueue((current) =>
          current.map((item) => ({
            ...item,
            status: "failed",
            message: error instanceof Error ? error.message : "音频合并失败。",
          })),
        );
      } finally {
        setIsProcessing(false);
      }
      return;
    }

    for (const item of queue) {
      updateQueueItem(item.id, { status: "running", message: `正在处理 ${fileNameFromPath(item.filePath)}...` });
      try {
        const result =
          activeTool === "enhance" ? await processEnhance(item) : activeTool === "convert" ? await processConvert(item) : await processClip(item);
        updateQueueItem(item.id, {
          status: "done",
          message: result.message,
          outputPath: result.outputPath,
          outputPaths: displayOutputPaths(result.outputPaths),
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
    const resetSegment = createClipSegment();
    setQueue([]);
    setClipSegments([resetSegment]);
    setActiveClipSegmentId(resetSegment.id);
    setClipPreviewSrc("");
    setWaveformSrc("");
    setClipDuration(0);
    setClipCurrentTime(0);
    setTimelineZoom(1);
    setTimelineWindowStart(0);
    setClipPreviewMessage("");
    previewAudioRefs.current = {};
    preparingPreviewIds.current.clear();
    clipMediaRef.current = null;
    setActivePreviewId("");
    setMessage("处理队列和历史结果已清空。");
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
    if (!clipSegments.some((segment) => segment.id === activeClipSegmentId)) {
      setActiveClipSegmentId(clipSegments[0]?.id ?? "");
    }
  }, [activeClipSegmentId, clipSegments]);

  useEffect(() => {
    setTimelineZoom((current) => Math.min(Math.max(1, current), timelineMaxZoom));
    setTimelineWindowStart((current) => Math.min(Math.max(0, current), Math.max(0, timelineDuration - timelineWindowDuration)));
  }, [timelineDuration, timelineMaxZoom, timelineWindowDuration]);

  useEffect(() => {
    const viewport = timelineViewportRef.current;
    if (activeTool !== "clip" || !viewport) {
      return;
    }

    const handleWheel = (event: WheelEvent) => {
      if (!event.altKey) {
        if (Math.abs(event.deltaY) > Math.abs(event.deltaX)) {
          const pageScroller = document.scrollingElement;
          if (pageScroller) {
            event.preventDefault();
            pageScroller.scrollBy({ top: event.deltaY, behavior: "auto" });
          }
        }
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      if (timelineMaxZoom <= 1) {
        return;
      }

      const rect = viewport.getBoundingClientRect();
      const anchorRatio = rect.width > 0 ? (event.clientX - rect.left) / rect.width : 0.5;
      const anchorSeconds = secondsFromTimelineClientX(event.clientX);
      const zoomFactor = event.deltaY < 0 ? 1.2 : 1 / 1.2;
      setTimelineZoomAroundAnchor(activeTimelineZoom * zoomFactor, anchorSeconds, anchorRatio);
    };

    viewport.addEventListener("wheel", handleWheel, { passive: false });
    return () => viewport.removeEventListener("wheel", handleWheel);
  }, [activeTool, activeTimelineZoom, timelineMaxZoom, timelineDuration, timelineWindowDuration, safeTimelineWindowStart]);

  useEffect(() => {
    if (activeTool !== "clip" || !queue[0]?.filePath) {
      return;
    }

    let cancelled = false;
    void probeMediaFrameRate(queue[0].filePath)
      .then((result) => {
        if (!cancelled) {
          setFrameRate(result);
        }
      })
      .catch((error) => {
        if (!cancelled) {
          setFrameRate({
            fps: 25,
            source: "fallback",
            message: error instanceof Error ? `${error.message}；已回退 25fps。` : "帧率检测失败，已回退 25fps。",
          });
        }
      });

    return () => {
      cancelled = true;
    };
  }, [activeTool, queue]);

  useEffect(() => {
    if (activeTool !== "clip" || !clipSourcePath) {
      setClipPreviewSrc("");
      setWaveformSrc("");
      setClipDuration(0);
      setClipCurrentTime(0);
      setTimelineZoom(1);
      setTimelineWindowStart(0);
      setClipPreviewMessage("");
      return;
    }

    let cancelled = false;
    setClipPreviewMessage("正在准备预览和波形...");
    setWaveformSrc("");
    setTimelineZoom(1);
    setTimelineWindowStart(0);
    void Promise.all([
      prepareAudioPreview(clipSourcePath),
      prepareAudioWaveform(clipSourcePath, { width: 1800, height: 220 }),
    ])
      .then(([previewPath, waveform]) => {
        if (cancelled) {
          return;
        }
        setClipPreviewSrc(audioPathToSrc(previewPath));
        setWaveformSrc(audioPathToSrc(waveform.waveformPath));
        if (waveform.durationSeconds > 0) {
          setClipDuration(waveform.durationSeconds);
          setClipSegments((current) =>
            current.map((segment) => ({
              ...segment,
              startSeconds: clampSeconds(segment.startSeconds, waveform.durationSeconds),
              endSeconds: Math.min(Math.max(segment.endSeconds, segment.startSeconds + 0.001), waveform.durationSeconds),
            })),
          );
        }
        setClipPreviewMessage(
          waveform.durationSeconds > 0
            ? "Ctrl 拖动波形可直接拉出新选区；Alt + 滚轮可缩放时间线；拖动入点/出点可微调当前选区。"
            : "波形已准备，时长会在媒体加载后校准。",
        );
      })
      .catch((error) => {
        if (!cancelled) {
          setClipPreviewMessage(error instanceof Error ? `预览准备失败：${error.message}` : "预览准备失败。");
        }
      });

    return () => {
      cancelled = true;
    };
  }, [activeTool, clipSourcePath]);

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
            if (isProcessing) {
              setMessage("正在处理时不能继续拖放文件。");
              return;
            }
            void addPathsToQueue(event.payload.paths);
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
  }, [addPathsToQueue, isProcessing]);

  return (
    <div className="page-stack audio-processing-page">
      <section className={`panel audio-drop-panel ${isDragging ? "audio-drop-panel--active" : ""}`}>
        <div className="panel-heading">
          <div>
            <p className="section-label">音频工具</p>
            <h2>批量处理音频和视频文件</h2>
            <p className="panel-hint">支持单文件、多文件、文件夹递归导入和拖放；默认输出到源文件目录。</p>
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
          <span>{outputDir ? `自定义输出目录：${outputDir}` : "默认输出到每个源文件所在目录"}</span>
        </div>
      </section>

      <section className="panel audio-tool-panel">
        <div className="audio-tool-tabs" role="tablist" aria-label="音频工具">
          {tools.map((tool) => (
            <button
              className={activeTool === tool.id ? "audio-tool-tab audio-tool-tab--active" : "audio-tool-tab"}
              key={tool.id}
              type="button"
              disabled={isProcessing}
              onClick={() => setActiveTool(tool.id)}
            >
              <strong>{tool.label}</strong>
              <span>{tool.description}</span>
            </button>
          ))}
        </div>

        {activeTool === "enhance" && (
          <>
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
          </>
        )}

        {activeTool === "convert" && (
          <div className="audio-settings-grid">
            <label>
              <span>输出格式</span>
              <select value={convertFormat} disabled={isProcessing} onChange={(event) => setConvertFormat(event.target.value as AudioOutputFormat)}>
                {outputFormats.map((format) => (
                  <option key={format.id} value={format.id}>
                    {format.label}
                  </option>
                ))}
              </select>
            </label>
          </div>
        )}

        {activeTool === "clip" && (
          <div className="audio-clip-workbench">
            <div className="audio-settings-grid">
              <label>
                <span>剪辑模式</span>
                <select value={clipMode} disabled={isProcessing} onChange={(event) => setClipMode(event.target.value as ClipMode)}>
                  <option value="multi">片段截取</option>
                  <option value="split">按长度批量切分</option>
                </select>
              </label>
              <label>
                <span>输出格式</span>
                <select value={clipFormat} disabled={isProcessing} onChange={(event) => setClipFormat(event.target.value as AudioOutputFormat)}>
                  {outputFormats.map((format) => (
                    <option key={format.id} value={format.id}>
                      {format.label}
                    </option>
                  ))}
                </select>
              </label>
              {clipMode !== "split" && (
                <label>
                  <span>片段导出</span>
                  <select value={clipExportMode} disabled={isProcessing} onChange={(event) => setClipExportMode(event.target.value as ClipExportMode)}>
                    <option value="separate">每段独立文件</option>
                    <option value="merged">合并为单个文件</option>
                  </select>
                </label>
              )}
            </div>
            <p className="panel-hint">
              帧率：{frameRate.fps.toFixed(3)}fps。{frameRate.message}
            </p>
            <div className="clip-preview-editor">
              <div className="clip-media-preview">
                {clipPreviewSrc ? (
                  isVideoPath(clipSourcePath) ? (
                    <video
                      controls
                      preload="metadata"
                      ref={(node) => {
                        clipMediaRef.current = node;
                      }}
                      src={clipPreviewSrc}
                      onLoadedMetadata={(event) => {
                        const duration = event.currentTarget.duration;
                        if (Number.isFinite(duration) && duration > 0) {
                          setClipDuration(duration);
                        }
                      }}
                      onPause={clearSelectionPlaybackLimit}
                      onTimeUpdate={handleClipMediaTimeUpdate}
                    />
                  ) : (
                    <audio
                      controls
                      preload="metadata"
                      ref={(node) => {
                        clipMediaRef.current = node;
                      }}
                      src={clipPreviewSrc}
                      onLoadedMetadata={(event) => {
                        const duration = event.currentTarget.duration;
                        if (Number.isFinite(duration) && duration > 0) {
                          setClipDuration(duration);
                        }
                      }}
                      onPause={clearSelectionPlaybackLimit}
                      onTimeUpdate={handleClipMediaTimeUpdate}
                    />
                  )
                ) : (
                  <div className="clip-preview-empty">选择文件后显示预览</div>
                )}
                <div className="clip-preview-actions">
                  <button className="secondary-button" type="button" disabled={!clipPreviewSrc || clipMode === "split"} onClick={() => markBoundaryFromPlayhead("start")}>
                    设为入点
                  </button>
                  <button className="secondary-button" type="button" disabled={!clipPreviewSrc || clipMode === "split"} onClick={() => markBoundaryFromPlayhead("end")}>
                    设为出点
                  </button>
                  {clipMode !== "split" && (
                    <button className="secondary-button" type="button" disabled={!clipPreviewSrc || isProcessing} onClick={addClipSegmentAtPlayhead}>
                      <Scissors size={16} />
                      新增选区
                    </button>
                  )}
                  <button className="secondary-button" type="button" disabled={!clipPreviewSrc || clipMode === "split"} onClick={playActiveSelection}>
                    <Play size={16} />
                    播放选区
                  </button>
                </div>
              </div>
              <div className="clip-timeline-wrap">
                <div className="clip-timeline-controls">
                  <div className="clip-timeline-buttons">
                    <button
                      className="icon-button"
                      title="缩小时间线"
                      type="button"
                      disabled={activeTimelineZoom <= 1}
                      onClick={() => setTimelineZoomAroundPlayhead(activeTimelineZoom / 1.8)}
                    >
                      <ZoomOut size={16} />
                    </button>
                    <button
                      className="icon-button"
                      title="放大时间线"
                      type="button"
                      disabled={activeTimelineZoom >= timelineMaxZoom}
                      onClick={() => setTimelineZoomAroundPlayhead(activeTimelineZoom * 1.8)}
                    >
                      <ZoomIn size={16} />
                    </button>
                    <button className="icon-button" title="显示完整时间线" type="button" onClick={showFullTimeline}>
                      <Maximize2 size={16} />
                    </button>
                  </div>
                  <label className="clip-zoom-control">
                    <span>缩放 {activeTimelineZoom.toFixed(1)}x</span>
                    <input
                      aria-label="时间线缩放"
                      max={timelineMaxZoom}
                      min="1"
                      step="0.1"
                      type="range"
                      value={activeTimelineZoom}
                      onChange={(event) => setTimelineZoomAroundPlayhead(Number(event.target.value))}
                    />
                  </label>
                  <button className="secondary-button" type="button" disabled={!clipPreviewSrc} onClick={centerTimelineOnPlayhead}>
                    定位播放头
                  </button>
                </div>
                <div className="clip-waveform-viewport" ref={timelineViewportRef} onScroll={handleTimelineViewportScroll}>
                  <div
                    className="clip-waveform"
                    style={waveformImageStyle()}
                    ref={timelineRef}
                    onClick={(event) => seekClipPreview(secondsFromTimelineEvent(event))}
                    onPointerDown={(event) => handleTimelinePointerDown("playhead", event)}
                  >
                    {waveformSrc ? <img alt="音频波形" src={waveformSrc} /> : <div className="clip-waveform-loading">{clipPreviewMessage || "波形准备中..."}</div>}
                    {clipMode !== "split" &&
                      visibleClipSegments.map(({ segment, index }) => (
                        <button
                          className={activeClipSegment?.id === segment.id ? "clip-selection clip-selection--active" : "clip-selection"}
                          key={segment.id}
                          style={clipSegmentStyle(segment, index)}
                          type="button"
                          onClick={(event) => {
                            event.stopPropagation();
                            setActiveClipSegmentId(segment.id);
                            seekClipPreview(segment.startSeconds);
                          }}
                          onPointerDown={(event) => {
                            event.stopPropagation();
                            setActiveClipSegmentId(segment.id);
                          }}
                        >
                          <span>{index + 1}</span>
                        </button>
                      ))}
                    {clipMode !== "split" && activeClipSegment && (
                      <>
                        <button
                          aria-label="拖动入点"
                          className="clip-handle clip-handle--start"
                          style={{ left: `${timelinePositionPercent(activeClipSegment.startSeconds)}%` }}
                          type="button"
                          onClick={(event) => event.stopPropagation()}
                          onPointerDown={(event) => {
                            event.stopPropagation();
                            handleTimelinePointerDown("start", event);
                          }}
                        >
                          入
                        </button>
                        <button
                          aria-label="拖动出点"
                          className="clip-handle clip-handle--end"
                          style={{ left: `${timelinePositionPercent(activeClipSegment.endSeconds)}%` }}
                          type="button"
                          onClick={(event) => event.stopPropagation()}
                          onPointerDown={(event) => {
                            event.stopPropagation();
                            handleTimelinePointerDown("end", event);
                          }}
                        >
                          出
                        </button>
                      </>
                    )}
                    <span className="clip-playhead" style={{ left: `${timelinePositionPercent(clipCurrentTime)}%` }} />
                  </div>
                </div>
                <label className="clip-pan-control">
                  <span>{formatTimecode(safeTimelineWindowStart, frameRate.fps)}</span>
                  <input
                    aria-label="时间线位置"
                    max={Math.max(0, timelineDuration - timelineWindowDuration)}
                    min="0"
                    step="0.001"
                    type="range"
                    value={safeTimelineWindowStart}
                    onChange={(event) => scrollTimelineToStart(Number(event.target.value))}
                  />
                  <span>{formatTimecode(timelineWindowEnd, frameRate.fps)}</span>
                </label>
                <div className="clip-timeline-meta">
                  <span>{formatTimecode(safeTimelineWindowStart, frameRate.fps)}</span>
                  <strong>
                    {activeClipSegment && clipMode !== "split"
                      ? `${formatSeconds(activeClipSegment.startSeconds)}s - ${formatSeconds(activeClipSegment.endSeconds)}s`
                      : `总时长 ${formatSeconds(timelineDuration)}s`}
                  </strong>
                  <span>{formatTimecode(timelineWindowEnd, frameRate.fps)}</span>
                </div>
                {clipPreviewMessage && <p className="panel-hint">{clipPreviewMessage}</p>}
              </div>
            </div>
            {clipMode === "split" ? (
              <div className="audio-settings-grid">
                <label>
                  <span>切分单位</span>
                  <select value={splitUnit} disabled={isProcessing} onChange={(event) => setSplitUnit(event.target.value as SplitUnit)}>
                    <option value="seconds">秒</option>
                    <option value="frames">帧</option>
                  </select>
                </label>
                <label>
                  <span>{splitUnit === "seconds" ? "每段秒数" : "每段帧数"}</span>
                  <input min="1" step="1" type="number" value={splitValue} disabled={isProcessing} onChange={(event) => setSplitValue(Number(event.target.value))} />
                </label>
                <div className="button-group audio-inline-actions">
                  <button className="secondary-button" type="button" disabled={isProcessing} onClick={() => setSplitValue(30)}>
                    30 秒
                  </button>
                  <button className="secondary-button" type="button" disabled={isProcessing} onClick={() => setSplitValue(60)}>
                    60 秒
                  </button>
                  <button className="secondary-button" type="button" disabled={isProcessing} onClick={() => setSplitValue(300)}>
                    5 分钟
                  </button>
                </div>
              </div>
            ) : (
              <div className="audio-segment-list">
                {clipSegments.map((segment, index) => (
                  <div
                    className={activeClipSegment?.id === segment.id ? "audio-segment-row audio-segment-row--active" : "audio-segment-row"}
                    key={segment.id}
                    onClick={() => setActiveClipSegmentId(segment.id)}
                  >
                    <strong>片段 {index + 1}</strong>
                    <label>
                      <span>开始时间</span>
                      <input
                        aria-label={`片段 ${index + 1} 开始时间`}
                        inputMode="decimal"
                        placeholder="00:00:00.000"
                        value={formatClockTime(segment.startSeconds)}
                        disabled={isProcessing}
                        onChange={(event) => {
                          const seconds = parseClockTime(event.target.value, frameRate.fps);
                          if (seconds !== null) {
                            setClipSegmentBoundary(segment.id, "start", seconds);
                          }
                        }}
                      />
                    </label>
                    <label>
                      <span>结束时间</span>
                      <input
                        aria-label={`片段 ${index + 1} 结束时间`}
                        inputMode="decimal"
                        placeholder="00:00:00.000"
                        value={formatClockTime(segment.endSeconds)}
                        disabled={isProcessing}
                        onChange={(event) => {
                          const seconds = parseClockTime(event.target.value, frameRate.fps);
                          if (seconds !== null) {
                            setClipSegmentBoundary(segment.id, "end", seconds);
                          }
                        }}
                      />
                    </label>
                    <label>
                      <span>开始帧码</span>
                      <input
                        value={formatTimecode(segment.startSeconds, frameRate.fps)}
                        disabled={isProcessing}
                        onChange={(event) => {
                          const seconds = parseTimecode(event.target.value, frameRate.fps);
                          if (seconds !== null) {
                            updateClipSegment(segment.id, { startSeconds: seconds });
                          }
                        }}
                      />
                    </label>
                    <label>
                      <span>结束帧码</span>
                      <input
                        value={formatTimecode(segment.endSeconds, frameRate.fps)}
                        disabled={isProcessing}
                        onChange={(event) => {
                          const seconds = parseTimecode(event.target.value, frameRate.fps);
                          if (seconds !== null) {
                            updateClipSegment(segment.id, { endSeconds: seconds });
                          }
                        }}
                      />
                    </label>
                    <div className="audio-segment-actions">
                      <button
                        className="icon-button"
                        title="片段上移"
                        type="button"
                        disabled={index === 0 || isProcessing}
                        onClick={() => moveClipSegment(segment.id, -1)}
                      >
                        <ArrowUp size={16} />
                      </button>
                      <button
                        className="icon-button"
                        title="片段下移"
                        type="button"
                        disabled={index === clipSegments.length - 1 || isProcessing}
                        onClick={() => moveClipSegment(segment.id, 1)}
                      >
                        <ArrowDown size={16} />
                      </button>
                      <button className="icon-button" title="删除片段" type="button" disabled={isProcessing} onClick={() => removeClipSegment(segment.id)}>
                        <Trash2 size={16} />
                      </button>
                    </div>
                  </div>
                ))}
                <button className="secondary-button" type="button" disabled={isProcessing} onClick={addClipSegment}>
                  <Scissors size={16} />
                  添加片段
                </button>
              </div>
            )}
          </div>
        )}

        {activeTool === "merge" && (
          <div className="audio-settings-grid">
            <label>
              <span>合并方式</span>
              <select value={mergeMode} disabled={isProcessing} onChange={(event) => setMergeMode(event.target.value as AudioMergeMode)}>
                <option value="reencode">重新编码，兼容不同格式</option>
                <option value="copy">无需重新编码，要求格式兼容</option>
              </select>
            </label>
            <label>
              <span>输出格式</span>
              <select value={mergeFormat} disabled={isProcessing} onChange={(event) => setMergeFormat(event.target.value as AudioOutputFormat)}>
                {outputFormats.map((format) => (
                  <option key={format.id} value={format.id}>
                    {format.label}
                  </option>
                ))}
              </select>
            </label>
          </div>
        )}

        <div className="audio-action-row">
          <button
            className="primary-button audio-process-button"
            type="button"
            disabled={isProcessing || queue.length === 0}
            onClick={() => void handleProcess()}
          >
            {activeTool === "clip" ? <Scissors size={16} /> : activeTool === "merge" ? <Merge size={16} /> : <SlidersHorizontal size={16} />}
            {isProcessing
              ? "处理中..."
              : activeTool === "enhance"
                ? `批量运行：${selectedPreset.title}`
                : activeTool === "convert"
                  ? `批量转换为 ${convertFormat.toUpperCase()}`
                  : activeTool === "clip"
                    ? "开始剪辑"
                    : "开始合并"}
          </button>
          <button className="secondary-button" type="button" disabled={isProcessing || queue.length === 0} onClick={clearQueue}>
            <Trash2 size={16} />
            清空队列/历史
          </button>
        </div>
        {message && <p className="model-message">{message}</p>}
      </section>

      <section className="panel audio-queue-panel">
        <div className="panel-heading panel-heading--compact">
          <div>
            <p className="section-label">处理队列</p>
            <p className="panel-hint">已加入 {queue.length} 个文件。{activeTool === "merge" ? "合并顺序按列表从上到下执行。" : ""}</p>
          </div>
        </div>
        {queue.length === 0 ? (
          <div className="audio-queue-empty">
            <FileAudio size={20} />
            <span>拖放文件/文件夹、选择文件，或选择文件夹加入队列。</span>
          </div>
        ) : (
          <div className="audio-processing-queue">
            {queue.map((item, index) => (
              <div
                className={`audio-queue-row audio-queue-row--${item.status} ${
                  activePreviewId === item.id ? "audio-queue-row--previewing" : ""
                }`}
                key={item.id}
              >
                <FileAudio size={18} />
                <div>
                  <strong>{item.filePath}</strong>
                  <span>
                    {item.outputPaths && item.outputPaths.length > 1
                      ? `${item.message}：${item.outputPaths.join("；")}`
                      : item.outputPath
                        ? `${item.message}：${item.outputPath}`
                        : item.message}
                  </span>
                </div>
                <div className="audio-row-actions">
                  {activeTool === "merge" && (
                    <>
                      <button className="icon-button" title="上移" type="button" disabled={index === 0 || isProcessing} onClick={() => moveQueueItem(item.id, -1)}>
                        <ArrowUp size={16} />
                      </button>
                      <button
                        className="icon-button"
                        title="下移"
                        type="button"
                        disabled={index === queue.length - 1 || isProcessing}
                        onClick={() => moveQueueItem(item.id, 1)}
                      >
                        <ArrowDown size={16} />
                      </button>
                    </>
                  )}
                  <button className="secondary-button" type="button" disabled={!item.previewPath} onClick={() => playPreview(item.id)}>
                    <Play size={16} />
                    试听
                  </button>
                  <button className="secondary-button" type="button" disabled={!item.outputPath && !item.outputPaths?.length} onClick={() => void handleOpenOutputFolder(item)}>
                    <FolderOpen size={16} />
                    打开目录
                  </button>
                </div>
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
