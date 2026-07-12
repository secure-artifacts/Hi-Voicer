# Third-Party Runtime Notices

Hi-Voicer ships CPU-only local inference components and no bundled GPU runtime.

CUDA support has been removed from the public product path because it requires NVIDIA-specific CUDA Toolkit and cuDNN dependencies that are difficult to distribute reliably for ordinary Windows users.

DirectML acceleration is experimental and must be validated per machine with diagnostics and CPU comparison before it is treated as a reliable path.

## FFmpeg

Hi-Voicer release packages include `ffmpeg.exe` and `ffprobe.exe` from the FFmpeg Windows release essentials builds provided by Gyan Doshi at https://www.gyan.dev/ffmpeg/builds/.

FFmpeg is a third-party multimedia framework. The bundled Gyan Windows builds are 64-bit static builds licensed as GPLv3. FFmpeg source code is available from https://github.com/FFmpeg/FFmpeg and project information is available at https://ffmpeg.org/.

Hi-Voicer does not modify FFmpeg. `ffplay.exe` is not bundled.

## llama.cpp

Hi-Voicer includes the official `b9964` Windows CPU x64 runtime from llama.cpp for local Qwen3-ASR GGUF inference. llama.cpp is licensed under the MIT License. Project source and license information are available at https://github.com/ggml-org/llama.cpp.

## Sherpa-ONNX

Hi-Voicer includes two static CPU executables from the official Sherpa-ONNX `v1.13.2` Windows release: `sherpa-onnx-offline.exe` and `sherpa-onnx-offline-websocket-server.exe`. Sherpa-ONNX is licensed under the Apache License 2.0. Project source and license information are available at https://github.com/k2-fsa/sherpa-onnx.
