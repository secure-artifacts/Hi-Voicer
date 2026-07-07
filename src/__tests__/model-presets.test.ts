import { describe, expect, it } from "vitest";
import { modelPresets } from "../data/modelPresets";

describe("model presets", () => {
  it("keeps one-click Sherpa presets installable from Hugging Face", () => {
    const installable = modelPresets.filter((model) => model.installKind === "sherpaOnnx");

    expect(installable.map((model) => model.id)).toEqual(
      expect.arrayContaining([
        "sensevoice-small",
        "qwen3-asr-0.6b",
        "sherpa-funasr-nano",
      ]),
    );

    for (const model of installable) {
      expect(model.downloadUrl).toContain("huggingface.co/");
      expect(model.modelFiles?.length).toBeGreaterThan(0);
      expect(model.sherpaArgs).toContain("{modelDir}");

      for (const modelFile of model.modelFiles ?? []) {
        expect(modelFile.url).toContain("huggingface.co/");
        expect(modelFile.url).toContain("/resolve/main/");
        expect(modelFile.path).not.toContain("..");
        expect(modelFile.path).not.toMatch(/^[a-zA-Z]:/);
      }
    }
  });

  it("uses measured Qwen3-ASR defaults", () => {
    const qwen = modelPresets.find((model) => model.id === "qwen3-asr-0.6b");

    expect(qwen?.sherpaArgs).toContain("--num-threads=6");
    expect(qwen?.sherpaArgs).toContain("--qwen3-asr-max-new-tokens=128");
  });
  it("does not expose superseded Whisper Base or Zipformer presets", () => {
    expect(modelPresets.map((model) => model.id)).not.toContain("whisper-base");
    expect(modelPresets.map((model) => model.id)).not.toContain("sherpa-zipformer-zh");
  });
  it("exposes Faster-Whisper only as an external engine", () => {
    const fasterWhisper = modelPresets.find((model) => model.id === "faster-whisper");

    expect(fasterWhisper?.installKind).toBe("engineRequired");
    expect(fasterWhisper?.downloadUrl).toContain("github.com/SYSTRAN/faster-whisper");
    expect(fasterWhisper?.modelFiles).toBeUndefined();
  });

  it("does not expose Qwen3-ASR 1.7B until a supported local engine exists", () => {
    expect(modelPresets.map((model) => model.id)).not.toContain("qwen3-asr-1.7b");
  });
});