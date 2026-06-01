import { Check, Download, FolderOpen, Keyboard, Moon, Sun } from "lucide-react";
import type { KeyboardEvent } from "react";
import { useEffect, useRef, useState } from "react";
import { SettingRow } from "../components/SettingRow";
import { modelPresets } from "../data/modelPresets";
import { installModel, listenModelInstallProgress, openExternalUrl, selectDirectory } from "../lib/api";
import type { ThemeMode, UserSettings } from "../types";

interface SettingsPageProps {
  settings: UserSettings;
  onOpenRecordingsFolder: () => void;
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

export function SettingsPage({ settings, onOpenRecordingsFolder, onSettingsChange }: SettingsPageProps) {
  const [isCapturingShortcut, setIsCapturingShortcut] = useState(false);
  const [modelMessage, setModelMessage] = useState("");
  const [installingModelId, setInstallingModelId] = useState<string | null>(null);
  const shortcutButtonRef = useRef<HTMLButtonElement>(null);
  const selectedModel = modelPresets.find((model) => model.id === settings.selectedModelId) ?? modelPresets[0];

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listenModelInstallProgress((progress) => {
      if (!disposed) {
        setModelMessage(`${progress.message}（${progress.completed}/${progress.total}）`);
      }
    }).then((nextUnlisten) => {
      unlisten = nextUnlisten;
      if (disposed) {
        unlisten();
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  async function handleSelectModelDir() {
    try {
      setModelMessage("");
      const selected = await selectDirectory();
      if (selected) {
        onSettingsChange({ ...settings, modelDir: selected });
        setModelMessage("模型目录已保存。");
      } else {
        setModelMessage("没有选择目录。");
      }
    } catch (error) {
      setModelMessage(error instanceof Error ? error.message : "打开目录选择失败。");
    }
  }

  async function handleInstallSelectedModel() {
    if (selectedModel.installKind === "engineRequired") {
      setModelMessage(selectedModel.engineNote);
      await openExternalUrl(selectedModel.downloadUrl);
      return;
    }

    try {
      setInstallingModelId(selectedModel.id);
      setModelMessage(`正在下载并配置 ${selectedModel.name}，首次安装会比较慢，请不要关闭软件。`);
      const modelDir = await installModel(selectedModel);
      onSettingsChange({ ...settings, selectedModelId: selectedModel.id, modelDir });
      setModelMessage(`已安装到 ${modelDir}`);
    } catch (error) {
      setModelMessage(error instanceof Error ? error.message : "模型安装失败。");
    } finally {
      setInstallingModelId(null);
    }
  }

  function updateTheme(theme: ThemeMode) {
    onSettingsChange({ ...settings, theme });
  }

  return (
    <section className="panel settings-panel">
      <p className="section-label">设置</p>

      <SettingRow label="界面皮肤" description="暗色不是纯黑，适合夜间长时间使用。">
        <div className="segmented-control" role="group" aria-label="界面皮肤">
          <button
            className={settings.theme === "light" ? "segment-button segment-button--active" : "segment-button"}
            type="button"
            onClick={() => updateTheme("light")}
          >
            <Sun size={16} />
            亮色
          </button>
          <button
            className={settings.theme === "dark" ? "segment-button segment-button--active" : "segment-button"}
            type="button"
            onClick={() => updateTheme("dark")}
          >
            <Moon size={16} />
            暗色
          </button>
        </div>
      </SettingRow>

      <SettingRow label="快捷键" description="点击右侧按钮后，直接按键盘上的按键或组合键。">
        <button
          ref={shortcutButtonRef}
          className={`shortcut-capture ${isCapturingShortcut ? "shortcut-capture--active" : ""}`}
          type="button"
          onClick={() => {
            setIsCapturingShortcut(true);
            shortcutButtonRef.current?.focus();
          }}
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
            <p>先选一个预设模型，需要时一键下载。可自动配置的模型会直接放到本机固定目录。</p>
          </div>
          <button className="secondary-button" type="button" onClick={handleSelectModelDir}>
            <FolderOpen size={17} />
            选择已有模型目录
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
                <span>
                  {model.size} / {model.quality}
                </span>
                <span>{model.memory}</span>
                <small>{model.recommendedFor}</small>
                <em>{model.installKind === "sherpaOnnx" ? "可自动配置 Sherpa" : "需要后续接入引擎"}</em>
              </button>
            );
          })}
        </div>

        <div className="model-actions">
          <div>
            <span>当前模型目录</span>
            <strong>{settings.modelDir || "尚未选择"}</strong>
          </div>
          <button
            className="primary-button"
            type="button"
            disabled={installingModelId !== null}
            onClick={() => void handleInstallSelectedModel()}
          >
            <Download size={17} />
            {selectedModel.installKind === "sherpaOnnx" ? `下载并配置 ${selectedModel.name}` : `查看 ${selectedModel.name}`}
          </button>
        </div>
        {modelMessage && <p className="model-message">{modelMessage}</p>}
      </div>

      <SettingRow label="录音文件夹" description="纯录音模式和保留录音片段都会保存到这里。">
        <button className="path-button" type="button" onClick={onOpenRecordingsFolder}>
          <FolderOpen size={17} />
          <span>打开录音文件夹</span>
        </button>
      </SettingRow>

      <SettingRow label="保留识别录音" description="开启后会保留每次识别前的录音片段，便于排查识别问题。">
        <input
          aria-label="保留识别录音"
          type="checkbox"
          checked={settings.saveRecordings}
          onChange={(event) => onSettingsChange({ ...settings, saveRecordings: event.target.checked })}
        />
      </SettingRow>

      <SettingRow label="开机启动" description="开启后登录 Windows 自动启动，并安静驻留在托盘。">
        <input
          aria-label="开机启动"
          type="checkbox"
          checked={settings.launchAtStartup}
          onChange={(event) => onSettingsChange({ ...settings, launchAtStartup: event.target.checked })}
        />
      </SettingRow>

      <SettingRow label="显示悬浮按钮" description="开启后显示一个置顶 mini 录制按钮；关闭后只保留主窗口和快捷键。">
        <input
          aria-label="显示悬浮按钮"
          type="checkbox"
          checked={settings.showMiniWindow}
          onChange={(event) => onSettingsChange({ ...settings, showMiniWindow: event.target.checked })}
        />
      </SettingRow>
    </section>
  );
}
