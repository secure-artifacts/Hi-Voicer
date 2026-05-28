# Hi-Voicer Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 搭建 Hi-Voicer 第一阶段项目基础：可运行的 Tauri + React + TypeScript 应用骨架、中文工具型 UI、类型化前后端通信、模拟状态和本地设置保存。

**Architecture:** 前端使用 React + TypeScript + Vite，负责应用外壳、页面、状态展示和设置表单。后端使用 Tauri/Rust，先提供轻量命令和状态结构，不接真实录音和 ASR；所有后续桌面能力都通过窄 Tauri 命令接入。

**Tech Stack:** Tauri 2, Rust, React, TypeScript, Vite, CSS Modules or plain CSS, Vitest, React Testing Library.

---

## 范围说明

完整产品包含 UI、托盘、快捷键、录音、ASR、文件转录、打包。这个计划只覆盖第一阶段“项目基础”，目标是先得到一个结构正确、界面可操作、后续能接真实能力的桌面应用骨架。

这个计划不实现真实 ASR、真实麦克风录音、全局快捷键、托盘、文件解码。这些会在后续计划中单独实现。

## 文件结构

创建或修改以下文件：

```text
package.json
index.html
vite.config.ts
tsconfig.json
src/main.tsx
src/App.tsx
src/styles.css
src/types.ts
src/data/mockState.ts
src/lib/api.ts
src/components/AppShell.tsx
src/components/StatusBadge.tsx
src/components/SettingRow.tsx
src/pages/HomePage.tsx
src/pages/TranscriptionPage.tsx
src/pages/HotwordsPage.tsx
src/pages/SettingsPage.tsx
src/pages/DiagnosticsPage.tsx
src/__tests__/settings.test.ts
src-tauri/Cargo.toml
src-tauri/tauri.conf.json
src-tauri/src/main.rs
src-tauri/src/app_state.rs
src-tauri/src/config.rs
```

职责边界：

1. `src/lib/api.ts` 只封装 Tauri 命令和浏览器开发环境 fallback。
2. `src/types.ts` 定义前端共享类型。
3. `src/data/mockState.ts` 提供第一阶段 UI 初始数据。
4. `src/components/*` 只做可复用 UI，不直接调用 Tauri。
5. `src/pages/*` 负责页面组合和局部交互。
6. `src-tauri/src/config.rs` 负责设置默认值、读取、保存。
7. `src-tauri/src/app_state.rs` 负责应用状态快照，不接真实 ASR。

---

### Task 1: 创建前端项目基础

**Files:**
- Create: `package.json`
- Create: `index.html`
- Create: `vite.config.ts`
- Create: `tsconfig.json`
- Create: `src/main.tsx`
- Create: `src/App.tsx`
- Create: `src/styles.css`

- [ ] **Step 1: 创建 `package.json`**

写入：

```json
{
  "name": "hi-voicer",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc && vite build",
    "preview": "vite preview",
    "test": "vitest run",
    "tauri": "tauri"
  },
  "dependencies": {
    "@tauri-apps/api": "^2.0.0",
    "lucide-react": "^0.468.0",
    "react": "^18.3.1",
    "react-dom": "^18.3.1"
  },
  "devDependencies": {
    "@testing-library/jest-dom": "^6.6.3",
    "@testing-library/react": "^16.1.0",
    "@types/react": "^18.3.18",
    "@types/react-dom": "^18.3.5",
    "@vitejs/plugin-react": "^4.3.4",
    "typescript": "^5.7.2",
    "vite": "^6.0.3",
    "vitest": "^2.1.8"
  }
}
```

- [ ] **Step 2: 创建 `index.html`**

写入：

```html
<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Hi-Voicer</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

- [ ] **Step 3: 创建 `vite.config.ts`**

写入：

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
});
```

- [ ] **Step 4: 创建 `tsconfig.json`**

写入：

```json
{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["DOM", "DOM.Iterable", "ES2020"],
    "allowJs": false,
    "skipLibCheck": true,
    "esModuleInterop": true,
    "allowSyntheticDefaultImports": true,
    "strict": true,
    "forceConsistentCasingInFileNames": true,
    "module": "ESNext",
    "moduleResolution": "Node",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noEmit": true,
    "jsx": "react-jsx",
    "types": ["vitest/globals", "@testing-library/jest-dom"]
  },
  "include": ["src"],
  "references": []
}
```

