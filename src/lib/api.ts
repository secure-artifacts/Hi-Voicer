import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { open as openUrl } from "@tauri-apps/plugin-shell";
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

export async function selectDirectory(): Promise<string | null> {
  try {
    const selected = await openDialog({
      directory: true,
      multiple: false,
    });
    return typeof selected === "string" ? selected : null;
  } catch {
    const typed = window.prompt("请输入已解压模型文件夹路径");
    return typed?.trim() || null;
  }
}

export async function openExternalUrl(url: string): Promise<void> {
  try {
    await openUrl(url);
  } catch {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}
