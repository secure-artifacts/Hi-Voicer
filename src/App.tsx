import { useEffect, useState } from "react";
import { AppShell } from "./components/AppShell";
import { initialDiagnostics, initialHotwords, initialSettings, initialStatus, initialTasks } from "./data/mockState";
import { findModelPreset } from "./data/modelPresets";
import { loadSettings, saveSettings } from "./lib/api";
import { DiagnosticsPage } from "./pages/DiagnosticsPage";
import { HomePage } from "./pages/HomePage";
import { HotwordsPage } from "./pages/HotwordsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { TranscriptionPage } from "./pages/TranscriptionPage";
import type { AppPage, AppStatus, UserSettings } from "./types";

export default function App() {
  const [currentPage, setCurrentPage] = useState<AppPage>("home");
  const [settings, setSettings] = useState<UserSettings>(initialSettings);

  useEffect(() => {
    void loadSettings(initialSettings).then(setSettings);
  }, []);

  function handleSettingsChange(nextSettings: UserSettings) {
    setSettings(nextSettings);
    void saveSettings(nextSettings);
  }

  const selectedModel = findModelPreset(settings.selectedModelId);
  const status: AppStatus = {
    ...initialStatus,
    readiness: settings.modelDir ? "ready" : "model-required",
    shortcut: settings.shortcut,
    modelName: settings.modelDir ? selectedModel?.name ?? "自定义模型" : "未配置模型",
  };

  return (
    <AppShell status={status} currentPage={currentPage} onPageChange={setCurrentPage}>
      {currentPage === "home" && <HomePage status={status} />}
      {currentPage === "transcription" && <TranscriptionPage tasks={initialTasks} />}
      {currentPage === "hotwords" && <HotwordsPage rules={initialHotwords} />}
      {currentPage === "settings" && <SettingsPage settings={settings} onSettingsChange={handleSettingsChange} />}
      {currentPage === "diagnostics" && <DiagnosticsPage items={initialDiagnostics} />}
    </AppShell>
  );
}