- [ ] **Step 5: 创建最小 React 入口**

`src/main.tsx`：

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
```

`src/App.tsx`：

```tsx
export default function App() {
  return (
    <main className="boot-screen">
      <h1>Hi-Voicer</h1>
      <p>离线中文语音输入和文件转录工作台</p>
    </main>
  );
}
```

`src/styles.css`：

```css
:root {
  font-family: "Microsoft YaHei", "Segoe UI", system-ui, sans-serif;
  color: #172033;
  background: #f6f7fb;
  font-synthesis: none;
  text-rendering: optimizeLegibility;
}

* {
  box-sizing: border-box;
}

body {
  margin: 0;
  min-width: 1024px;
  min-height: 720px;
}

button,
input,
select,
textarea {
  font: inherit;
}

.boot-screen {
  min-height: 100vh;
  display: grid;
  place-items: center;
  align-content: center;
  gap: 12px;
}

.boot-screen h1 {
  margin: 0;
  font-size: 40px;
}

.boot-screen p {
  margin: 0;
  color: #5d6a7c;
}
```

- [ ] **Step 6: 安装依赖**

Run: `npm install`

Expected: creates `package-lock.json` and installs dependencies without errors.

- [ ] **Step 7: 验证前端能构建**

Run: `npm run build`

Expected: `tsc && vite build` succeeds and creates `dist`.

- [ ] **Step 8: Commit**

```bash
git add Hi-Voicer/package.json Hi-Voicer/package-lock.json Hi-Voicer/index.html Hi-Voicer/vite.config.ts Hi-Voicer/tsconfig.json Hi-Voicer/src/main.tsx Hi-Voicer/src/App.tsx Hi-Voicer/src/styles.css
git commit -m "feat: scaffold Hi-Voicer frontend"
```

---

### Task 2: 创建共享类型和模拟状态

**Files:**
- Create: `src/types.ts`
- Create: `src/data/mockState.ts`
- Modify: `src/App.tsx`

- [ ] **Step 1: 创建共享类型**

`src/types.ts`：

```ts
export type ReadinessState =
  | "starting"
  | "loading-model"
  | "ready"
  | "model-required"
  | "microphone-unavailable"
  | "error";

export type AppPage = "home" | "transcription" | "hotwords" | "settings" | "diagnostics";

export type PasteMode = "direct" | "clipboard";

export interface AppStatus {
  readiness: ReadinessState;
  modelName: string;
  shortcut: string;
  microphoneName: string;
  lastResult: string;
  isRecording: boolean;
}

export interface TranscriptTask {
  id: string;
  fileName: string;
  status: "queued" | "running" | "done" | "failed";
  progress: number;
  outputFormats: Array<"txt" | "srt" | "json">;
  message: string;
}

export interface HotwordRule {
  id: string;
  source: string;
  target: string;
  enabled: boolean;
}

export interface UserSettings {
  shortcut: string;
  modelDir: string;
  outputDir: string;
  pasteMode: PasteMode;
  saveRecordings: boolean;
  launchAtStartup: boolean;
}

export interface DiagnosticItem {
  id: string;
  label: string;
  status: "ok" | "warning" | "error";
  detail: string;
}
```

- [ ] **Step 2: 创建模拟状态**

`src/data/mockState.ts`：

```ts
import type { AppStatus, DiagnosticItem, HotwordRule, TranscriptTask, UserSettings } from "../types";

export const initialStatus: AppStatus = {
  readiness: "model-required",
  modelName: "未配置模型",
  shortcut: "CapsLock",
  microphoneName: "默认麦克风",
  lastResult: "模型配置完成后，这里会显示最近一次识别结果。",
  isRecording: false,
};

export const initialTasks: TranscriptTask[] = [
  {
    id: "sample-1",
    fileName: "会议录音示例.wav",
    status: "queued",
    progress: 0,
    outputFormats: ["txt", "srt", "json"],
    message: "等待模型准备完成",
  },
];

