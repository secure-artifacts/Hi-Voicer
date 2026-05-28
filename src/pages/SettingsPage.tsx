import { SettingRow } from "../components/SettingRow";
import type { UserSettings } from "../types";

interface SettingsPageProps {
  settings: UserSettings;
  onSettingsChange: (settings: UserSettings) => void;
}

export function SettingsPage({ settings, onSettingsChange }: SettingsPageProps) {
  return (
    <section className="panel settings-panel">
      <p className="section-label">设置</p>
      <SettingRow label="按住说话快捷键" description="第一版默认使用 CapsLock。">
        <input
          value={settings.shortcut}
          onChange={(event) => onSettingsChange({ ...settings, shortcut: event.target.value })}
        />
      </SettingRow>
      <SettingRow label="模型目录" description="选择本地离线模型所在目录。">
        <input
          value={settings.modelDir}
          placeholder="尚未选择"
          onChange={(event) => onSettingsChange({ ...settings, modelDir: event.target.value })}
        />
      </SettingRow>
      <SettingRow label="输出目录" description="文件转录结果保存位置。">
        <input
          value={settings.outputDir}
          placeholder="默认保存到源文件旁边"
          onChange={(event) => onSettingsChange({ ...settings, outputDir: event.target.value })}
        />
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
