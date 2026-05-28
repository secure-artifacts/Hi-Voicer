import type { ReactNode } from "react";
import { Activity, FileAudio, Home, ListChecks, Settings, Wrench } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { AppPage, AppStatus } from "../types";
import { StatusBadge } from "./StatusBadge";

const pages: Array<{ id: AppPage; label: string; icon: LucideIcon }> = [
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
  children: ReactNode;
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