export const initialHotwords: HotwordRule[] = [
  { id: "rule-1", source: "太瑞", target: "Tauri", enabled: true },
  { id: "rule-2", source: "阿萨尔", target: "ASR", enabled: true },
];

export const initialSettings: UserSettings = {
  shortcut: "CapsLock",
  modelDir: "",
  outputDir: "",
  pasteMode: "clipboard",
  saveRecordings: false,
  launchAtStartup: false,
};

export const initialDiagnostics: DiagnosticItem[] = [
  {
    id: "model",
    label: "模型",
    status: "warning",
    detail: "尚未选择本地模型目录。",
  },
  {
    id: "microphone",
    label: "麦克风",
    status: "ok",
    detail: "已检测到默认麦克风。",
  },
  {
    id: "shortcut",
    label: "快捷键",
    status: "ok",
    detail: "默认快捷键 CapsLock 可用于按住说话。",
  },
];
```

- [ ] **Step 3: 更新 `src/App.tsx` 使用模拟状态**

```tsx
import { initialStatus } from "./data/mockState";

export default function App() {
  return (
    <main className="boot-screen">
      <h1>Hi-Voicer</h1>
      <p>离线中文语音输入和文件转录工作台</p>
      <p>当前快捷键：{initialStatus.shortcut}</p>
    </main>
  );
}
```

- [ ] **Step 4: 运行类型检查**

Run: `npm run build`

Expected: TypeScript build succeeds.

- [ ] **Step 5: Commit**

```bash
git add Hi-Voicer/src/types.ts Hi-Voicer/src/data/mockState.ts Hi-Voicer/src/App.tsx
git commit -m "feat: add Hi-Voicer state types"
```

---

### Task 3: 实现应用外壳和页面导航

**Files:**
- Create: `src/components/AppShell.tsx`
- Create: `src/components/StatusBadge.tsx`
- Modify: `src/App.tsx`
- Modify: `src/styles.css`

- [ ] **Step 1: 创建状态标签组件**

`src/components/StatusBadge.tsx`：

```tsx
import type { ReadinessState } from "../types";

const labels: Record<ReadinessState, string> = {
  starting: "启动中",
  "loading-model": "正在加载模型",
  ready: "可以录音",
  "model-required": "需要配置模型",
  "microphone-unavailable": "麦克风不可用",
  error: "异常",
};

export function StatusBadge({ state }: { state: ReadinessState }) {
  return <span className={`status-badge status-badge--${state}`}>{labels[state]}</span>;
}
```

- [ ] **Step 2: 创建应用外壳**

`src/components/AppShell.tsx`：

```tsx
import { Activity, FileAudio, Home, ListChecks, Settings, Wrench } from "lucide-react";
import type { AppPage, AppStatus } from "../types";
import { StatusBadge } from "./StatusBadge";

const pages: Array<{ id: AppPage; label: string; icon: React.ComponentType<{ size?: number }> }> = [
  { id: "home", label: "首页", icon: Home },
  { id: "transcription", label: "转录", icon: FileAudio },
  { id: "hotwords", label: "热词", icon: ListChecks },
  { id: "settings", label: "设置", icon: Settings },
  { id: "diagnostics", label: "诊断", icon: Wrench },
];

interface AppShellProps {
  status: AppStatus;
  currentPage: AppPage;
  onPageChange: (page: AppPage) => void;
  children: React.ReactNode;
}

export function AppShell({ status, currentPage, onPageChange, children }: AppShellProps) {
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">
            <Activity size={22} />
          </div>
          <div>
            <strong>Hi-Voicer</strong>
            <span>离线语音工作台</span>
          </div>
        </div>

        <nav className="nav-list" aria-label="主导航">
          {pages.map((page) => {
            const Icon = page.icon;
            return (
              <button
                key={page.id}
                className={page.id === currentPage ? "nav-item nav-item--active" : "nav-item"}
                onClick={() => onPageChange(page.id)}
                type="button"
              >
                <Icon size={18} />
                {page.label}
              </button>
            );
          })}
        </nav>
      </aside>

      <section className="workspace">
        <header className="topbar">
          <div>
            <p className="eyebrow">本地离线模式</p>
            <h1>中文语音输入与文件转录</h1>
          </div>
          <div className="topbar-status">
            <StatusBadge state={status.readiness} />
            <span>{status.shortcut}</span>
          </div>
        </header>
        {children}
      </section>
    </div>
  );
}
```

- [ ] **Step 3: 更新 `src/App.tsx` 导航状态**

```tsx
import { useState } from "react";
import { AppShell } from "./components/AppShell";
import { initialStatus } from "./data/mockState";
import type { AppPage } from "./types";

