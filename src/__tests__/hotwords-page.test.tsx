import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { initialTermCategories } from "../data/mockState";
import { HotwordsPage } from "../pages/HotwordsPage";

vi.mock("../lib/api", () => ({
  saveTextFile: vi.fn(() => Promise.resolve("C:\\exports\\hi-voicer-hotwords.json")),
}));

describe("HotwordsPage", () => {
  const storage = new Map<string, string>();

  beforeEach(() => {
    storage.clear();
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: vi.fn(() => "blob:hotwords"),
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      value: vi.fn(),
    });
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      value: {
        clear: () => storage.clear(),
        getItem: (key: string) => storage.get(key) ?? null,
        removeItem: (key: string) => storage.delete(key),
        setItem: (key: string, value: string) => storage.set(key, value),
      },
    });
  });

  it("adds an editable rule", () => {
    const onRulesChange = vi.fn();
    render(<HotwordsPage rules={[]} onRulesChange={onRulesChange} />);

    fireEvent.click(screen.getAllByRole("button")[2]);

    expect(screen.getAllByRole("textbox")).toHaveLength(2);
    expect(onRulesChange).toHaveBeenCalledWith([expect.objectContaining({ enabled: true })]);
  });

  it("imports rules from json", async () => {
    const onRulesChange = vi.fn();
    render(<HotwordsPage rules={[]} onRulesChange={onRulesChange} />);

    const file = new File([JSON.stringify({ rules: [{ source: "tauri_term", target: "Tauri", enabled: true }] })], "rules.json", {
      type: "application/json",
    });
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, { target: { files: [file] } });

    expect(await screen.findByDisplayValue("tauri_term")).toBeTruthy();
    expect(await screen.findByDisplayValue("Tauri")).toBeTruthy();
    expect(onRulesChange).toHaveBeenCalledWith([
      expect.objectContaining({ source: "tauri_term", target: "Tauri", enabled: true }),
    ]);
  });

  it("imports legacy rule arrays without treating them as categories", async () => {
    const onCategoriesChange = vi.fn();
    const onRulesChange = vi.fn();
    render(<HotwordsPage rules={[]} onRulesChange={onRulesChange} onCategoriesChange={onCategoriesChange} />);

    const file = new File([JSON.stringify([{ from: "legacy_term", replacement: "Legacy Term", enabled: false }])], "legacy.json", {
      type: "application/json",
    });
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, { target: { files: [file] } });

    expect(await screen.findByDisplayValue("legacy_term")).toBeTruthy();
    expect(await screen.findByDisplayValue("Legacy Term")).toBeTruthy();
    expect(onCategoriesChange).toHaveBeenCalledWith(
      expect.arrayContaining([expect.objectContaining({ id: initialTermCategories[0].id })]),
    );
    expect(onRulesChange).toHaveBeenCalledWith([
      expect.objectContaining({
        source: "legacy_term",
        target: "Legacy Term",
        categoryId: initialTermCategories[0].id,
        enabled: false,
      }),
    ]);
  });

  it("exports current rules as json", async () => {
    render(<HotwordsPage rules={[{ id: "rule-1", source: "tauri_term", target: "Tauri", enabled: true }]} />);

    fireEvent.click(screen.getAllByRole("button")[1]);

    expect(await screen.findByText(/C:\\exports\\hi-voicer-hotwords.json/)).toBeTruthy();
  });

  it("imports editable categories with rules", async () => {
    const onCategoriesChange = vi.fn();
    const onRulesChange = vi.fn();
    render(<HotwordsPage rules={[]} onRulesChange={onRulesChange} onCategoriesChange={onCategoriesChange} />);

    const file = new File(
      [
        JSON.stringify({
          categories: [{ id: "clients", name: "客户", order: 0 }],
          rules: [{ source: "acme", target: "ACME", categoryId: "clients", enabled: true }],
        }),
      ],
      "terms.json",
      { type: "application/json" },
    );
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, { target: { files: [file] } });

    expect((await screen.findAllByText("客户")).length).toBeGreaterThan(0);
    expect(await screen.findByDisplayValue("acme")).toBeTruthy();
    expect(onCategoriesChange).toHaveBeenCalledWith([expect.objectContaining({ id: "clients", name: "客户" })]);
    expect(onRulesChange).toHaveBeenCalledWith([expect.objectContaining({ categoryId: "clients", target: "ACME" })]);
  });

  it("moves imported rules with missing categories into the first available category", async () => {
    const onRulesChange = vi.fn();
    render(<HotwordsPage rules={[]} onRulesChange={onRulesChange} />);

    const file = new File(
      [
        JSON.stringify({
          categories: [{ id: "valid", name: "Valid", order: 0 }],
          rules: [{ source: "orphan", target: "Orphan", categoryId: "missing", enabled: true }],
        }),
      ],
      "terms.json",
      { type: "application/json" },
    );
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, { target: { files: [file] } });

    expect(await screen.findByDisplayValue("orphan")).toBeTruthy();
    expect(onRulesChange).toHaveBeenCalledWith([expect.objectContaining({ categoryId: "valid", target: "Orphan" })]);
  });
});
