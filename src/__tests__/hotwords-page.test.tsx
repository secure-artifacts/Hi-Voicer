import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
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
    render(<HotwordsPage rules={[]} />);

    fireEvent.click(screen.getAllByRole("button")[2]);

    expect(screen.getAllByRole("textbox")).toHaveLength(2);
  });

  it("imports rules from json", async () => {
    render(<HotwordsPage rules={[]} />);

    const file = new File([JSON.stringify({ rules: [{ source: "tauri_term", target: "Tauri", enabled: true }] })], "rules.json", {
      type: "application/json",
    });
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, { target: { files: [file] } });

    expect(await screen.findByDisplayValue("tauri_term")).toBeTruthy();
    expect(await screen.findByDisplayValue("Tauri")).toBeTruthy();
  });

  it("exports current rules as json", async () => {
    render(<HotwordsPage rules={[{ id: "rule-1", source: "tauri_term", target: "Tauri", enabled: true }]} />);

    fireEvent.click(screen.getAllByRole("button")[1]);

    expect(await screen.findByText(/C:\\exports\\hi-voicer-hotwords.json/)).toBeTruthy();
  });
});
