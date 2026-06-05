import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { exportAudioSegment, prepareAudioPreview, saveTextFile, selectDirectory } from "../lib/api";
import { SubtitleEditorPage } from "../pages/SubtitleEditorPage";
import type { SubtitleSegment, TermCategory } from "../types";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn((path: string) => `asset://local/${path}`),
  isTauri: vi.fn(() => true),
}));

vi.mock("../lib/api", () => ({
  exportAudioSegment: vi.fn(() => Promise.resolve("C:\\exports\\segment.wav")),
  prepareAudioPreview: vi.fn(() => new Promise<string>(() => {})),
  saveTextFile: vi.fn(() => Promise.resolve("C:\\exports\\subtitle.srt")),
  selectDirectory: vi.fn(() => Promise.resolve("D:\\clips")),
}));

const categories: TermCategory[] = [{ id: "technical", name: "技术词", order: 0 }];
const segments: SubtitleSegment[] = [
  {
    id: "segment-1",
    index: 1,
    start: 0,
    end: 2,
    text: "陶瑞应用",
    sourceAudioPath: "C:\\HiVoicer\\review.wav",
  },
  {
    id: "segment-2",
    index: 2,
    start: 2,
    end: 4,
    text: "第二句字幕",
    sourceAudioPath: "C:\\HiVoicer\\review.wav",
  },
];

