# Кладёт рядом с приложением DLL, без которых оно не запустится на «чистой» Windows:
#   vcruntime140*.dll, msvcp140.dll — рантайм Visual C++ (whisper.cpp написан на C++);
#   vulkan-1.dll — загрузчик Vulkan. Если драйвера Vulkan нет, ggml просто не найдёт
#   видеокарту и посчитает на процессоре, а не упадёт при старте.
# Берём из System32 машины, на которой собираем (нужен установленный Vulkan Runtime/SDK).
$ErrorActionPreference = "Stop"
$dst = Join-Path $PSScriptRoot "..\src-tauri\windows-runtime"
New-Item -ItemType Directory -Force -Path $dst | Out-Null
foreach ($dll in "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll", "vulkan-1.dll") {
    $src = Join-Path $env:SystemRoot "System32\$dll"
    if (-not (Test-Path $src)) { throw "$dll not found in System32" }
    Copy-Item $src $dst -Force
    Write-Host "  + $dll"
}

# Разделение по голосам (sherpa-onnx) на Windows подключается как DLL. Скачиваем
# их заранее: Tauri проверяет наличие файлов-ресурсов ещё при компиляции.
# SHERPA_ONNX_LIB_DIR говорит sherpa-onnx-sys взять библиотеки отсюда же.
$sherpaVersion = "1.13.8"
$stem = "sherpa-onnx-v$sherpaVersion-win-x64-shared-MT-Release-lib"
$cache = Join-Path $PSScriptRoot "..\src-tauri\target\sherpa-onnx-prebuilt"
$libDir = Join-Path $cache "$stem\lib"
if (-not (Test-Path (Join-Path $libDir "sherpa-onnx-c-api.dll"))) {
    New-Item -ItemType Directory -Force -Path $cache | Out-Null
    $archive = Join-Path $cache "$stem.tar.bz2"
    Invoke-WebRequest "https://github.com/k2-fsa/sherpa-onnx/releases/download/v$sherpaVersion/$stem.tar.bz2" -OutFile $archive
    tar -xjf $archive -C $cache
}
foreach ($dll in "sherpa-onnx-c-api.dll", "onnxruntime.dll", "onnxruntime_providers_shared.dll") {
    Copy-Item (Join-Path $libDir $dll) $dst -Force
    Write-Host "  + $dll"
}
$libDir = (Resolve-Path $libDir).Path
if ($env:GITHUB_ENV) { "SHERPA_ONNX_LIB_DIR=$libDir" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8 }
Write-Host "SHERPA_ONNX_LIB_DIR=$libDir"
