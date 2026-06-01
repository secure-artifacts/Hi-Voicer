import { Check, Copy, Download, FolderOpen, Mic, Radio, Settings2, Square, Trash2, Waves } from "lucide-react";
import type { AppStatus, RecordingMode, TranscriptHistoryItem } from "../types";

interface HomePageProps {
  status: AppStatus;
  onOpenRecordingsFolder: () => void;
  onOpenSettings: () => void;
  onToggleRecording: () => void;
  onRecordingModeChange: (mode: RecordingMode) => void;
  recordingLevel: number;
  transcriptHistory: TranscriptHistoryItem[];
  onCopyTranscript: (text: string) => void;
  onDownloadTranscript: (item: TranscriptHistoryItem) => void;
  onClearTranscriptHistory: () => void;
}

const modeCopy = {
  hold: {
    title: "可以开始说话",
    description: "在任意输入框按住快捷键说话，松开后自动识别并粘贴上屏。下面按钮用于手动测试录音链路。",
    input: "按住说话，上屏",
    recording: "松开或点击停止",
    idle: "开始录音",
  },
  toggle: {
    title: "连续识别模式",
    description: "按一下快捷键开始录音，可以松开键盘；再次按快捷键后停止识别并粘贴上屏。",
    input: "按一下开始，再按一下停止",
    recording: "停止并上屏",
    idle: "开始连续识别",
  },
  audioOnly: {
    title: "纯录音模式",
    description: "按快捷键开始录音，再按一次停止。结束后只保存音频，不识别、不粘贴，方便后续剪辑或归档。",
    input: "只保存音频",
    recording: "停止并保存",
    idle: "开始纯录音",
  },
} as const;

const recordingModes: Array<{
  id: RecordingMode;
  title: string;
  description: string;
  icon: typeof Mic;
}> = [
  {
    id: "hold",
    title: "按住说话",
    description: "按住快捷键录音，松开后识别并上屏。",
    icon: Mic,
  },
  {
    id: "toggle",
    title: "连续识别",
    description: "按一下开始，再按一下停止识别上屏。",
    icon: Radio,
  },
  {
    id: "audioOnly",
    title: "纯录音",
    description: "只保存音频，不识别、不粘贴。",
    icon: Waves,
  },
];

export function HomePage({
  status,
  onOpenRecordingsFolder,
  onOpenSettings,
  onToggleRecording,
  onRecordingModeChange,
  recordingLevel,
  transcriptHistory,
  onCopyTranscript,
  onDownloadTranscript,
  onClearTranscriptHistory,
}: HomePageProps) {
  const copy = modeCopy[status.recordingMode];
  const isReady = status.readiness === "ready";
  const waveHeights = [0.72, 1.1, 1.45, 1.05, 0.82].map((factor) =>
    Math.max(12, Math.round(14 + recordingLevel * factor * 30)),
  );

  return (
    <div className="page-grid page-grid--home">
      <section className="panel hero-panel">
        <p className="section-label">语音输入</p>
        <h2>{isReady ? copy.title : "先完成模型配置"}</h2>
        <p>
          {copy.description} 当前快捷键：<strong>{status.shortcut}</strong>
        </p>
        <div className="hero-actions">
          <button className="primary-button" type="button" disabled={!isReady} onClick={onToggleRecording}>
            {status.isRecording ? <Square size={17} /> : <Mic size={17} />}
            {status.isRecording ? copy.recording : copy.idle}
          </button>
          <button className="secondary-button" type="button" onClick={onOpenRecordingsFolder}>
            <FolderOpen size={17} />
            打开录音文件夹
          </button>
          {!isReady && (
            <button className="secondary-button" type="button" onClick={onOpenSettings}>
              <Settings2 size={17} />
              打开模型设置
            </button>
          )}
        </div>
      </section>

      <section className="panel recording-mode-panel">
        <p className="section-label">录制模式</p>
        <div className="mode-grid mode-grid--compact">
          {recordingModes.map((mode) => {
            const Icon = mode.icon;
            const isSelected = status.recordingMode === mode.id;
            return (
              <button
                className={isSelected ? "mode-card mode-card--selected" : "mode-card"}
                key={mode.id}
                type="button"
                onClick={() => onRecordingModeChange(mode.id)}
              >
                <span>
                  <Icon size={17} />
                  {mode.title}
                  {isSelected && <Check size={16} />}
                </span>
                <small>{mode.description}</small>
              </button>
            );
          })}
        </div>
      </section>

      {status.isRecording && (
        <div className="recording-wave" aria-live="polite">
          {waveHeights.map((height, index) => (
            <span key={index} style={{ height: `${height}px` }} />
          ))}
        </div>
      )}

      <section className="panel transcript-history-panel">
        <div className="panel-heading panel-heading--compact">
          <div>
            <p className="section-label">录制文字历史</p>
            <blockquote className="recent-result-inline">{status.lastResult}</blockquote>
          </div>
          <button
            className="secondary-button"
            type="button"
            disabled={transcriptHistory.length === 0}
            onClick={onClearTranscriptHistory}
          >
            <Trash2 size={16} />
            清空
          </button>
        </div>
        {transcriptHistory.length === 0 ? (
          <p className="empty-state">还没有录制文字。</p>
        ) : (
          <div className="history-list">
            {transcriptHistory.map((item) => (
              <article className="history-row" key={item.id}>
                <div>
                  <time>{new Date(item.createdAt).toLocaleString()}</time>
                  <p>{item.text}</p>
                  {item.outputPaths.length > 0 && <small>{item.outputPaths.join(" / ")}</small>}
                </div>
                <div className="history-actions">
                  <button className="icon-button" type="button" title="复制文字" onClick={() => onCopyTranscript(item.text)}>
                    <Copy size={16} />
                  </button>
                  <button className="icon-button" type="button" title="下载文字" onClick={() => onDownloadTranscript(item)}>
                    <Download size={16} />
                  </button>
                </div>
              </article>
            ))}
          </div>
        )}
      </section>

      <section className="panel info-list">
        <div>
          <span>模型</span>
          <strong>{status.modelName}</strong>
        </div>
        <div>
          <span>麦克风</span>
          <strong>{status.microphoneName}</strong>
        </div>
        <div>
          <span>录制模式</span>
          <strong>{copy.input}</strong>
        </div>
      </section>
    </div>
  );
}
