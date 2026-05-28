import { Check, Download, FolderOpen, Keyboard } from "lucide-react";
import type { KeyboardEvent } from "react";
import { useState } from "react";
import { SettingRow } from "../components/SettingRow";
import { modelPresets } from "../data/modelPresets";
import { openExternalUrl, selectDirectory } from "../lib/api";
import type { UserSettings } from "../types";

interface SettingsPageProps {
  settings: UserSettings;
  onSettingsChange: (settings: UserSettings) => void;
}

function formatShortcut(event: KeyboardEvent<HTMLButtonElement>) {
  const parts: string[] = [];

  if (event.ctrlKey) {
    parts.push("Ctrl");
  }

  if (event.altKey) {
    parts.push("Alt");
  }

  if (event.shiftKey) {
    parts.push("Shift");
  }

  if (event.metaKey) {
    parts.push("Win");
  }

  const keyMap: Record<string, string> = {
    " ": "Space",
    Escape: "Esc",
    ArrowUp: "Up",
    ArrowDown: "Down",
    ArrowLeft: "Left",
    ArrowRight: "Right",
  };
  const key = keyMap[event.key] ?? event.key;

  if (!["Control", "Alt", "Shift", "Meta"].includes(event.key)) {
    parts.push(key.length === 1 ? key.toUpperCase() : key);
  }

  return parts.join("+");
}

export function SettingsPage({ settings, onSettingsChange }: SettingsPageProps) {
  const [isCapturingShortcut, setIsCapturingShortcut] = useState(false);
  const selectedModel = modelPresets.find((model) => model.id === settings.selectedModelId) ?? modelPresets[0];

  async function handleSelectModelDir() {
    const selected = await selectDirectory();
    if (selected) {
      onSettingsChange({ ...settings, modelDir: selected });
    }
  }

  async function handleSelectOutputDir() {
    const selected = await selectDirectory();
    if (selected) {
      onSettingsChange({ ...settings, outputDir: selected });
    }
  }

  return (
    <section className="panel settings-panel">
      <p className="section-label">设置</p>

      <SettingRow label="按住说话快捷键" description="点右侧按钮后，直接按键盘上的按键或组合键。">
        <button
          className={`shortcut-capture ${isCapturingShortcut ? "shortcut-capture--active" : ""}`}
          type="button"
          onClick={() => setIsCapturingShortcut(true)}
          onBlur={() => setIsCapturingShortcut(false)}
          onKeyDown={(event) => {
            event.preventDefault();
            const nextShortcut = formatShortcut(event);
            if (nextShortcut) {
              onSettingsChange({ ...settings, shortcut: nextShortcut });
              setIsCapturingShortcut(false);
            }
          }}
        >
          <Keyboard size={18} />
          {isCapturingShortcut ? "请按键..." : settings.shortcut}
        </button>
      </SettingRow>

      <div className="setting-row setting-row--stacked">
        <div className="setting-heading">
          <div>
            <strong>离线模型</strong>
            <p>先选一个预设模型，需要时下载，下载后解压并选择模型目录。</p>
          </div>
          <button className="secondary-button" type="button" onClick={handleSelectModelDir}>
            <FolderOpen size={17} />
            选择模型目录
          </button>
        </div>

        <div className="model-grid">
          {modelPresets.map((model) => {
            const isSelected = model.id === selectedModel.id;

            return (
              <button
                className={`model-card ${isSelected ? "model-card--selected" : ""}`}
                key={model.id}
                type="button"
                onClick={() => onSettingsChange({ ...settings, selectedModelId: model.id })}
              >
                <span className="model-card__title">
                  {model.name}
                  {isSelected && <Check size={17} />}
                </span>
                <span>{model.size} · {model.quality}</span>
                <span>{model.memory}</span>
                <small>{model.recommendedFor}</small>
              </button>
            );
          })}
        </div>

        <div className="model-actions">
          <div>
            <span>当前模型目录</span>
            <strong>{settings.modelDir || "尚未选择"}</strong>
          </div>
          <button className="primary-button" type="button" onClick={() => void openExternalUrl(selectedModel.downloadUrl)}>
            <Download size={17} />
            下载{selectedModel.name}
          </button>
        </div>
      </div>

      <SettingRow label="输出目录" description="文件转录结果保存位置。留空时保存到源文件旁边。">
        <button className="path-button" type="button" onClick={handleSelectOutputDir}>
          <FolderOpen size={17} />
          <span>{settings.outputDir || "默认保存到源文件旁边"}</span>
        </button>
      </SettingRow>

      <SettingRow label="保存录音" description="开启后会保留每次按键录音片段。">
        <input
          type="checkbox"
          checked={settings.saveRecordings}
          onChange={(event) => onSettingsChange({ ...settings, saveRecordings: event.target.checked })}
        />
      </SettingRow>
    </section>
  );
}