export default function App() {
  const [currentPage, setCurrentPage] = useState<AppPage>("home");

  return (
    <AppShell status={initialStatus} currentPage={currentPage} onPageChange={setCurrentPage}>
      <div className="page-placeholder">
        <h2>{currentPage}</h2>
        <p>页面内容将在后续任务中接入。</p>
      </div>
    </AppShell>
  );
}
```

- [ ] **Step 4: 更新样式**

Append to `src/styles.css`:

```css
.app-shell {
  min-height: 100vh;
  display: grid;
  grid-template-columns: 248px 1fr;
  background: #f3f5f9;
}

.sidebar {
  border-right: 1px solid #dde3ee;
  background: #ffffff;
  padding: 24px 18px;
}

.brand {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 28px;
}

.brand-mark {
  width: 42px;
  height: 42px;
  display: grid;
  place-items: center;
  color: #ffffff;
  background: #2563eb;
  border-radius: 8px;
}

.brand strong,
.brand span {
  display: block;
}

.brand span {
  margin-top: 3px;
  color: #6b7280;
  font-size: 13px;
}

.nav-list {
  display: grid;
  gap: 6px;
}

.nav-item {
  height: 42px;
  display: flex;
  align-items: center;
  gap: 10px;
  border: 0;
  border-radius: 8px;
  padding: 0 12px;
  color: #3f4a5f;
  background: transparent;
  cursor: pointer;
}

.nav-item--active {
  color: #174ea6;
  background: #e8f0ff;
}

.workspace {
  min-width: 0;
  padding: 24px 28px;
}

.topbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 24px;
}

.topbar h1 {
  margin: 4px 0 0;
  font-size: 24px;
  line-height: 1.2;
}

.eyebrow {
  margin: 0;
  color: #64748b;
  font-size: 13px;
}

.topbar-status {
  display: flex;
  align-items: center;
  gap: 10px;
  color: #536174;
}

.status-badge {
  display: inline-flex;
  align-items: center;
  height: 30px;
  border-radius: 999px;
  padding: 0 12px;
  font-size: 13px;
  font-weight: 600;
}

.status-badge--ready {
  color: #166534;
  background: #dcfce7;
}

.status-badge--model-required,
.status-badge--loading-model,
.status-badge--starting {
  color: #92400e;
  background: #fef3c7;
}

.status-badge--microphone-unavailable,
.status-badge--error {
  color: #991b1b;
  background: #fee2e2;
}

.page-placeholder {
  border: 1px solid #dde3ee;
  border-radius: 8px;
  background: #ffffff;
  padding: 24px;
}
```

- [ ] **Step 5: 构建验证**

Run: `npm run build`

Expected: build succeeds.

- [ ] **Step 6: Commit**

```bash
git add Hi-Voicer/src/components/AppShell.tsx Hi-Voicer/src/components/StatusBadge.tsx Hi-Voicer/src/App.tsx Hi-Voicer/src/styles.css
git commit -m "feat: add Hi-Voicer app shell"
```

---

### Task 4: 实现五个主要页面静态 UI

**Files:**
- Create: `src/components/SettingRow.tsx`
- Create: `src/pages/HomePage.tsx`
- Create: `src/pages/TranscriptionPage.tsx`
- Create: `src/pages/HotwordsPage.tsx`
- Create: `src/pages/SettingsPage.tsx`
- Create: `src/pages/DiagnosticsPage.tsx`
- Modify: `src/App.tsx`
- Modify: `src/styles.css`

- [ ] **Step 1: 创建设置行组件**

`src/components/SettingRow.tsx`：

```tsx
interface SettingRowProps {
  label: string;
  description: string;
  children: React.ReactNode;
}