describe("SubtitleEditorPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Object.defineProperty(HTMLMediaElement.prototype, "play", {
      configurable: true,
      value: vi.fn(() => Promise.resolve()),
    });
  });

  it("uses the Tauri asset protocol for cached local audio playback", async () => {
    vi.mocked(prepareAudioPreview).mockResolvedValueOnce("C:\\cache\\review-preview.wav");
    render(
      <SubtitleEditorPage
        project={{
          fileName: "demo.wav",
          sourceAudioPath: "C:\\HiVoicer\\review.wav",
          text: "陶瑞应用",
          segments,
        }}
        termCategories={categories}
        onAddTermRule={vi.fn()}
        onProjectChange={vi.fn()}
      />,
    );

    const audio = document.querySelector("audio") as HTMLAudioElement;
    await waitFor(() => expect(prepareAudioPreview).toHaveBeenCalledWith("C:\\HiVoicer\\review.wav"));
    await waitFor(() => expect(audio.src).toContain("asset://local/C:\\cache\\review-preview.wav"));
  });

  it("starts estimated timeline playback before the selected segment boundary", () => {
    render(
      <SubtitleEditorPage
        project={{
          fileName: "demo.wav",
          sourceAudioPath: "C:\\HiVoicer\\review.wav",
          text: "review line",
          segments,
          timelineKind: "estimated",
        }}
        termCategories={categories}
        onAddTermRule={vi.fn()}
        onProjectChange={vi.fn()}
      />,
    );

    const audio = document.querySelector("audio") as HTMLAudioElement;
    const rows = document.querySelectorAll<HTMLButtonElement>(".subtitle-row");
    fireEvent.click(rows[1]);

    expect(audio.currentTime).toBeCloseTo(1.2);
  });

  it("offers a confirmed terminology suggestion after subtitle text edits", () => {
    const onAddTermRule = vi.fn();
    render(
      <SubtitleEditorPage
        project={{
          fileName: "demo.wav",
          sourceAudioPath: "C:\\HiVoicer\\review.wav",
          text: "陶瑞应用",
          segments,
        }}
        termCategories={categories}
        onAddTermRule={onAddTermRule}
        onProjectChange={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByDisplayValue("陶瑞应用"), { target: { value: "Tauri 应用" } });
    fireEvent.click(screen.getByRole("button", { name: "加入术语库" }));

    expect(onAddTermRule).toHaveBeenCalledWith(
      expect.objectContaining({
        source: "陶瑞应用",
        target: "Tauri 应用",
        categoryId: "technical",
        enabled: true,
      }),
    );
  });

  it("disables selected audio export when the project has no source audio path", () => {
    render(
      <SubtitleEditorPage
        project={{
          fileName: "demo.wav",
          sourceAudioPath: "",
          text: "review line",
          segments: [{ ...segments[0], text: "review line", sourceAudioPath: "" }],
        }}
        termCategories={categories}
        onAddTermRule={vi.fn()}
        onProjectChange={vi.fn()}
      />,
    );

    const exportAudioButton = document.querySelector(".subtitle-toolbar .primary-button") as HTMLButtonElement;
    expect(exportAudioButton.disabled).toBe(true);
  });

  it("exports multiple selected subtitle clips with stable suggested names", async () => {
    render(
      <SubtitleEditorPage
        project={{
          fileName: "demo.wav",
          sourceAudioPath: "C:\\HiVoicer\\review.wav",
          text: "陶瑞应用\n第二句字幕",
          segments,
        }}
        termCategories={categories}
        onAddTermRule={vi.fn()}
        onProjectChange={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByLabelText("选择第 1 条字幕"));
    fireEvent.click(screen.getByLabelText("选择第 2 条字幕"));
    fireEvent.click(screen.getByRole("button", { name: "导出选中片段" }));

    expect(await screen.findByText(/已导出 2 段音频/)).toBeTruthy();
    expect(exportAudioSegment).toHaveBeenNthCalledWith(
      1,
      "C:\\HiVoicer\\review.wav",
      0,
      2,
      expect.objectContaining({ suggestedName: "demo-segment-001.wav" }),
    );
    expect(exportAudioSegment).toHaveBeenNthCalledWith(
      2,
      "C:\\HiVoicer\\review.wav",
      2,
      4,
      expect.objectContaining({ suggestedName: "demo-segment-002.wav" }),
    );
  });

  it("adds guard time when exporting estimated subtitle clips", async () => {
    render(
      <SubtitleEditorPage
        project={{
          fileName: "demo.wav",
          sourceAudioPath: "C:\\HiVoicer\\review.wav",
          text: "first line\nsecond line",
          segments,
          timelineKind: "estimated",
        }}
        termCategories={categories}
        onAddTermRule={vi.fn()}
        onProjectChange={vi.fn()}
      />,
    );

    const checkboxes = document.querySelectorAll<HTMLInputElement>(".subtitle-clip-check input");
    const exportAudioButton = document.querySelector(".subtitle-toolbar .primary-button") as HTMLButtonElement;
    fireEvent.click(checkboxes[1]);
    fireEvent.click(exportAudioButton);

    await waitFor(() => expect(exportAudioSegment).toHaveBeenCalled());
    expect(exportAudioSegment).toHaveBeenCalledWith(
      "C:\\HiVoicer\\review.wav",
      1.2,
      4.3,
      expect.objectContaining({ suggestedName: "demo-segment-002.wav" }),
    );
  });

  it("uses a custom folder for selected subtitle clip exports when configured", async () => {
    render(
      <SubtitleEditorPage
        project={{
          fileName: "demo.wav",
          sourceAudioPath: "C:\\HiVoicer\\review.wav",
          text: "陶瑞应用",
          segments,
        }}
        termCategories={categories}
        onAddTermRule={vi.fn()}
        onProjectChange={vi.fn()}
      />,
    );

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "自定义片段目录" }));
    });
    expect((await screen.findAllByText((content) => content.includes("D:\\clips"))).length).toBeGreaterThan(0);
    fireEvent.click(screen.getByLabelText("选择第 1 条字幕"));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "导出选中片段" }));
    });

    expect(selectDirectory).toHaveBeenCalled();
    expect(exportAudioSegment).toHaveBeenCalledWith(
      "C:\\HiVoicer\\review.wav",
      0,
      2,
      expect.objectContaining({ destinationDir: "D:\\clips" }),
    );
  });

  it("shows text export failures instead of leaving the action silent", async () => {
    vi.mocked(saveTextFile).mockRejectedValueOnce(new Error("disk full"));
    render(
      <SubtitleEditorPage
        project={{
          fileName: "demo.wav",
          sourceAudioPath: "C:\\HiVoicer\\review.wav",
          text: "review line",
          segments: [{ ...segments[0], text: "review line" }],
        }}
        termCategories={categories}
        onAddTermRule={vi.fn()}
        onProjectChange={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /SRT/ }));

    expect(await screen.findByText("disk full")).toBeTruthy();
  });
});
