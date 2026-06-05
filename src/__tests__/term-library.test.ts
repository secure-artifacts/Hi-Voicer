import { describe, expect, it } from "vitest";
import { initialTermCategories } from "../data/mockState";
import { normalizeHotwordRules, normalizeTermCategories } from "../lib/termLibrary";

describe("term library normalization", () => {
  it("uses default categories when legacy rule arrays are not allowed as category input", () => {
    const categories = normalizeTermCategories([{ source: "legacy", target: "Legacy" }], { allowPlainArray: false });

    expect(categories[0].id).toBe(initialTermCategories[0].id);
  });

  it("deduplicates imported category ids", () => {
    const categories = normalizeTermCategories({
      categories: [
        { id: "clients", name: "Clients", order: 0 },
        { id: "clients", name: "Clients Copy", order: 1 },
      ],
    });

    expect(categories.map((category) => category.id)).toEqual(["clients", "clients-1"]);
  });

  it("normalizes legacy rule fields and missing categories", () => {
    const categories = normalizeTermCategories({ categories: [{ id: "valid", name: "Valid", order: 0 }] });
    const rules = normalizeHotwordRules(
      [{ from: "old term", replacement: "New Term", categoryId: "missing", enabled: false }],
      categories,
    );

    expect(rules).toEqual([
      expect.objectContaining({
        source: "old term",
        target: "New Term",
        categoryId: "valid",
        enabled: false,
      }),
    ]);
  });
});
