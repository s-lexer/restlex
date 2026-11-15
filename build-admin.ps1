$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$cacheDir = Join-Path $root ".pyinstaller-cache"
$workDir = Join-Path $root "build"
$distDir = Join-Path $root "dist"
$agentSrc = Join-Path $root "rust\target\release\restlex-agent.exe"
$agentDst = Join-Path $root "restlex-agent.exe"
$agentDist = Join-Path $distDir "restlex-agent.exe"

function Copy-FileSafe {
    param(
        [string]$Source,
        [string]$Destination,
        [string]$Label
    )

    if (-not (Test-Path -LiteralPath $Source)) {
        return $false
    }

    try {
        if (Test-Path -LiteralPath $Destination) {
            $srcItem = Get-Item -LiteralPath $Source
            $dstItem = Get-Item -LiteralPath $Destination
            if ($srcItem.Length -eq $dstItem.Length -and $srcItem.LastWriteTimeUtc -eq $dstItem.LastWriteTimeUtc) {
                return $true
            }
        }

        Copy-Item -LiteralPath $Source -Destination $Destination -Force
        return $true
    }
    catch {
        Write-Warning ("Failed to update {0}: {1}" -f $Label, $_.Exception.Message)
        return $false
    }
}

New-Item -ItemType Directory -Force -Path $cacheDir | Out-Null
New-Item -ItemType Directory -Force -Path $workDir | Out-Null
New-Item -ItemType Directory -Force -Path $distDir | Out-Null

foreach ($oldFile in @(
    (Join-Path $root "restlex-agent-local.exe"),
    (Join-Path $distDir "restlex-agent-local.exe")
)) {
    if (Test-Path -LiteralPath $oldFile) {
        try { Remove-Item -LiteralPath $oldFile -Force } catch { Write-Warning ("Failed to remove obsolete file {0}: {1}" -f $oldFile, $_.Exception.Message) }
    }
}

Copy-FileSafe -Source $agentSrc -Destination $agentDst -Label "restlex-agent.exe next to project" | Out-Null

$env:PYINSTALLER_CONFIG_DIR = $cacheDir
$specFile = Join-Path $root "restlex.spec"

pyinstaller --noconfirm --distpath $distDir --workpath $workDir $specFile

Copy-FileSafe -Source $agentDst -Destination $agentDist -Label "restlex-agent.exe to dist" | Out-Null

# Build Rust installer (win7-compatible)
$env:RUSTFLAGS = "-C target-feature=+crt-static -L native=$($env:USERPROFILE -replace '\\','/')/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/windows_x86_64_msvc-0.48.5/lib"
# Ключи организации — из того же файла, что и в build.ps1 (в исходниках их нет).
$secFile = Join-Path $env:USERPROFILE ".restlex\build-secrets.json"
if (Test-Path $secFile) {
    $sec = Get-Content $secFile -Raw | ConvertFrom-Json
    $env:RESTLEX_UPDATE_KEY = $sec.update_key; $env:RESTLEX_INSTALL_KEY = $sec.install_key
} else { Write-Warning "$secFile нет — restlex-installer собирается без ключей (публичная сборка)" }
Push-Location (Join-Path $root "rust")
cargo +nightly build --release -p restlex-installer --target x86_64-win7-windows-msvc -Z build-std=std,panic_abort
Pop-Location
Remove-Item Env:\RESTLEX_UPDATE_KEY, Env:\RESTLEX_INSTALL_KEY -ErrorAction SilentlyContinue
$installerSrc  = Join-Path $root "rust\target\x86_64-win7-windows-msvc\release\restlex-installer.exe"
$installerDist = Join-Path $distDir "restlex-installer.exe"
Copy-FileSafe -Source $installerSrc -Destination $installerDist -Label "restlex-installer.exe to dist" | Out-Null
