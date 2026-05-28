import { useState } from "react";
import { AppShell } from "./components/AppShell";
import { initialDiagnostics, initialHotwords, initialSettings, initialStatus, initialTasks } from "./data/mockState";
import { DiagnosticsPage } from "./pages/DiagnosticsPage";
import { HomePage } from "./pages/HomePage";
import { HotwordsPage } from "./pages/HotwordsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { TranscriptionPage } from "./pages/TranscriptionPage";
import type { AppPage, UserSettings } from "./types";

export default function App() {
  const [currentPage, setCurrentPage] = useState<AppPage>("home");
  const [settings, setSettings] = useState<UserSettings>(initialSettings);

  return (
    <AppShell status={initialStatus} currentPage={currentPage} onPageChange={setCurrentPage}>
      {currentPage === "home" && <HomePage status={initialStatus} />}
      {currentPage === "transcription" && <TranscriptionPage tasks={initialTasks} />}
      {currentPage === "hotwords" && <HotwordsPage rules={initialHotwords} />}
      {currentPage === "settings" && <SettingsPage settings={settings} onSettingsChange={setSettings} />}
      {currentPage === "diagnostics" && <DiagnosticsPage items={initialDiagnostics} />}
    </AppShell>
  );
}
