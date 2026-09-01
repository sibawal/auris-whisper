#!/bin/bash
# Статическая сборка кодеков Ogg: Vorbis и Opus (macOS их сам не читает).
set -euo pipefail
export PATH=/opt/homebrew/bin:$PATH
ROOT="$(cd "$(dirname "$0")" && pwd)"
D="$ROOT/deps"
PREFIX="$D/install"
COMMON="-DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DCMAKE_OSX_ARCHITECTURES=arm64 \
        -DCMAKE_OSX_DEPLOYMENT_TARGET=12.0 -DCMAKE_INSTALL_PREFIX=$PREFIX \
        -DCMAKE_PREFIX_PATH=$PREFIX -DCMAKE_POLICY_VERSION_MINIMUM=3.5"

echo "==> libogg"
cmake -S "$D/ogg" -B "$D/ogg/build" $COMMON -DBUILD_TESTING=OFF >/dev/null
cmake --build "$D/ogg/build" -j 10 --target install >/dev/null

echo "==> libopus"
cmake -S "$D/opus" -B "$D/opus/build" $COMMON -DOPUS_BUILD_PROGRAMS=OFF -DOPUS_BUILD_TESTING=OFF >/dev/null
cmake --build "$D/opus/build" -j 10 --target install >/dev/null

echo "==> libopusfile"
cmake -S "$D/opusfile" -B "$D/opusfile/build" $COMMON \
      -DOP_DISABLE_HTTP=ON -DOP_DISABLE_EXAMPLES=ON -DOP_DISABLE_DOCS=ON >/dev/null
cmake --build "$D/opusfile/build" -j 10 --target install >/dev/null

echo "==> libvorbis"
cmake -S "$D/vorbis" -B "$D/vorbis/build" $COMMON -DBUILD_TESTING=OFF >/dev/null
cmake --build "$D/vorbis/build" -j 10 --target install >/dev/null

echo "==> итог"
ls -1 "$PREFIX/lib"/*.a
