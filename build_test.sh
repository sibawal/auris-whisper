#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
WCPP="$ROOT/whisper.cpp"; BUILD="$WCPP/build"; DEPS="$ROOT/deps/install"
xcrun swiftc -O -swift-version 5 -target arm64-apple-macos14.0 -sdk "$(xcrun --show-sdk-path)" \
  -import-objc-header "$ROOT/src/Bridge.h" \
  -I "$WCPP/include" -I "$WCPP/ggml/include" -I "$DEPS/include" -I "$DEPS/include/opus" \
  "$ROOT/src/Localization.swift" "$ROOT/src/AudioDecoder.swift" "$ROOT/src/OggDecoder.swift" "$ROOT/src/WhisperEngine.swift" "$ROOT/test/main.swift" \
  -L "$BUILD/src" -L "$BUILD/ggml/src" -L "$BUILD/ggml/src/ggml-metal" -L "$BUILD/ggml/src/ggml-blas" -L "$DEPS/lib" \
  -lwhisper -lggml -lggml-base -lggml-cpu -lggml-blas -lggml-metal \
  -lopusfile -lopus -lvorbisfile -lvorbis -logg -lc++ \
  -framework Foundation -framework AVFoundation -framework CoreMedia -framework AudioToolbox \
  -framework Accelerate -framework Metal -framework MetalKit -framework CoreGraphics \
  -o "$ROOT/test/whispertest"
echo "test/whispertest собран"