export function SettingRow({ label, description, children }: SettingRowProps) {
  return (
    <div className="setting-row">
      <div>
        <strong>{label}</strong>
        <p>{description}</p>
      </div>
      <div className="setting-control">{children}</div>
    </div>
  );
}
```

- [ ] **Step 2: 创建首页**

`src/pages/HomePage.tsx`：

```tsx
import type { AppStatus } from "../types";

export function HomePage({ status }: { status: AppStatus }) {
  return (
    <div className="page-grid page-grid--home">
      <section className="panel hero-panel">
        <p className="section-label">语音输入</p>
        <h2>{status.readiness === "ready" ? "可以开始说话" : "先完成模型配置"}</h2>
        <p>按住 {status.shortcut} 说话，松开后自动识别并输入到当前窗口。</p>
        <button className="primary-button" type="button">打开模型设置</button>
      </section>

      <section className="panel">
        <p className="section-label">最近结果</p>
        <blockquote>{status.lastResult}</blockquote>
      </section>

      <section className="panel info-list">
        <div><span>模型</span><strong>{status.modelName}</strong></div>
        <div><span>麦克风</span><strong>{status.microphoneName}</strong></div>
        <div><span>输入方式</span><strong>剪贴板兜底</strong></div>
      </section>
    </div>
  );
}
```

- [ ] **Step 3: 创建转录页**

`src/pages/TranscriptionPage.tsx`：

```tsx
import { Upload } from "lucide-react";
import type { TranscriptTask } from "../types";

