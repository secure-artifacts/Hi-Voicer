import { Download, FolderPlus, Pencil, Plus, Trash2, Upload } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { initialTermCategories } from "../data/mockState";
import { saveTextFile } from "../lib/api";
import { normalizeHotwordRules, normalizeTermCategories } from "../lib/termLibrary";
import type { HotwordRule, TermCategory } from "../types";

function createRule(categoryId: string): HotwordRule {
  return {
    id: `rule-${Date.now()}-${Math.random().toString(16).slice(2)}`,
    source: "",
    target: "",
    categoryId,
    enabled: true,
    hitCount: 0,
  };
}

function createCategory(name: string, order: number): TermCategory {
  return {
    id: `category-${Date.now()}-${Math.random().toString(16).slice(2)}`,
    name,
    order,
  };
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

export function HotwordsPage({
  rules,
  categories = initialTermCategories,
  onRulesChange,
  onCategoriesChange,
}: {
  rules: HotwordRule[];
  categories?: TermCategory[];
  onRulesChange?: (rules: HotwordRule[]) => void;
  onCategoriesChange?: (categories: TermCategory[]) => void;
}) {
  const [items, setItems] = useState<HotwordRule[]>(rules);
  const [categoryItems, setCategoryItems] = useState<TermCategory[]>(normalizeTermCategories(categories));
  const [activeCategoryId, setActiveCategoryId] = useState(categoryItems[0]?.id ?? "replacements");
  const [message, setMessage] = useState("");
  const fileInputRef = useRef<HTMLInputElement>(null);
  const sortedCategories = useMemo(() => [...categoryItems].sort((left, right) => left.order - right.order), [categoryItems]);
  const activeCategory = sortedCategories.find((category) => category.id === activeCategoryId) ?? sortedCategories[0];
  const visibleRules =
    activeCategoryId === "all" ? items : items.filter((rule) => (rule.categoryId ?? "replacements") === activeCategoryId);

  useEffect(() => {
    setItems(rules);
  }, [rules]);

  useEffect(() => {
    const next = normalizeTermCategories(categories);
    setCategoryItems(next);
    setActiveCategoryId((current) => (next.some((category) => category.id === current) ? current : next[0]?.id ?? "replacements"));
  }, [categories]);

  function setRules(next: HotwordRule[] | ((current: HotwordRule[]) => HotwordRule[])) {
    setItems((current) => {
      const updated = typeof next === "function" ? next(current) : next;
      onRulesChange?.(updated);
      return updated;
    });
  }

  function setCategories(next: TermCategory[] | ((current: TermCategory[]) => TermCategory[])) {
    setCategoryItems((current) => {
      const updated = (typeof next === "function" ? next(current) : next).map((category, index) => ({
        ...category,
        order: index,
      }));
      onCategoriesChange?.(updated);
      return updated;
    });
  }

  function updateRule(id: string, patch: Partial<HotwordRule>) {
    setRules((current) => current.map((rule) => (rule.id === id ? { ...rule, ...patch } : rule)));
  }

  function handleAddRule() {
    const categoryId = activeCategoryId === "all" ? sortedCategories[0]?.id ?? "replacements" : activeCategoryId;
    setRules((current) => [createRule(categoryId), ...current]);
    setActiveCategoryId(categoryId);
    setMessage("已新增一条术语规则。");
  }

  function handleAddCategory() {
    const name = window.prompt("请输入新分类名称");
    if (!name?.trim()) {
      return;
    }

    const category = createCategory(name.trim(), categoryItems.length);
    setCategories((current) => [...current, category]);
    setActiveCategoryId(category.id);
    setMessage(`已新增分类：${category.name}`);
  }

  function handleRenameCategory(category: TermCategory) {
    const name = window.prompt("请输入分类名称", category.name);
    if (!name?.trim()) {
      return;
    }

    setCategories((current) => current.map((item) => (item.id === category.id ? { ...item, name: name.trim() } : item)));
    setMessage("分类名称已更新。");
  }

  function handleDeleteCategory(category: TermCategory) {
    if (categoryItems.length <= 1) {
      setMessage("至少保留一个术语分类。");
      return;
    }

    const fallback = sortedCategories.find((item) => item.id !== category.id);
    if (!fallback) {
      return;
    }

    setCategories((current) => current.filter((item) => item.id !== category.id));
    setRules((current) =>
      current.map((rule) => (rule.categoryId === category.id ? { ...rule, categoryId: fallback.id } : rule)),
    );
    setActiveCategoryId(fallback.id);
    setMessage(`已删除分类，原规则已移动到 ${fallback.name}。`);
  }

  async function handleExport() {
    const content = JSON.stringify({ version: 2, categories: categoryItems, rules: items }, null, 2);
    try {
      const path = await saveTextFile("hi-voicer-terms.json", content);
      setMessage(path ? `术语库已导出：${path}` : "已取消导出。");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "术语库导出失败。");
    }
  }

  async function handleImport(file: File | undefined) {
    if (!file) {
      return;
    }

    try {
      const parsed = JSON.parse(await readHotwordFileText(file));
      const importedCategories = normalizeTermCategories(parsed, { allowPlainArray: false });
      const importedRules = normalizeHotwordRules(parsed, importedCategories);
      setCategories(importedCategories);
      setRules(importedRules);
      setActiveCategoryId(importedCategories[0]?.id ?? "all");
      setMessage(`已导入 ${importedRules.length} 条术语规则。`);
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
          <p className="section-label">术语库</p>
          <h2>让识别结果按你的写法输出</h2>
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
          <button className="secondary-button" type="button" onClick={handleAddCategory}>
            <FolderPlus size={16} />
            新增分类
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

      <div className="term-library-layout">
        <aside className="term-category-list" aria-label="术语分类">
          <button
            className={activeCategoryId === "all" ? "term-category term-category--active" : "term-category"}
            type="button"
            onClick={() => setActiveCategoryId("all")}
          >
            <span>全部术语</span>
            <strong>{items.length}</strong>
          </button>
          {sortedCategories.map((category) => (
            <div className={activeCategoryId === category.id ? "term-category-wrap term-category-wrap--active" : "term-category-wrap"} key={category.id}>
              <button className="term-category" type="button" onClick={() => setActiveCategoryId(category.id)}>
                <span>{category.name}</span>
                <strong>{items.filter((rule) => (rule.categoryId ?? "replacements") === category.id).length}</strong>
              </button>
              <button className="icon-button" type="button" title="重命名分类" onClick={() => handleRenameCategory(category)}>
                <Pencil size={15} />
              </button>
              <button className="icon-button" type="button" title="删除分类" onClick={() => handleDeleteCategory(category)}>
                <Trash2 size={15} />
              </button>
            </div>
          ))}
        </aside>

        <div className="rule-list rule-list--editable">
          {visibleRules.length === 0 ? (
            <p className="empty-state">{activeCategory?.name ?? "当前分类"} 还没有术语规则。</p>
          ) : (
            visibleRules.map((rule) => (
              <div className="rule-row rule-row--editable rule-row--terms" key={rule.id}>
                <label>
                  <span>识别成</span>
                  <input value={rule.source} onChange={(event) => updateRule(rule.id, { source: event.target.value })} />
                </label>
                <label>
                  <span>替换为</span>
                  <input value={rule.target} onChange={(event) => updateRule(rule.id, { target: event.target.value })} />
                </label>
                <label>
                  <span>分类</span>
                  <select
                    value={rule.categoryId ?? sortedCategories[0]?.id ?? ""}
                    onChange={(event) => updateRule(rule.id, { categoryId: event.target.value })}
                  >
                    {sortedCategories.map((category) => (
                      <option key={category.id} value={category.id}>
                        {category.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="inline-toggle">
                  <input
                    type="checkbox"
                    checked={rule.enabled}
                    onChange={(event) => updateRule(rule.id, { enabled: event.target.checked })}
                  />
                  启用
                </label>
                <span className="term-hit-count">命中 {rule.hitCount ?? 0}</span>
                <button
                  aria-label="删除规则"
                  className="icon-button"
                  type="button"
                  onClick={() => setRules((current) => current.filter((item) => item.id !== rule.id))}
                >
                  <Trash2 size={16} />
                </button>
              </div>
            ))
          )}
        </div>
      </div>

      {message && <p className="model-message">{message}</p>}
    </section>
  );
}
