#!/bin/bash
# Сборка Auris Whisper.app — самодостаточного приложения для macOS (Apple Silicon).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WCPP="$ROOT/whisper.cpp"
BUILD="$WCPP/build"
DEPS="$ROOT/deps/install"
SDK="$(xcrun --show-sdk-path)"

# Настраивается снаружи:
#   MIN_MACOS=12.0 ./build.sh          — минимальная версия macOS (по умолчанию 13.0)
#   MODEL=models/ggml-large-v3.bin     — какую модель вшить
#   APP_PATH="out/Auris Whisper.app"   — куда положить бандл
MIN_MACOS="${MIN_MACOS:-13.0}"
MODEL="${MODEL:-$ROOT/models/ggml-large-v3.bin}"
APP="${APP_PATH:-$ROOT/Auris Whisper.app}"

echo "==> Чищу старый бандл"
rm -rf "$APP" "$ROOT/Whisper.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

echo "==> Компилирую Swift (минимальная macOS ${MIN_MACOS}, модель $(basename "$MODEL"))"
xcrun swiftc \
  -O -swift-version 5 \
  -target arm64-apple-macos${MIN_MACOS} \
  -sdk "$SDK" \
  -import-objc-header "$ROOT/src/Bridge.h" \
  -I "$WCPP/include" -I "$WCPP/ggml/include" -I "$DEPS/include" -I "$DEPS/include/opus" \
  "$ROOT/src/Localization.swift" \
  "$ROOT/src/SystemMonitor.swift" \
  "$ROOT/src/AudioDecoder.swift" \
  "$ROOT/src/OggDecoder.swift" \
  "$ROOT/src/WhisperEngine.swift" \
  "$ROOT/src/Recorder.swift" \
  "$ROOT/src/AppModel.swift" \
  "$ROOT/src/ContentView.swift" \
  "$ROOT/src/WhisperApp.swift" \
  -L "$BUILD/src" -L "$BUILD/ggml/src" -L "$BUILD/ggml/src/ggml-metal" -L "$BUILD/ggml/src/ggml-blas" \
  -L "$DEPS/lib" \
  -lwhisper -lggml -lggml-base -lggml-cpu -lggml-blas -lggml-metal \
  -lopusfile -lopus -lvorbisfile -lvorbis -logg -lc++ \
  -framework Foundation -framework AppKit -framework SwiftUI \
  -framework AVFoundation -framework CoreMedia -framework CoreAudio -framework AudioToolbox \
  -framework Accelerate -framework Metal -framework MetalKit -framework QuartzCore \
  -framework CoreGraphics -framework UniformTypeIdentifiers -framework IOKit \
  -o "$APP/Contents/MacOS/AurisWhisper"

echo "==> Кладу модель и лицензии в бандл"
cp "$MODEL" "$APP/Contents/Resources/"
cp "$ROOT/LICENSE" "$APP/Contents/Resources/LICENSE.txt"
cp "$ROOT/THIRD-PARTY-LICENSES.txt" "$APP/Contents/Resources/"

echo "==> Info.plist"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>                 <string>Auris Whisper</string>
    <key>CFBundleDisplayName</key>          <string>Auris Whisper</string>
    <key>CFBundleExecutable</key>           <string>AurisWhisper</string>
    <key>CFBundleIdentifier</key>           <string>ru.boopi.auriswhisper</string>
    <key>CFBundlePackageType</key>          <string>APPL</string>
    <key>CFBundleShortVersionString</key>   <string>1.0</string>
    <key>CFBundleVersion</key>              <string>1</string>
    <key>CFBundleIconFile</key>             <string>AppIcon</string>
    <key>LSMinimumSystemVersion</key>       <string>__MIN_MACOS__</string>
    <key>LSApplicationCategoryType</key>    <string>public.app-category.productivity</string>
    <key>NSHighResolutionCapable</key>      <true/>
    <key>NSHumanReadableCopyright</key>
    <string>© 2026 boopi.ru — открытая лицензия MIT</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>Доступ к микрофону нужен, чтобы надиктовывать текст прямо в приложении.</string>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>     <string>Аудио или видео</string>
            <key>CFBundleTypeRole</key>     <string>Viewer</string>
            <key>LSHandlerRank</key>        <string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.audio</string>
                <string>public.movie</string>
                <string>public.mpeg-4-audio</string>
                <string>public.mp3</string>
                <string>com.microsoft.waveform-audio</string>
                <string>org.xiph.ogg-audio</string>
            </array>
        </dict>
    </array>
    <key>UTImportedTypeDeclarations</key>
    <array>
        <dict>
            <key>UTTypeIdentifier</key>      <string>org.xiph.ogg-audio</string>
            <key>UTTypeDescription</key>     <string>Аудио Ogg (Vorbis/Opus)</string>
            <key>UTTypeConformsTo</key>
            <array><string>public.audio</string></array>
            <key>UTTypeTagSpecification</key>
            <dict>
                <key>public.filename-extension</key>
                <array><string>ogg</string><string>oga</string><string>opus</string></array>
                <key>public.mime-type</key>
                <array><string>audio/ogg</string><string>audio/opus</string></array>
            </dict>
        </dict>
    </array>
</dict>
</plist>
PLIST

sed -i '' "s/__MIN_MACOS__/${MIN_MACOS}/" "$APP/Contents/Info.plist"

printf 'APPL????' > "$APP/Contents/PkgInfo"

if [ -f "$ROOT/AppIcon.icns" ]; then
  echo "==> Иконка"
  cp "$ROOT/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
fi

echo "==> Подпись (ad-hoc)"
codesign --force --deep --sign - --timestamp=none "$APP"
codesign --verify --verbose=1 "$APP" 2>&1 | tail -1

echo "==> Готово: $APP"
du -sh "$APP"
