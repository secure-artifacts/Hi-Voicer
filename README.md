# Hi-Voicer

Hi-Voicer 是面向 Windows 的本地离线语音工作台。它把语音输入、音频/视频文件转写、字幕校对、术语替换和基础音频处理放在同一个桌面应用里完成；模型、录音、缓存和转写结果默认留在本机。

![Hi-Voicer 产品概览](docs/assets/hi-voicer-overview.svg)

## 适合谁

- 想用快捷键在任意输入框里说话并自动上屏的用户。
- 需要把会议、网课、录音、视频整理成文字或字幕的用户。
- 希望转写流程尽量离线、本地可控的用户。
- 需要批量处理音频、校正字幕片段、维护术语替换表的用户。

## 主要能力

- 语音输入：支持按住说话、连续识别、纯录音三种模式。
- 文件转写：支持音频和常见视频文件，导出纯文本、时间线文本和 SRT 字幕。
- 字幕编辑：校正文案、拆分/合并字幕、播放选中片段、导出选中片段音频。
- 术语库：把常见错词、专有名词和客户名统一替换。
- 音频处理：降噪、增强、格式转换、视频提取音频、波形剪辑、多段导出和音频合并。
- 本机诊断：检查模型、CPU 识别运行时、麦克风、系统声音和 ffmpeg 状态。

## 1.2.1 更新重点

- 新增 Qwen3-ASR 0.6B Q8 GGUF 高效转录模式，使用内置 llama.cpp CPU 服务。
- GGUF、SenseVoice、Qwen3 兼容版等模型目录可以整体复制给其他用户，配置会自动重绑定到实际路径。
- 发布包内置最小 Sherpa-ONNX CPU 运行时，复制已有模型后无需再次联网下载引擎。
- Qwen GGUF 使用 60 秒分片和常驻服务，空闲 5 分钟自动释放内存；异常时可回退已安装的兼容版。
- CUDA、cuDNN、TensorRT 和 Vulkan 运行文件继续排除在正式安装包之外。

详见：[Hi-Voicer 1.2.1 发布说明](docs/release-1.2.1.md)

## 下载与安装
优先从 [secure-artifacts/Hi-Voicer Releases](https://github.com/secure-artifacts/Hi-Voicer/releases) 下载可信构建；个人仓库 [cg202601/Hi-Voicer Releases](https://github.com/cg202601/Hi-Voicer/releases) 同步发布相同版本：

- 推荐普通用户使用 `Hi-Voicer_1.2.1_x64-setup.exe`
- 也可以下载同版本 MSI 安装包

首次配置模型时只需联网下载模型。Qwen3 高效版所需的 llama.cpp CPU 组件，以及 SenseVoice、Qwen3 兼容版等模型所需的最小 Sherpa-ONNX CPU 组件都随安装包提供。已安装的模型目录可整体复制到其他电脑并通过“选择已有模型目录”使用。配置完成后，录音识别、文件转写和基础音频处理在本地运行。正式发布包内置 `ffmpeg.exe` 和 `ffprobe.exe`，用于音频转码、字幕片段导出、双轨混音、波形生成和媒体信息探测；不内置 `ffplay.exe`。

## 当前模型策略

1.2.1 keeps CPU as the stable distribution path for ordinary Windows machines. DirectML remains available as a per-machine verified acceleration path, while CUDA runtime files are not bundled.

推荐模型：

- SenseVoiceSmall：默认推荐，适合中文语音输入和低延迟短音频转写。
- Qwen3-ASR 0.6B 高效版：文件转录默认推荐，Q8 GGUF 模型按需下载，运行组件随安装包提供。
- Qwen3-ASR 0.6B 兼容版：保留 Sherpa-ONNX INT8 路线，作为旧安装和自动回退方案。
- Sherpa FunASR-Nano：中文质量优先，下载体积更大。
- Faster-Whisper：外部专业引擎入口，适合需要成熟 NVIDIA GPU 路线的用户。
- Sherpa Paraformer：轻量备用模型，适合低配置电脑。

## 发布来源验证

正式安装包由 GitHub Actions 在 `v*` tag 推送后自动构建、生成 attestation 并上传到 Release。组织仓库是首选可信来源；不要使用来源不明的本地手工包。

```powershell
gh attestation verify .\Hi-Voicer_1.2.1_x64-setup.exe --repo secure-artifacts/Hi-Voicer
```

## 开发验证

```powershell
npm ci
npm run prepare:llama
npm run prepare:sherpa
npm run check:resources
npm test
npm run build
cargo test --manifest-path src-tauri\Cargo.toml
npm run tauri -- build
```

更多说明见：

- [模型说明](docs/模型说明.md)
- [环境准备](docs/环境准备.md)
- [0.2.1 打包测试清单](docs/0.2.1-打包测试清单.md)