export function TranscriptionPage({ tasks }: { tasks: TranscriptTask[] }) {
  return (
    <div className="page-stack">
      <section className="drop-zone">
        <Upload size={28} />
        <h2>拖入音频或视频文件</h2>
        <p>第一版将支持导出 txt、srt、json。真实转录会在 ASR 集成阶段接入。</p>
      </section>

      <section className="panel">
        <p className="section-label">任务队列</p>
        <div className="task-list">
          {tasks.map((task) => (
            <div className="task-row" key={task.id}>
              <div>
                <strong>{task.fileName}</strong>
                <p>{task.message}</p>
              </div>
              <span>{task.progress}%</span>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
```

- [ ] **Step 4: 创建热词页**

`src/pages/HotwordsPage.tsx`：

```tsx
import type { HotwordRule } from "../types";

export function HotwordsPage({ rules }: { rules: HotwordRule[] }) {
  return (
    <section className="panel">
      <div className="panel-heading">
        <div>
          <p className="section-label">热词和替换</p>
          <h2>让识别结果更像你的用词</h2>
        </div>
        <button className="secondary-button" type="button">新增规则</button>
      </div>
      <div className="rule-list">
        {rules.map((rule) => (
          <div className="rule-row" key={rule.id}>
            <span>{rule.source}</span>
            <strong>{rule.target}</strong>
            <em>{rule.enabled ? "启用" : "停用"}</em>
          </div>
        ))}
      </div>
    </section>
  );
}
```

- [ ] **Step 5: 创建设置页**

`src/pages/SettingsPage.tsx`：

```tsx
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
        <input value={settings.shortcut} onChange={(event) => onSettingsChange({ ...settings, shortcut: event.target.value })} />
      </SettingRow>
      <SettingRow label="模型目录" description="选择本地离线模型所在目录。">
        <input value={settings.modelDir} placeholder="尚未选择" onChange={(event) => onSettingsChange({ ...settings, modelDir: event.target.value })} />
      </SettingRow>
      <SettingRow label="输出目录" description="文件转录结果保存位置。">
        <input value={settings.outputDir} placeholder="默认保存到源文件旁边" onChange={(event) => onSettingsChange({ ...settings, outputDir: event.target.value })} />
      </SettingRow>
      <SettingRow label="保存录音" description="开启后会保留每次按键录音片段。">
        <input type="checkbox" checked={settings.saveRecordings} onChange={(event) => onSettingsChange({ ...settings, saveRecordings: event.target.checked })} />
      </SettingRow>
    </section>
  );
}
```

- [ ] **Step 6: 创建诊断页**

`src/pages/DiagnosticsPage.tsx`：

```tsx
import type { DiagnosticItem } from "../types";

export function DiagnosticsPage({ items }: { items: DiagnosticItem[] }) {
  return (
    <section className="panel">
      <p className="section-label">诊断</p>
      <div className="diagnostic-list">
        {items.map((item) => (
          <div className={`diagnostic-row diagnostic-row--${item.status}`} key={item.id}>
            <strong>{item.label}</strong>
            <p>{item.detail}</p>
          </div>
        ))}
      </div>
    </section>
  );
}
```

- [ ] **Step 7: 更新 `src/App.tsx` 组合页面**

```tsx
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
```

- [ ] **Step 8: 追加页面样式**

Append to `src/styles.css`:

```css
.page-grid {
  display: grid;
  gap: 18px;
}

.page-grid--home {
  grid-template-columns: minmax(0, 1.4fr) minmax(280px, 0.8fr);
}

.page-stack {
  display: grid;
  gap: 18px;
}

.panel,
.drop-zone {
  border: 1px solid #dde3ee;
  border-radius: 8px;
  background: #ffffff;
  padding: 22px;
}

.hero-panel {
  min-height: 260px;
}

.hero-panel h2,
.drop-zone h2,
.panel-heading h2 {
  margin: 6px 0 8px;
  font-size: 26px;
}

.section-label {
  margin: 0;
  color: #2563eb;
  font-size: 13px;
  font-weight: 700;
}

.primary-button,
.secondary-button {
  height: 38px;
  border-radius: 8px;
  border: 0;
  padding: 0 14px;
  cursor: pointer;
}

.primary-button {
  color: #ffffff;
  background: #2563eb;
}

.secondary-button {
  color: #174ea6;
  background: #e8f0ff;
}

.info-list,
.task-list,
.rule-list,
.diagnostic-list {
  display: grid;
  gap: 10px;
}

.info-list div,
.task-row,
.rule-row,
.diagnostic-row,
.setting-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  border-top: 1px solid #edf1f6;
  padding: 14px 0;
}

.info-list span,
.task-row p,
.setting-row p,
.diagnostic-row p {
  margin: 0;
  color: #64748b;
}

.drop-zone {
  min-height: 220px;
  display: grid;
  place-items: center;
  align-content: center;
  text-align: center;
  border-style: dashed;
}

.panel-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 12px;
}

.rule-row em {
  color: #166534;
  font-style: normal;
}

.settings-panel {
  display: grid;
}

.setting-control input {
  width: 280px;
  height: 36px;
  border: 1px solid #cfd8e6;
  border-radius: 8px;
  padding: 0 10px;
}

.setting-control input[type="checkbox"] {
  width: 20px;
}

.diagnostic-row--ok strong {
  color: #166534;
}

.diagnostic-row--warning strong {
  color: #92400e;
}

.diagnostic-row--error strong {
  color: #991b1b;
}
```

- [ ] **Step 9: 构建验证**

Run: `npm run build`

Expected: build succeeds and UI pages compile.

- [ ] **Step 10: Commit**

```bash
git add Hi-Voicer/src/components/SettingRow.tsx Hi-Voicer/src/pages/HomePage.tsx Hi-Voicer/src/pages/TranscriptionPage.tsx Hi-Voicer/src/pages/HotwordsPage.tsx Hi-Voicer/src/pages/SettingsPage.tsx Hi-Voicer/src/pages/DiagnosticsPage.tsx Hi-Voicer/src/App.tsx Hi-Voicer/src/styles.css
git commit -m "feat: add Hi-Voicer foundation pages"
```

---

### Task 5: 添加前端 API 封装和设置持久化 fallback

**Files:**
- Create: `src/lib/api.ts`
- Create: `src/__tests__/settings.test.ts`
- Modify: `src/App.tsx`
- Modify: `package.json`

- [ ] **Step 1: 创建 API 封装**

`src/lib/api.ts`：

```ts
import { invoke } from "@tauri-apps/api/core";
import type { UserSettings } from "../types";

const STORAGE_KEY = "hi-voicer-settings";

export async function loadSettings(defaultSettings: UserSettings): Promise<UserSettings> {
  try {
    return await invoke<UserSettings>("load_settings");
  } catch {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return raw ? { ...defaultSettings, ...JSON.parse(raw) } : defaultSettings;
  }
}

export async function saveSettings(settings: UserSettings): Promise<UserSettings> {
  try {
    return await invoke<UserSettings>("save_settings", { settings });
  } catch {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
    return settings;
  }
}
```

- [ ] **Step 2: 修改 `src/App.tsx` 加载和保存设置**

```tsx
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

  return (
    <AppShell status={{ ...initialStatus, shortcut: settings.shortcut }} currentPage={currentPage} onPageChange={setCurrentPage}>
      {currentPage === "home" && <HomePage status={{ ...initialStatus, shortcut: settings.shortcut }} />}
      {currentPage === "transcription" && <TranscriptionPage tasks={initialTasks} />}
      {currentPage === "hotwords" && <HotwordsPage rules={initialHotwords} />}
      {currentPage === "settings" && <SettingsPage settings={settings} onSettingsChange={handleSettingsChange} />}
      {currentPage === "diagnostics" && <DiagnosticsPage items={initialDiagnostics} />}
    </AppShell>
  );
}
```

- [ ] **Step 3: 创建设置测试**

`src/__tests__/settings.test.ts`：

```ts
import { describe, expect, it, vi, beforeEach } from "vitest";
import { initialSettings } from "../data/mockState";
import { loadSettings, saveSettings } from "../lib/api";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.reject(new Error("not in tauri"))),
}));

