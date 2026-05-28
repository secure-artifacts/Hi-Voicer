import type { ModelPreset } from "../types";

export const modelPresets: ModelPreset[] = [
  {
    id: "vosk-small-cn-0.22",
    name: "中文轻量模型",
    size: "42 MB",
    quality: "启动快，占用低",
    memory: "约 300 MB 内存",
    recommendedFor: "日常输入、低配置电脑、先体验",
    license: "Apache 2.0",
    downloadUrl: "https://alphacephei.com/vosk/models/vosk-model-small-cn-0.22.zip",
  },
  {
    id: "vosk-cn-0.22",
    name: "中文高精度模型",
    size: "1.3 GB",
    quality: "准确率更高",
    memory: "建议 8 GB 以上内存",
    recommendedFor: "长文本转写、会议录音、较新电脑",
    license: "Apache 2.0",
    downloadUrl: "https://alphacephei.com/vosk/models/vosk-model-cn-0.22.zip",
  },
  {
    id: "vosk-cn-kaldi-multicn-0.15",
    name: "中文兼容模型",
    size: "1.5 GB",
    quality: "旧版宽带模型",
    memory: "建议 8 GB 以上内存",
    recommendedFor: "高精度模型效果不理想时备用",
    license: "Apache 2.0",
    downloadUrl: "https://alphacephei.com/vosk/models/vosk-model-cn-kaldi-multicn-0.15.zip",
  },
];

export function findModelPreset(modelId: string) {
  return modelPresets.find((model) => model.id === modelId);
}
