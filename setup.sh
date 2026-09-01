#!/bin/bash
# Готовит всё, что нужно для сборки Auris Whisper:
# движок whisper.cpp, кодеки Ogg/Opus/Vorbis и модель распознавания.
# Запускать один раз. Нужны: Xcode (или Command Line Tools), cmake, git, curl.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"

# Пинним версии, на которых всё собрано и проверено
WHISPER_CPP_SHA=eacbd8234c6654cdbf2c377f72b2106875479bdc
OGG_SHA=06a5e0262cdc28aa4ae6797627a783b5010440f0
OPUS_SHA=228a0f855ce24284e47866e30a66407202446a3a
OPUSFILE_SHA=6dfd29e7adb87f2e193575fc3fa88cbf1a0b27df
VORBIS_SHA=1b75110b5a2754ba1931d82dd83cb822b266a21d

MODEL_NAME="${MODEL_NAME:-ggml-large-v3-turbo-q5_0}"
MODEL_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${MODEL_NAME}.bin"

command -v cmake >/dev/null || { echo "Нужен cmake: brew install cmake"; exit 1; }
command -v xcrun >/dev/null || { echo "Нужны инструменты Xcode: xcode-select --install"; exit 1; }

fetch() {   # fetch <каталог> <url> <sha>
  local dir="$1" url="$2" sha="$3"
  if [ -d "$dir/.git" ]; then echo "==> $dir уже на месте"; return; fi
  echo "==> Забираю $url @ ${sha:0:8}"
  mkdir -p "$dir"
  git -C "$dir" init -q
  git -C "$dir" remote add origin "$url" 2>/dev/null || true
  git -C "$dir" fetch -q --depth 1 origin "$sha"
  git -C "$dir" checkout -q FETCH_HEAD
}

fetch "$ROOT/whisper.cpp" https://github.com/ggml-org/whisper.cpp.git "$WHISPER_CPP_SHA"
fetch "$ROOT/deps/ogg"      https://github.com/xiph/ogg.git      "$OGG_SHA"
fetch "$ROOT/deps/opus"     https://github.com/xiph/opus.git     "$OPUS_SHA"
fetch "$ROOT/deps/opusfile" https://github.com/xiph/opusfile.git "$OPUSFILE_SHA"
fetch "$ROOT/deps/vorbis"   https://github.com/xiph/vorbis.git   "$VORBIS_SHA"

echo "==> Собираю кодеки Ogg/Opus/Vorbis"
"$ROOT/build_deps.sh"

echo "==> Собираю whisper.cpp (Metal, статически)"
# GGML_NATIVE=OFF + фиксированный ARM_ARCH обязательны: иначе ggml вкомпилит
# инструкции того чипа, на котором собирают (на M4 — SME/i8mm), и бинарник
# упадёт с «Illegal instruction» на M1–M3.
cmake -S "$ROOT/whisper.cpp" -B "$ROOT/whisper.cpp/build" \
  -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF \
  -DGGML_METAL=ON -DGGML_METAL_EMBED_LIBRARY=ON -DGGML_ACCELERATE=ON \
  -DGGML_NATIVE=OFF -DGGML_CPU_ARM_ARCH="armv8.2-a+dotprod+fp16" \
  -DWHISPER_BUILD_EXAMPLES=OFF -DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_SERVER=OFF \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=12.0 -DCMAKE_OSX_ARCHITECTURES=arm64 >/dev/null
cmake --build "$ROOT/whisper.cpp/build" -j "$(sysctl -n hw.ncpu)" >/dev/null

echo "==> Скачиваю модель ${MODEL_NAME} (это надолго, файл большой)"
mkdir -p "$ROOT/models"
if [ ! -f "$ROOT/models/${MODEL_NAME}.bin" ]; then
  curl -L --fail --progress-bar -o "$ROOT/models/${MODEL_NAME}.bin" "$MODEL_URL"
fi

echo
echo "Готово. Дальше:"
echo "  MODEL=\"\$PWD/models/${MODEL_NAME}.bin\" ./build.sh"