describe("settings fallback storage", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("returns defaults when no saved settings exist", async () => {
    await expect(loadSettings(initialSettings)).resolves.toEqual(initialSettings);
  });

  it("saves and loads settings from localStorage outside Tauri", async () => {
    const next = { ...initialSettings, shortcut: "Mouse4", saveRecordings: true };
    await saveSettings(next);
    await expect(loadSettings(initialSettings)).resolves.toMatchObject(next);
  });
});
```

- [ ] **Step 4: 确保 Vitest 使用 jsdom**

Modify `package.json` devDependencies:

```json
"jsdom": "^25.0.1"
```

Modify `vite.config.ts`:

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  test: {
    environment: "jsdom",
    globals: true,
  },
});
```

- [ ] **Step 5: 安装新增测试依赖**

Run: `npm install`

Expected: `package-lock.json` includes `jsdom`.

- [ ] **Step 6: 运行测试和构建**

Run: `npm test`

Expected: 2 tests pass.

Run: `npm run build`

Expected: build succeeds.

- [ ] **Step 7: Commit**

```bash
git add Hi-Voicer/src/lib/api.ts Hi-Voicer/src/__tests__/settings.test.ts Hi-Voicer/src/App.tsx Hi-Voicer/package.json Hi-Voicer/package-lock.json Hi-Voicer/vite.config.ts
git commit -m "feat: persist Hi-Voicer settings"
```

---

### Task 6: 添加 Tauri/Rust 最小后端

**Files:**
- Create: `src-tauri/Cargo.toml`
- Create: `src-tauri/tauri.conf.json`
- Create: `src-tauri/src/main.rs`
- Create: `src-tauri/src/config.rs`
- Create: `src-tauri/src/app_state.rs`

- [ ] **Step 1: 创建 `src-tauri/Cargo.toml`**

```toml
[package]
name = "hi-voicer"
version = "0.1.0"
description = "Offline Chinese voice input and transcription desktop app"
authors = ["Hi-Voicer"]
edition = "2021"

[lib]
name = "hi_voicer_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tauri = { version = "2", features = [] }
tauri-plugin-shell = "2"
```

