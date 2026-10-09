#!/usr/bin/env bash
# Готовые статические библиотеки sherpa-onnx (разделение по голосам, русские модели)
# для macOS и Linux. Кладём их вне target/: rust-cache чистит target, а сохранённый
# вывод сборочного скрипта sherpa-onnx-sys по-прежнему указывает туда — и линковка
# падает с «could not find native static library». SHERPA_ONNX_LIB_DIR это обходит.
set -euo pipefail
V=1.13.8
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) A=osx-arm64-static-lib ;;
  Darwin-x86_64) A=osx-x64-static-lib ;;
  Linux-x86_64) A=linux-x64-static-lib ;;
  *) echo "unsupported: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac
STEM="sherpa-onnx-v$V-$A"
DIR="${RUNNER_TEMP:-/tmp}/sherpa-onnx"
mkdir -p "$DIR"
if [ ! -d "$DIR/$STEM/lib" ]; then
  curl -sSL --fail "https://github.com/k2-fsa/sherpa-onnx/releases/download/v$V/$STEM.tar.bz2" | tar xj -C "$DIR"
fi
ls "$DIR/$STEM/lib"
if [ -n "${GITHUB_ENV:-}" ]; then echo "SHERPA_ONNX_LIB_DIR=$DIR/$STEM/lib" >> "$GITHUB_ENV"; fi
