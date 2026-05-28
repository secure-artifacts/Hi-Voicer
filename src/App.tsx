import { useEffect, useState } from "react";
import { AppShell } from "./components/AppShell";
import { initialDiagnostics, initialHotwords, initialSettings, initialStatus, initialTasks } from "./data/mockState";
import { loadSettings, saveSettings } from "./lib/api";
import { DiagnosticsPage } from "./pages/DiagnosticsPage";
import { HomePage } from "./pages/HomePage";
import { HotwordsPage } from "./pages/HotwordsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { TranscriptionPage } from "./pages/TranscriptionPage";
import type { AppPage, UserSettings } from "./types";

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

  const status = { ...initialStatus, shortcut: settings.shortcut };

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
