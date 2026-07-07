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

## 1.1.0 更新重点

- DirectML SenseVoice 加速路径已接入真实语音输入和文件转写。
- 诊断页新增 CPU vs DirectML benchmark，用同一音频比较速度、fallback 和文本相似度。
- benchmark 达标后会在本机设置中标记 `DirectML（已验证）`；换电脑仍需重新验证。
- CUDA product entry points have been removed. CPU remains the default stable path; DirectML is available as a per-machine verified acceleration path.
- 发布包继续保持离线优先：模型/运行时准备好后，录音识别和文件转写不依赖外网。

详见：[Hi-Voicer 1.1.0 发布说明](docs/release-1.1.0.md)

## 下载与安装
从 [cg202601/Hi-Voicer Releases](https://github.com/cg202601/Hi-Voicer/releases) 下载最新版本：

- 推荐普通用户使用 `Hi-Voicer_1.1.0_x64-setup.exe`
- 也可以下载同版本 MSI 安装包

首次配置模型时需要联网下载模型和 Sherpa-ONNX CPU 运行时。配置完成后，录音识别和文件转写在本地运行。音频转码、字幕片段导出、双轨混音和音频处理需要本机已有 `ffmpeg.exe`，或把它放到应用数据目录/程序目录的 `engines\ffmpeg` 下。

## 当前模型策略

1.1.0 keeps Sherpa-ONNX CPU as the default stable path for ordinary Windows machines. CUDA product entry points have been removed; DirectML is available as a per-machine acceleration path after the Diagnostics CPU vs DirectML benchmark passes.

推荐模型：

- SenseVoiceSmall：默认推荐，适合中文语音输入和低延迟短音频转写。
- Qwen3-ASR 0.6B：可一键配置，适合验证 Qwen3-ASR 路线。
- Sherpa FunASR-Nano：中文质量优先，下载体积更大。
- OpenAI Whisper Base：适合多语言文件转写验证。
- Sherpa Paraformer / Zipformer：轻量备用模型，适合低配置电脑。

## 发布来源验证

正式安装包由 `cg202601/Hi-Voicer` 的 GitHub Actions 在 `v*` tag 推送后自动构建、生成 attestation 并上传到 Release。不要使用来源不明的本地手工包。

```powershell
gh attestation verify .\Hi-Voicer_1.1.0_x64-setup.exe --repo cg202601/Hi-Voicer
```

## 开发验证

```powershell
npm ci
npm test
npm run build
cargo test --manifest-path src-tauri\Cargo.toml
npm run tauri -- build
```

更多说明见：

- [模型说明](docs/模型说明.md)
- [环境准备](docs/环境准备.md)
- [0.2.1 打包测试清单](docs/0.2.1-打包测试清单.md)
