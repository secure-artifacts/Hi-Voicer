import { Download, FileAudio, FolderOpen, Merge, Scissors, Search, StepForward, Subtitles } from "lucide-react";
import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import { exportAudioSegment, prepareAudioPreview, saveTextFile, selectDirectory } from "../lib/api";
import type { HotwordRule, SubtitleSegment, TermCategory, TimelineKind } from "../types";

interface SubtitleProject {
  fileName: string;
  sourceAudioPath: string;
  text: string;
  segments: SubtitleSegment[];
  timelineKind?: TimelineKind;
}

interface SubtitleEditorPageProps {
  project: SubtitleProject | null;
  onProjectChange: (project: SubtitleProject | null) => void;
  termCategories: TermCategory[];
  onAddTermRule: (rule: HotwordRule) => void;
}

function formatSrtTimestamp(seconds: number) {
  const millis = Math.max(0, Math.round(seconds * 1000));
  const hours = Math.floor(millis / 3_600_000);
  const minutes = Math.floor((millis % 3_600_000) / 60_000);
  const secs = Math.floor((millis % 60_000) / 1000);
  const ms = millis % 1000;
  return `${hours.toString().padStart(2, "0")}:${minutes.toString().padStart(2, "0")}:${secs
    .toString()
    .padStart(2, "0")},${ms.toString().padStart(3, "0")}`;
}

function formatTimelineTimestamp(seconds: number) {
  const totalFrames = Math.floor(Math.max(0, seconds) * 25);
  const frames = totalFrames % 25;
  const totalSeconds = Math.floor(totalFrames / 25);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const secs = totalSeconds % 60;
  return `${hours.toString().padStart(2, "0")}:${minutes.toString().padStart(2, "0")}:${secs
    .toString()
    .padStart(2, "0")}:${frames.toString().padStart(2, "0")}`;
}

function normalizeSegments(segments: SubtitleSegment[]) {
  return segments.map((segment, index) => {
    const start = Math.max(0, Number.isFinite(segment.start) ? segment.start : 0);
    const end = Math.max(start + 0.1, Number.isFinite(segment.end) ? segment.end : start + 1);
    return {
      ...segment,
      index: index + 1,
      start,
      end,
    };
  });
}

function plainText(segments: SubtitleSegment[]) {
  return segments.map((segment) => segment.text.trim()).filter(Boolean).join("\n");
}

function timelineText(segments: SubtitleSegment[]) {
  return segments
    .map((segment) => `[${formatTimelineTimestamp(segment.start)} --> ${formatTimelineTimestamp(segment.end)}]\n${segment.text.trim()}`)
    .join("\n\n");
}

function srtText(segments: SubtitleSegment[]) {
  return `${segments
    .map(
      (segment, index) =>
        `${index + 1}\n${formatSrtTimestamp(segment.start)} --> ${formatSrtTimestamp(segment.end)}\n${segment.text.trim()}`,
    )
    .join("\n\n")}\n`;
}

function joinSubtitleText(left: string, right: string) {
  const leftText = left.trim();
  const rightText = right.trim();
  if (!leftText) {
    return rightText;
  }
  if (!rightText) {
    return leftText;
  }
  const needsSpace = /[A-Za-z0-9]$/.test(leftText) && /^[A-Za-z0-9]/.test(rightText);
  return `${leftText}${needsSpace ? " " : ""}${rightText}`;
}

function stemFromFileName(fileName: string) {
  return fileName.replace(/\.[^.]+$/, "") || "subtitle";
}

function segmentSuggestedName(fileName: string, segment: SubtitleSegment) {
  return `${stemFromFileName(fileName)}-segment-${segment.index.toString().padStart(3, "0")}.wav`;
}

function isDesktopRuntime() {
  try {
    return isTauri();
  } catch {
    return false;
  }
}

function audioPathToSrc(path: string) {
  try {
    return isDesktopRuntime() ? convertFileSrc(path) : path;
  } catch {
    return path;
  }
}

function estimatedClipPadding(segment: SubtitleSegment, timelineKind?: TimelineKind) {
  if (timelineKind !== "estimated") {
    return { before: 0, after: 0 };
  }

  const duration = Math.max(0.1, segment.end - segment.start);
  return {
    before: Math.min(3, Math.max(0.75, duration * 0.4)),
    after: Math.min(1, Math.max(0.25, duration * 0.15)),
  };
}

