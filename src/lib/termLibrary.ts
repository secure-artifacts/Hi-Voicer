import { initialTermCategories } from "../data/mockState";
import type { HotwordRule, TermCategory } from "../types";

interface NormalizeCategoryOptions {
  allowPlainArray?: boolean;
}

type ImportedRule = Partial<HotwordRule> & {
  from?: unknown;
  replacement?: unknown;
  to?: unknown;
};

export function normalizeTermCategories(value: unknown, options: NormalizeCategoryOptions = {}): TermCategory[] {
  const allowPlainArray = options.allowPlainArray ?? true;
  const list =
    typeof value === "object" && value && Array.isArray((value as { categories?: unknown }).categories)
      ? (value as { categories: unknown[] }).categories
      : allowPlainArray && Array.isArray(value)
        ? value
        : [];
  const seenIds = new Set<string>();

  const categories = list
    .map((item, index) => {
      const category = item as Partial<TermCategory>;
      const rawId =
        typeof category.id === "string" && category.id.trim() ? category.id.trim() : `imported-category-${Date.now()}-${index}`;
      const id = seenIds.has(rawId) ? `${rawId}-${index}` : rawId;
      seenIds.add(id);
      return {
        id,
        name: typeof category.name === "string" && category.name.trim() ? category.name.trim() : `分类 ${index + 1}`,
        order: typeof category.order === "number" ? category.order : index,
      };
    })
    .filter((category) => category.name.trim());

  return categories.length > 0 ? categories : initialTermCategories;
}

export function normalizeHotwordRules(value: unknown, categories: TermCategory[] = initialTermCategories): HotwordRule[] {
  const list = Array.isArray(value)
    ? value
    : typeof value === "object" && value && Array.isArray((value as { rules?: unknown }).rules)
      ? (value as { rules: unknown[] }).rules
      : [];
  const fallbackCategoryId = categories[0]?.id ?? "replacements";
  const validCategoryIds = new Set(categories.map((category) => category.id));

  return list
    .map((item, index) => {
      const rule = item as ImportedRule;
      const source = typeof rule.source === "string" ? rule.source : typeof rule.from === "string" ? rule.from : "";
      const target =
        typeof rule.target === "string"
          ? rule.target
          : typeof rule.to === "string"
            ? rule.to
            : typeof rule.replacement === "string"
              ? rule.replacement
              : "";
      const categoryId =
        typeof rule.categoryId === "string" && validCategoryIds.has(rule.categoryId) ? rule.categoryId : fallbackCategoryId;

      return {
        id: typeof rule.id === "string" && rule.id.trim() ? rule.id : `hotword-${Date.now()}-${index}`,
        source,
        target,
        categoryId,
        enabled: typeof rule.enabled === "boolean" ? rule.enabled : true,
        hitCount: typeof rule.hitCount === "number" ? rule.hitCount : 0,
        lastUsedAt: typeof rule.lastUsedAt === "string" ? rule.lastUsedAt : undefined,
      };
    })
    .filter((rule) => rule.source.trim() || rule.target.trim());
}
