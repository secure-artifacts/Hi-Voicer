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