function audioBoundsForSegment(segment: SubtitleSegment, timelineKind?: TimelineKind) {
  const padding = estimatedClipPadding(segment, timelineKind);
  return {
    start: Math.max(0, segment.start - padding.before),
    end: Math.max(segment.start + 0.1, segment.end + padding.after),
  };
}

export function SubtitleEditorPage({ project, onProjectChange, termCategories, onAddTermRule }: SubtitleEditorPageProps) {
  const audioRef = useRef<HTMLAudioElement>(null);
  const [query, setQuery] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(project?.segments[0]?.id ?? null);
  const [selectedClipIds, setSelectedClipIds] = useState<Set<string>>(() => new Set());
  const [clipExportDir, setClipExportDir] = useState("");
  const [suggestion, setSuggestion] = useState<{ source: string; target: string; categoryId: string } | null>(null);
  const [message, setMessage] = useState("");
  const [audioSrc, setAudioSrc] = useState("");
  const segments = project?.segments ?? [];
  const selected = segments.find((segment) => segment.id === selectedId) ?? segments[0];
  const selectedClipSegments = segments.filter((segment) => selectedClipIds.has(segment.id));
  const exportTargets = selectedClipSegments.length > 0 ? selectedClipSegments : selected ? [selected] : [];
  const filteredSegments = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) {
      return segments;
    }
    return segments.filter((segment) => segment.text.toLowerCase().includes(needle));
  }, [query, segments]);
  useEffect(() => {
    setSelectedId((current) => (segments.some((segment) => segment.id === current) ? current : segments[0]?.id ?? null));
    setSelectedClipIds((current) => new Set([...current].filter((id) => segments.some((segment) => segment.id === id))));
  }, [segments]);

  useEffect(() => {
    let cancelled = false;
    const sourceAudioPath = project?.sourceAudioPath;

    if (!sourceAudioPath) {
      setAudioSrc("");
      return;
    }

    if (!isDesktopRuntime()) {
      setAudioSrc(sourceAudioPath);
      return;
    }

    setAudioSrc("");
    void prepareAudioPreview(sourceAudioPath)
      .then((previewPath) => {
        if (!cancelled) {
          setAudioSrc(audioPathToSrc(previewPath));
        }
      })
      .catch((error) => {
        if (!cancelled) {
          setMessage(error instanceof Error ? `音频预览准备失败：${error.message}` : "音频预览准备失败。");
        }
      });

    return () => {
      cancelled = true;
    };
  }, [project?.sourceAudioPath]);

  useEffect(() => {
    setQuery("");
    setSuggestion(null);
    setMessage("");
    setSelectedClipIds(new Set());
    setClipExportDir("");
  }, [project?.fileName, project?.sourceAudioPath]);

  function updateSegments(nextSegments: SubtitleSegment[]) {
    if (!project) {
      return;
    }
    onProjectChange({ ...project, text: plainText(nextSegments), segments: normalizeSegments(nextSegments) });
  }

  function updateSegment(id: string, patch: Partial<SubtitleSegment>) {
    updateSegments(segments.map((segment) => (segment.id === id ? { ...segment, ...patch } : segment)));
  }

  function updateSelectedText(nextText: string) {
    if (!selected) {
      return;
    }
    if (selected.text.trim() && nextText.trim() && selected.text.trim() !== nextText.trim()) {
      setSuggestion({
        source: selected.text.trim(),
        target: nextText.trim(),
        categoryId: termCategories[0]?.id ?? "replacements",
      });
    }
    updateSegment(selected.id, { text: nextText });
  }

  function selectSegment(segment: SubtitleSegment) {
    setSelectedId(segment.id);
    if (audioRef.current) {
      audioRef.current.currentTime = audioBoundsForSegment(segment, project?.timelineKind).start;
      void audioRef.current.play().catch(() => {});
    }
  }

  function toggleClipSelection(segmentId: string) {
    setSelectedClipIds((current) => {
      const next = new Set(current);
      if (next.has(segmentId)) {
        next.delete(segmentId);
      } else {
        next.add(segmentId);
      }
      return next;
    });
  }

  function selectAllClips() {
    setSelectedClipIds(new Set(segments.map((segment) => segment.id)));
  }

  function clearClipSelection() {
    setSelectedClipIds(new Set());
  }

  function splitSelected() {
    if (!selected) {
      return;
    }
    const index = segments.findIndex((segment) => segment.id === selected.id);
    if (index < 0) {
      return;
    }
    const midpoint = selected.start + (selected.end - selected.start) / 2;
    const text = selected.text.trim();
    if (text.length < 2) {
      setMessage("字幕文字太短，无法拆分。");
      return;
    }
    const splitAt = Math.max(1, Math.floor(text.length / 2));
    const next = [
      ...segments.slice(0, index),
      { ...selected, end: midpoint, text: text.slice(0, splitAt).trim() || selected.text },
      {
        ...selected,
        id: `subtitle-${Date.now()}-${Math.random().toString(16).slice(2)}`,
        start: midpoint,
        text: text.slice(splitAt).trim(),
      },
      ...segments.slice(index + 1),
    ];
    updateSegments(next);
  }

  function mergeSelectedWithNext() {
    if (!selected) {
      return;
    }
    const index = segments.findIndex((segment) => segment.id === selected.id);
    const nextSegment = segments[index + 1];
    if (index < 0 || !nextSegment) {
      return;
    }

    updateSegments([
      ...segments.slice(0, index),
      {
        ...selected,
        end: nextSegment.end,
        text: joinSubtitleText(selected.text, nextSegment.text),
      },
      ...segments.slice(index + 2),
    ]);
  }

  async function exportText(format: "plainText" | "timelineText" | "srt") {
    if (!project) {
      return;
    }
    const stem = stemFromFileName(project.fileName);
    const content = format === "srt" ? srtText(segments) : format === "timelineText" ? timelineText(segments) : plainText(segments);
    const extension = format === "srt" ? "srt" : "txt";
    const suffix = format === "timelineText" ? "-timeline" : "";
    try {
      const path = await saveTextFile(`${stem}${suffix}.${extension}`, content);
      setMessage(path ? `已导出：${path}` : "已取消导出。");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "字幕导出失败。");
    }
  }

  async function handleSelectClipExportDir() {
    try {
      const directory = await selectDirectory();
      if (directory) {
        setClipExportDir(directory);
        setMessage(`片段导出目录：${directory}`);
      }
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "选择片段导出目录失败。");
    }
  }

  async function handleExportSelectedAudio() {
    if (exportTargets.length === 0) {
      return;
    }
    if (!project?.sourceAudioPath) {
      setMessage("当前项目没有原始音频路径，无法导出音频片段。");
      return;
    }

    try {
      const paths = [];
      for (const segment of exportTargets) {
        const bounds = audioBoundsForSegment(segment, project.timelineKind);
        paths.push(
          await exportAudioSegment(project.sourceAudioPath, bounds.start, bounds.end, {
            destinationDir: clipExportDir || undefined,
            suggestedName: segmentSuggestedName(project.fileName, segment),
          }),
        );
      }
      setMessage(`已导出 ${paths.length} 段音频：${paths.join("；")}`);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "导出音频片段失败。");
    }
  }

  function handleAddSuggestion() {
    if (!suggestion) {
      return;
    }
    onAddTermRule({
      id: `rule-${Date.now()}-${Math.random().toString(16).slice(2)}`,
      source: suggestion.source,
      target: suggestion.target,
      categoryId: suggestion.categoryId,
      enabled: true,
      hitCount: 0,
    });
    setMessage("已加入术语库。");
    setSuggestion(null);
  }

  if (!project || segments.length === 0) {
    return (
      <section className="panel subtitle-empty">
        <p className="section-label">字幕编辑</p>
        <h2>还没有可编辑的字幕</h2>
        <p className="empty-state">请先在转录页完成一个文件转录，再从任务行进入编辑字幕。</p>
      </section>
    );
  }

  return (
    <div className="page-stack subtitle-editor-page">
      <section className="panel subtitle-player-panel">
        <div>
          <p className="section-label">字幕编辑</p>
          <h2>{project.fileName}</h2>
        </div>
        <audio ref={audioRef} controls src={audioSrc} />
        <div className="subtitle-toolbar">
          <label className="search-input">
            <Search size={16} />
            <input value={query} placeholder="搜索字幕文字" onChange={(event) => setQuery(event.target.value)} />
          </label>
          <button className="secondary-button" type="button" disabled={!selected} onClick={() => selected && selectSegment(selected)}>
            <StepForward size={16} />
            跳转选中
          </button>
          <button className="secondary-button" type="button" disabled={!selected} onClick={splitSelected}>
            <Scissors size={16} />
            拆分
          </button>
          <button className="secondary-button" type="button" disabled={!selected} onClick={mergeSelectedWithNext}>
            <Merge size={16} />
            合并下一条
          </button>
          <button
            className="primary-button"
            type="button"
            disabled={exportTargets.length === 0 || !project.sourceAudioPath}
            onClick={() => void handleExportSelectedAudio()}
          >
            <FileAudio size={16} />
            导出选中片段
          </button>
          <button className="secondary-button" type="button" onClick={() => void handleSelectClipExportDir()}>
            <FolderOpen size={16} />
            自定义片段目录
          </button>
        </div>
        <p className="panel-hint">
          {clipExportDir ? `片段导出目录：${clipExportDir}` : "片段默认导出到原始音频所在目录。"}
          {selectedClipSegments.length > 0 ? ` 已勾选 ${selectedClipSegments.length} 条。` : " 未勾选时导出当前选中字幕。"}
        </p>
      </section>

      <section className="subtitle-editor-grid">
        <div className="panel subtitle-list-panel">
          <div className="panel-heading panel-heading--compact">
            <div>
              <p className="section-label">时间线</p>
              <p className="panel-hint">当前时间线为估算结果，可手动校正。</p>
            </div>
            <div className="button-group">
              <button className="secondary-button" type="button" onClick={selectAllClips}>
                全选
              </button>
              <button className="secondary-button" type="button" onClick={clearClipSelection}>
                清空
              </button>
            </div>
          </div>
          <div className="subtitle-list">
            {filteredSegments.map((segment) => (
              <div className="subtitle-row-wrap" key={segment.id}>
                <label className="subtitle-clip-check">
                  <input
                    aria-label={`选择第 ${segment.index} 条字幕`}
                    checked={selectedClipIds.has(segment.id)}
                    type="checkbox"
                    onChange={() => toggleClipSelection(segment.id)}
                  />
                </label>
                <button
                  className={selected?.id === segment.id ? "subtitle-row subtitle-row--active" : "subtitle-row"}
                  type="button"
                  onClick={() => selectSegment(segment)}
                >
                  <span>{segment.index}</span>
                  <strong>
                    {formatSrtTimestamp(segment.start)} - {formatSrtTimestamp(segment.end)}
                  </strong>
                  <p>{segment.text}</p>
                </button>
              </div>
            ))}
          </div>
        </div>

        <div className="panel subtitle-detail-panel">
          <p className="section-label">选中字幕</p>
          {selected && (
            <div className="subtitle-detail-form">
              <label>
                <span>开始秒</span>
                <input
                  type="number"
                  step="0.01"
                  value={selected.start}
                  onChange={(event) => updateSegment(selected.id, { start: Number(event.target.value) })}
                />
              </label>
              <label>
                <span>结束秒</span>
                <input
                  type="number"
                  step="0.01"
                  value={selected.end}
                  onChange={(event) => updateSegment(selected.id, { end: Number(event.target.value) })}
                />
              </label>
              <label className="subtitle-textarea-label">
                <span>字幕文字</span>
                <textarea value={selected.text} onChange={(event) => updateSelectedText(event.target.value)} />
              </label>
              {suggestion && (
                <div className="term-suggestion">
                  <div>
                    <span>建议加入术语库</span>
                    <strong>{suggestion.source} → {suggestion.target}</strong>
                  </div>
                  <select
                    value={suggestion.categoryId}
                    onChange={(event) => setSuggestion({ ...suggestion, categoryId: event.target.value })}
                  >
                    {termCategories.map((category) => (
                      <option key={category.id} value={category.id}>
                        {category.name}
                      </option>
                    ))}
                  </select>
                  <button className="secondary-button" type="button" onClick={handleAddSuggestion}>
                    加入术语库
                  </button>
                </div>
              )}
              <div className="button-group">
                <button className="secondary-button" type="button" onClick={() => void exportText("plainText")}>
                  <Download size={16} />
                  纯文本
                </button>
                <button className="secondary-button" type="button" onClick={() => void exportText("timelineText")}>
                  <Download size={16} />
                  时间线 TXT
                </button>
                <button className="secondary-button" type="button" onClick={() => void exportText("srt")}>
                  <Subtitles size={16} />
                  SRT
                </button>
              </div>
            </div>
          )}
          {message && <p className="model-message">{message}</p>}
        </div>
      </section>
    </div>
  );
}
