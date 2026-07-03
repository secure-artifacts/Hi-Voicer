# Third-Party Runtime Notices

Hi-Voicer bundles third-party runtime binaries so the desktop app can run local transcription engines offline after installation.

## Sherpa-ONNX CUDA runtime

- Runtime: `sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-win-x64-cuda`
- Upstream: https://github.com/k2-fsa/sherpa-onnx
- Release: https://github.com/k2-fsa/sherpa-onnx/releases/tag/v1.13.2
- License: Apache License 2.0, see https://github.com/k2-fsa/sherpa-onnx/blob/master/LICENSE

## ONNX Runtime binaries

The Sherpa-ONNX CUDA runtime package includes ONNX Runtime provider binaries such as `onnxruntime.dll` and `onnxruntime_providers_cuda.dll`.

- Upstream: https://github.com/microsoft/onnxruntime
- License: MIT License, see https://github.com/microsoft/onnxruntime/blob/main/LICENSE

## NVIDIA CUDA/cuDNN dependencies

Hi-Voicer does not bundle NVIDIA CUDA Toolkit or cuDNN DLLs. CUDA acceleration checks the user's local system for CUDA 12.x and cuDNN 9.x libraries and falls back to CPU when they are unavailable.