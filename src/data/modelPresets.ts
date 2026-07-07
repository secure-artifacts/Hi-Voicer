import type { ModelPreset } from "../types";

const hf = (repo: string, file: string) => `https://huggingface.co/${repo}/resolve/main/${file}`;

export const modelPresets: ModelPreset[] = [
  {
    id: "sensevoice-small",
    name: "SenseVoiceSmall 中文优先",
    family: "funasr",
    installKind: "sherpaOnnx",
    size: "约 240 MB",
    quality: "中文、粤语、英文都稳，速度快",
    memory: "CPU 可跑",
    recommendedFor: "默认推荐，语音输入和低延迟文件转录",
    license: "SenseVoice 模型许可",
    downloadUrl: "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
    engineNote: "使用 Sherpa-ONNX 的 SenseVoiceSmall ONNX 版本，安装后自动写入本地配置。",
    modelFiles: [
      {
        path: "model.int8.onnx",
        url: hf("csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17", "model.int8.onnx"),
      },
      {
        path: "tokens.txt",
        url: hf("csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17", "tokens.txt"),
      },
    ],
    sherpaArgs:
      '--tokens="{modelDir}\\tokens.txt" --sense-voice-model="{modelDir}\\model.int8.onnx" --sense-voice-use-itn=1 --num-threads=4',
  },
  {
    id: "qwen3-asr-0.6b",
    name: "Qwen3-ASR 0.6B",
    family: "qwen",
    installKind: "sherpaOnnx",
    size: "约 1 GB",
    quality: "新一代多语种 ASR，质量更高",
    memory: "建议 8 GB 以上内存",
    recommendedFor: "质量优先，能接受更大下载体积",
    license: "Apache 2.0",
    downloadUrl: "https://huggingface.co/pantinor/sherpa-onnx-qwen3-asr-0.6b-int8",
    engineNote: "Sherpa-ONNX 兼容的 Qwen3-ASR 0.6B INT8 包，需要 conv frontend、encoder、decoder 和 tokenizer。",
    modelFiles: [
      { path: "conv_frontend.onnx", url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "conv_frontend.onnx") },
      { path: "encoder.int8.onnx", url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "encoder.int8.onnx") },
      { path: "decoder.int8.onnx", url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "decoder.int8.onnx") },
      { path: "tokenizer/merges.txt", url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "tokenizer/merges.txt") },
      {
        path: "tokenizer/tokenizer_config.json",
        url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "tokenizer/tokenizer_config.json"),
      },
      { path: "tokenizer/vocab.json", url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "tokenizer/vocab.json") },
    ],
    sherpaArgs:
      '--qwen3-asr-conv-frontend="{modelDir}\\conv_frontend.onnx" --qwen3-asr-encoder="{modelDir}\\encoder.int8.onnx" --qwen3-asr-decoder="{modelDir}\\decoder.int8.onnx" --qwen3-asr-tokenizer="{modelDir}\\tokenizer" --feat-dim=128 --num-threads=6 --qwen3-asr-max-new-tokens=128',
  },  {
    id: "faster-whisper",
    name: "Faster-Whisper 高级引擎",
    family: "whisper",
    installKind: "engineRequired",
    size: "可选引擎包",
    quality: "长文件稳定，NVIDIA GPU 路线成熟",
    memory: "CPU 可跑；GPU 加速推荐 NVIDIA 显卡",
    recommendedFor: "长视频字幕、稳定文件转录，作为可选专业引擎",
    license: "MIT / model license varies",
    downloadUrl: "https://github.com/SYSTRAN/faster-whisper",
    engineNote:
      "Faster-Whisper 需要单独配置 worker 和模型目录。PoC 期请准备包含 engine.json 的本地目录，engine 设为 faster-whisper，executable 指向 worker。",
  },

  {
    id: "sherpa-funasr-nano",
    name: "Sherpa FunASR-Nano",
    family: "sherpa",
    installKind: "sherpaOnnx",
    size: "约 1 GB",
    quality: "中文质量优先",
    memory: "建议 4 GB 以上内存",
    recommendedFor: "中文输入，较高准确率，下载时间更长",
    license: "Apache 2.0",
    downloadUrl: "https://huggingface.co/csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30",
    engineNote: "安装后自动配置 Sherpa-ONNX 运行时和模型文件。",
    modelFiles: [
      { path: "embedding.int8.onnx", url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "embedding.int8.onnx") },
      {
        path: "encoder_adaptor.int8.onnx",
        url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "encoder_adaptor.int8.onnx"),
      },
      { path: "llm.int8.onnx", url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "llm.int8.onnx") },
      { path: "Qwen3-0.6B/merges.txt", url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "Qwen3-0.6B/merges.txt") },
      {
        path: "Qwen3-0.6B/tokenizer.json",
        url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "Qwen3-0.6B/tokenizer.json"),
      },
      { path: "Qwen3-0.6B/vocab.json", url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "Qwen3-0.6B/vocab.json") },
    ],
    sherpaArgs:
      '--funasr-nano-encoder-adaptor="{modelDir}\\encoder_adaptor.int8.onnx" --funasr-nano-llm="{modelDir}\\llm.int8.onnx" --funasr-nano-embedding="{modelDir}\\embedding.int8.onnx" --funasr-nano-tokenizer="{modelDir}\\Qwen3-0.6B" --num-threads=3',
  },

  {
    id: "sherpa-paraformer-zh",
    name: "Sherpa Paraformer 中文",
    family: "sherpa",
    installKind: "sherpaOnnx",
    size: "约 250 MB",
    quality: "首装快，中文可用",
    memory: "CPU 可跑",
    recommendedFor: "低配置电脑，快速跑通全流程",
    license: "Apache 2.0",
    downloadUrl: "https://huggingface.co/csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14",
    engineNote: "轻量中文模型，适合快速验证和低配置机器。",
    modelFiles: [
      { path: "model.int8.onnx", url: hf("csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14", "model.int8.onnx") },
      { path: "tokens.txt", url: hf("csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14", "tokens.txt") },
    ],
    sherpaArgs:
      '--paraformer="{modelDir}\\model.int8.onnx" --tokens="{modelDir}\\tokens.txt" --num-threads=4 --decoding-method=greedy_search --model-type=paraformer',
  },

];

export function findModelPreset(modelId: string) {
  return modelPresets.find((model) => model.id === modelId);
}