- [ ] **Step 2: 创建 `src-tauri/tauri.conf.json`**

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Hi-Voicer",
  "version": "0.1.0",
  "identifier": "com.local.hivoicer",
  "build": {
    "beforeDevCommand": "npm run dev",
    "devUrl": "http://localhost:1420",
    "beforeBuildCommand": "npm run build",
    "frontendDist": "../dist"
  },
  "app": {
    "windows": [
      {
        "title": "Hi-Voicer",
        "width": 1180,
        "height": 760,
        "minWidth": 1024,
        "minHeight": 720
      }
    ],
    "security": {
      "csp": null
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": []
  }
}
```

- [ ] **Step 3: 创建 Rust 设置类型**

`src-tauri/src/config.rs`：

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSettings {
    pub shortcut: String,
    pub model_dir: String,
    pub output_dir: String,
    pub paste_mode: String,
    pub save_recordings: bool,
    pub launch_at_startup: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            shortcut: "CapsLock".to_string(),
            model_dir: String::new(),
            output_dir: String::new(),
            paste_mode: "clipboard".to_string(),
            save_recordings: false,
            launch_at_startup: false,
        }
    }
}
```

- [ ] **Step 4: 创建 Rust 状态类型**

`src-tauri/src/app_state.rs`：

```rust
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub readiness: String,
    pub model_name: String,
    pub shortcut: String,
    pub microphone_name: String,
}

impl Default for AppSnapshot {
    fn default() -> Self {
        Self {
            readiness: "model-required".to_string(),
            model_name: "未配置模型".to_string(),
            shortcut: "CapsLock".to_string(),
            microphone_name: "默认麦克风".to_string(),
        }
    }
}
```

- [ ] **Step 5: 创建 Tauri 命令**

`src-tauri/src/main.rs`：

```rust
mod app_state;
mod config;

use app_state::AppSnapshot;
use config::UserSettings;
use std::sync::Mutex;
use tauri::State;

struct RuntimeState {
    settings: Mutex<UserSettings>,
}

#[tauri::command]
fn get_app_snapshot(state: State<'_, RuntimeState>) -> AppSnapshot {
    let settings = state.settings.lock().expect("settings mutex poisoned");
    AppSnapshot {
        shortcut: settings.shortcut.clone(),
        ..AppSnapshot::default()
    }
}

#[tauri::command]
fn load_settings(state: State<'_, RuntimeState>) -> UserSettings {
    state.settings.lock().expect("settings mutex poisoned").clone()
}

#[tauri::command]
fn save_settings(settings: UserSettings, state: State<'_, RuntimeState>) -> UserSettings {
    let mut stored = state.settings.lock().expect("settings mutex poisoned");
    *stored = settings.clone();
    settings
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(RuntimeState {
            settings: Mutex::new(UserSettings::default()),
        })
        .invoke_handler(tauri::generate_handler![get_app_snapshot, load_settings, save_settings])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    run();
}
```

- [ ] **Step 6: 运行 Rust 检查**

Run: `cargo check --manifest-path src-tauri/Cargo.toml`

Expected: Rust code compiles.

- [ ] **Step 7: 运行前端构建**

Run: `npm run build`

Expected: frontend build still succeeds.

- [ ] **Step 8: Commit**

```bash
git add Hi-Voicer/src-tauri/Cargo.toml Hi-Voicer/src-tauri/tauri.conf.json Hi-Voicer/src-tauri/src/main.rs Hi-Voicer/src-tauri/src/config.rs Hi-Voicer/src-tauri/src/app_state.rs
git commit -m "feat: add Hi-Voicer Tauri backend"
```

---

## 自查结果

规格覆盖情况：

1. 友好 UI：Task 3 和 Task 4 覆盖第一版静态 UI 和页面结构。
2. 启动速度：Task 6 建立 Tauri 后端和前端构建基础，后续桌面计划实现异步模型加载；本计划不加载模型，避免第一阶段阻塞。
3. 即开即用离线输入和文件转录：本计划只做可接入骨架，真实输入和转录在后续计划中实现。
4. 架构选型：本计划落实 Tauri + Rust + React 项目结构。
5. 不显示服务端：Task 6 使用 Tauri 命令，不开放 localhost 业务服务端。

本计划刻意不覆盖真实 ASR、录音、托盘和文件转录，因为这些是独立子系统，需要单独计划和验证。
