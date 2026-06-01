import { Download, Plus, Trash2, Upload } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { saveTextFile } from "../lib/api";
import type { HotwordRule } from "../types";

const HOTWORDS_STORAGE_KEY = "hi-voicer-hotwords";

function createRule(): HotwordRule {
  return {
    id: `rule-${Date.now()}-${Math.random().toString(16).slice(2)}`,
    source: "",
    target: "",
    enabled: true,
  };
}

function normalizeImportedRules(value: unknown): HotwordRule[] {
  const list = Array.isArray(value)
    ? value
    : typeof value === "object" && value && Array.isArray((value as { rules?: unknown }).rules)
      ? (value as { rules: unknown[] }).rules
      : [];

  return list
    .map((item, index) => {
      const rule = item as Partial<HotwordRule>;
      return {
        id: typeof rule.id === "string" ? rule.id : `imported-${Date.now()}-${index}`,
        source: typeof rule.source === "string" ? rule.source : "",
        target: typeof rule.target === "string" ? rule.target : "",
        enabled: typeof rule.enabled === "boolean" ? rule.enabled : true,
      };
    })
    .filter((rule) => rule.source.trim() || rule.target.trim());
}

function readHotwordFileText(file: File): Promise<string> {
  if (typeof file.text === "function") {
    return file.text();
  }

  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(reader.error ?? new Error("Failed to read file"));
    reader.readAsText(file);
  });
}

export function HotwordsPage({ rules }: { rules: HotwordRule[] }) {
  const [items, setItems] = useState<HotwordRule[]>(rules);
  const [message, setMessage] = useState("");
  const fileInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const raw = window.localStorage.getItem(HOTWORDS_STORAGE_KEY);
    if (raw) {
      try {
        const imported = normalizeImportedRules(JSON.parse(raw));
        if (imported.length > 0) {
          setItems(imported);
        }
      } catch {
        window.localStorage.removeItem(HOTWORDS_STORAGE_KEY);
      }
    }
  }, []);

  useEffect(() => {
    window.localStorage.setItem(HOTWORDS_STORAGE_KEY, JSON.stringify(items));
  }, [items]);

  function updateRule(id: string, patch: Partial<HotwordRule>) {
    setItems((current) => current.map((rule) => (rule.id === id ? { ...rule, ...patch } : rule)));
  }

  function handleAddRule() {
    setItems((current) => [createRule(), ...current]);
    setMessage("已新增一条规则。");
  }

  async function handleExport() {
    const content = JSON.stringify({ version: 1, rules: items }, null, 2);
    try {
      const path = await saveTextFile("hi-voicer-hotwords.json", content);
      setMessage(path ? `热词配置已导出：${path}` : "已取消导出。");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "热词导出失败。");
    }
  }

  async function handleImport(file: File | undefined) {
    if (!file) {
      return;
    }

    try {
      const imported = normalizeImportedRules(JSON.parse(await readHotwordFileText(file)));
      setItems(imported);
      setMessage(`已导入 ${imported.length} 条规则。`);
    } catch {
      setMessage("导入失败，请选择 JSON 配置文件。");
    } finally {
      if (fileInputRef.current) {
        fileInputRef.current.value = "";
      }
    }
  }

  return (
    <section className="panel hotwords-panel">
      <div className="panel-heading">
        <div>
          <p className="section-label">热词和替换</p>
          <h2>让识别结果更像你的用语</h2>
        </div>
        <div className="button-group">
          <button className="secondary-button" type="button" onClick={() => fileInputRef.current?.click()}>
            <Upload size={16} />
            导入
          </button>
          <button className="secondary-button" type="button" onClick={() => void handleExport()}>
            <Download size={16} />
            导出
          </button>
          <button className="primary-button" type="button" onClick={handleAddRule}>
            <Plus size={16} />
            新增规则
          </button>
        </div>
      </div>

      <input
        ref={fileInputRef}
        hidden
        type="file"
        accept="application/json,.json"
        onChange={(event) => void handleImport(event.target.files?.[0])}
      />

      <div className="rule-list rule-list--editable">
        {items.map((rule) => (
          <div className="rule-row rule-row--editable" key={rule.id}>
            <label>
              <span>识别成</span>
              <input value={rule.source} onChange={(event) => updateRule(rule.id, { source: event.target.value })} />
            </label>
            <label>
              <span>替换为</span>
              <input value={rule.target} onChange={(event) => updateRule(rule.id, { target: event.target.value })} />
            </label>
            <label className="inline-toggle">
              <input
                type="checkbox"
                checked={rule.enabled}
                onChange={(event) => updateRule(rule.id, { enabled: event.target.checked })}
              />
              启用
            </label>
            <button
              aria-label="删除规则"
              className="icon-button"
              type="button"
              onClick={() => setItems((current) => current.filter((item) => item.id !== rule.id))}
            >
              <Trash2 size={16} />
            </button>
          </div>
        ))}
      </div>

      {message && <p className="model-message">{message}</p>}
    </section>
  );
}
