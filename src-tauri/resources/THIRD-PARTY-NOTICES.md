# Third-Party Runtime Notices

Hi-Voicer currently ships without a bundled GPU runtime. Local transcription models and the Sherpa-ONNX CPU runtime are prepared through the normal model setup flow.

CUDA support has been removed from the public product path because it requires NVIDIA-specific CUDA Toolkit and cuDNN dependencies that are difficult to distribute reliably for ordinary Windows users.

DirectML acceleration is experimental and must be validated per machine with diagnostics and CPU comparison before it is treated as a reliable path.