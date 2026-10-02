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
