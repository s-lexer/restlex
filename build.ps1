<#
.SYNOPSIS
    Единый билд-скрипт RestLex — собирает любой компонент или всё сразу,
    проверяет согласованность версий, копирует в dist\, подписывает, деплоит.

.DESCRIPTION
    Компоненты и как они собираются:
      gui            — PyInstaller (restlex.spec) -> dist\restlex.exe        (версия из version.py / file_version_info.txt)
      agent          — Rust win7-таргет (nightly + build-std)                -> dist\restlex-agent.exe
      installer      — Rust win7-таргет (nightly + build-std)                -> dist\restlex-installer.exe
      installer-klik — Rust win7-таргет (GUI-инсталлятор с вшитым ключом)   -> dist\restlex-installer-klik.exe
      webserver      — Rust обычный stable release                          -> dist\restlex-webserver.exe
      update-server  — Rust обычный stable release                          -> dist\restlex-update-server.exe

    win7-таргет нужен потому, что служба restlex-agent и installer крутятся на Win7/2008R2.
    webserver/update-server живут только на свежем сервере — им win7 не нужен.

.PARAMETER Components
    Что собрать: gui, agent, installer, webserver, update-server, или all (по умолчанию all).
    Можно несколько: -Components gui,agent

.PARAMETER Sign
    Подписать собранные exe через EV sign-service (адрес/токен — в build-secrets.json) после сборки.

.PARAMETER Deploy
    Запустить deploy.ps1 после успешной сборки (копирует на UPD + обновляет versions.json/launcher.version).

.PARAMETER CheckVersions
    Только проверить согласованность версий и выйти, ничего не собирая.

.PARAMETER SkipVersionCheck
    Не падать при рассогласовании версий (по умолчанию рассогласование = стоп).

.PARAMETER NoBump
    Не поднимать версии. По умолчанию скрипт сам бампает патч-версию КАЖДОГО
    собираемого компонента (gui — version.py + file_version_info.txt, остальные —
    Cargo.toml своего крейта) перед сборкой: выложить пересобранный бинарь под
    старой версией = клиенты его не увидят.

.PARAMETER Bump
    Оставлен для явности; bump и так включён по умолчанию (см. -NoBump).

.EXAMPLE
    .\build.ps1                              # собрать всё
    .\build.ps1 -Components gui              # только GUI
    .\build.ps1 -Components agent,webserver  # агент + веб
    .\build.ps1 -Sign -Deploy               # всё, подписать, выложить
    .\build.ps1 -CheckVersions              # только проверка версий
#>
[CmdletBinding()]
param(
    [ValidateSet('all','gui','agent','installer','installer-klik','webserver','update-server')]
    [string[]]$Components = @('all'),
    [switch]$Sign,
    [switch]$Deploy,
    [switch]$CheckVersions,
    [switch]$SkipVersionCheck,
    [switch]$Bump,
    [switch]$NoBump
)

$ErrorActionPreference = 'Stop'
$root     = $PSScriptRoot
$distDir  = Join-Path $root 'dist'
$rustDir  = Join-Path $root 'rust'
$rustTgt  = Join-Path $rustDir 'target'
$cacheDir = Join-Path $root '.pyinstaller-cache'
$workDir  = Join-Path $root 'build'

# win7 cross-build flags (как в build-admin.ps1)
$WIN7_TARGET   = 'x86_64-win7-windows-msvc'
$WIN7_RUSTFLAGS = "-C target-feature=+crt-static -L native=$($env:USERPROFILE -replace '\\','/')/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/windows_x86_64_msvc-0.48.5/lib"

function Info($m) { Write-Host "[build] $m" -ForegroundColor Cyan }
function Ok($m)   { Write-Host "[ ok ] $m"  -ForegroundColor Green }
function Warn($m) { Write-Host "[warn] $m"  -ForegroundColor Yellow }
function Die($m)  { Write-Host "[FAIL] $m"   -ForegroundColor Red; exit 1 }

New-Item -ItemType Directory -Force -Path $distDir | Out-Null

# ---------------------------------------------------------------------------
# Версии: единый источник правды
# ---------------------------------------------------------------------------
function Get-GuiVersion {
    $vp = Join-Path $root 'version.py'
    $line = Select-String -Path $vp -Pattern 'APP_VERSION\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($line) { return $line.Matches[0].Groups[1].Value }
    return '?'
}
function Get-FileVerInfoVersion {
    $fv = Join-Path $root 'file_version_info.txt'
    $line = Select-String -Path $fv -Pattern "ProductVersion',\s*u'([^']+)'" | Select-Object -First 1
    if ($line) { return $line.Matches[0].Groups[1].Value }
    return '?'
}
function Get-CargoVersion($crate) {
    $toml = Join-Path $rustDir "crates\$crate\Cargo.toml"
    $line = Select-String -Path $toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($line) { return $line.Matches[0].Groups[1].Value }
    return '?'
}

# Патч-инкремент последнего компонента версии: 1.8.3 → 1.8.4, 2.3 → 2.4
function Step-Version($v) {
    $p = @($v -split '\.')
    $p[-1] = [string]([int]$p[-1] + 1)
    return ($p -join '.')
}

function Bump-Gui {
    $old = Get-GuiVersion
    $new = Step-Version $old
    $vp = Join-Path $root 'version.py'
    $t  = (Get-Content $vp -Raw) -replace 'APP_VERSION\s*=\s*"[^"]+"', "APP_VERSION = `"$new`""
    Set-Content $vp $t -NoNewline -Encoding utf8
    # PE-ресурс: deploy.ps1 читает именно его → должен идти в ногу с version.py
    $fv    = Join-Path $root 'file_version_info.txt'
    $t     = Get-Content $fv -Raw
    $nums  = @($new -split '\.')
    while ($nums.Count -lt 4) { $nums += '0' }
    $tuple = ($nums -join ', ')
    $t = $t -replace 'filevers=\([^)]+\)', "filevers=($tuple)"
    $t = $t -replace 'prodvers=\([^)]+\)', "prodvers=($tuple)"
    $t = $t -replace "(FileVersion',\s+u')[^']+'",    "`${1}$new'"
    $t = $t -replace "(ProductVersion',\s+u')[^']+'", "`${1}$new'"
    Set-Content $fv $t -NoNewline -Encoding utf8
    Ok "GUI: $old → $new (version.py + file_version_info.txt)"
    Bump-Simple $new
}

# RestLex-Simple — отдельный продукт (C# .NET 3.5), но версия обязана совпадать
# с GUI: иначе веб показывает такие хосты как «устаревшие».
function Bump-Simple($new) {
    $bv = Join-Path $root 'BackupClient\BackupClient\BuildVersion.cs'
    if (-not (Test-Path $bv)) { Warn "BuildVersion.cs не найден — RestLex-Simple не бампнут."; return }
    $t = Get-Content $bv -Raw
    $t = $t -replace 'Version\s*=\s*"Mini-[^"]*"', ('Version = "Mini-' + $new + '"')
    Set-Content $bv $t -NoNewline -Encoding utf8

    $ai = Join-Path $root 'BackupClient\BackupClient\Properties\AssemblyInfo.cs'
    if (Test-Path $ai) {
        $nums = @($new -split '\.')
        while ($nums.Count -lt 4) { $nums += '0' }
        $quad = ($nums -join '.')
        $t = Get-Content $ai -Raw
        $t = $t -replace 'AssemblyVersion\("[^"]*"\)',     ('AssemblyVersion("' + $quad + '")')
        $t = $t -replace 'AssemblyFileVersion\("[^"]*"\)', ('AssemblyFileVersion("' + $quad + '")')
        Set-Content $ai $t -NoNewline -Encoding utf8
    }
    Ok "RestLex-Simple: Mini-$new (BuildVersion.cs + AssemblyInfo.cs)"
}

function Bump-Crate($crate) {
    $toml = Join-Path $rustDir "crates\$crate\Cargo.toml"
    $old  = Get-CargoVersion $crate
    $new  = Step-Version $old
    # только первое вхождение — это version в [package]; в [dependencies]
    # тоже есть version=, но они не в начале строки
    $rx = [regex]'(?m)^version\s*=\s*"[^"]+"'
    Set-Content $toml ($rx.Replace((Get-Content $toml -Raw), "version = `"$new`"", 1)) -NoNewline -Encoding utf8
    Ok "${crate}: $old → $new"
}

# Поднять версии тех компонентов, что реально собираем. Авто при -Deploy —
# выкладывать на UPD старую версию бессмысленно: клиенты её не увидят.
function Bump-Components($components) {
    Info 'Bump версий...'
    foreach ($c in $components) {
        switch ($c) {
            'gui'            { Bump-Gui }
            'agent'          { Bump-Crate 'restlex-agent' }
            'installer'      { Bump-Crate 'restlex-installer' }
            'installer-klik' { if (Test-KlikChanged) { Bump-Crate 'restlex-installer-klik' } else { Ok 'installer-klik — без изменений, bump пропущен.' } }
            'webserver'      { Bump-Crate 'restlex-webserver' }
            'update-server'  { Bump-Crate 'restlex-update-server' }
        }
    }
}

function Test-Versions {
    Info 'Проверка версий...'
    $gui      = Get-GuiVersion
    $guiPe    = Get-FileVerInfoVersion
    $agent    = Get-CargoVersion 'restlex-agent'
    $web      = Get-CargoVersion 'restlex-webserver'
    $upd      = Get-CargoVersion 'restlex-update-server'
    $inst     = Get-CargoVersion 'restlex-installer'
    $klik     = Get-CargoVersion 'restlex-installer-klik'
    $core     = Get-CargoVersion 'restlex-core'

    Write-Host ""
    Write-Host ("  GUI (version.py)            : {0}" -f $gui)
    Write-Host ("  GUI (file_version_info.txt) : {0}" -f $guiPe)
    Write-Host ("  agent                       : {0}" -f $agent)
    Write-Host ("  webserver                   : {0}" -f $web)
    Write-Host ("  update-server               : {0}" -f $upd)
    Write-Host ("  installer                   : {0}" -f $inst)
    Write-Host ("  installer-klik              : {0}" -f $klik)
    Write-Host ("  core                        : {0}" -f $core)
    Write-Host ""

    $problems = @()
    # GUI: version.py и PE-ресурс должны совпадать (PyInstaller вшивает PE → deploy.ps1 читает его)
    if ($gui -ne $guiPe) {
        $problems += "GUI рассогласование: version.py=$gui, file_version_info.txt=$guiPe (deploy.ps1 читает PE → веб покажет $guiPe)"
    }
    if ($problems.Count -gt 0) {
        foreach ($p in $problems) { Warn $p }
        if (-not $SkipVersionCheck) {
            Die "Версии рассогласованы. Поправь и пересобери, либо -SkipVersionCheck."
        }
    } else {
        Ok 'Версии согласованы.'
    }
    return [pscustomobject]@{ Gui=$gui; Agent=$agent; Web=$web; Upd=$upd; Inst=$inst; Klik=$klik }
}

# Возвращает $true если в кликовом крейте есть изменения новее dist-exe
function Test-KlikChanged {
    $exe = Join-Path $distDir 'restlex-installer-klik.exe'
    if (-not (Test-Path $exe)) { return $true }
    $exeTime = (Get-Item $exe).LastWriteTimeUtc
    $srcDir  = Join-Path $rustDir 'crates\restlex-installer-klik'
    $newer = Get-ChildItem -Path $srcDir -Recurse -File |
        Where-Object { $_.LastWriteTimeUtc -gt $exeTime }
    return ($null -ne $newer -and @($newer).Count -gt 0)
}

# ---------------------------------------------------------------------------
# Сборщики
# ---------------------------------------------------------------------------
function Build-Gui {
    Info 'GUI (PyInstaller)...'
    New-Item -ItemType Directory -Force -Path $cacheDir,$workDir | Out-Null
    $env:PYINSTALLER_CONFIG_DIR = $cacheDir
    & pyinstaller --noconfirm --distpath $distDir --workpath $workDir (Join-Path $root 'restlex.spec')
    if ($LASTEXITCODE -ne 0) { Die 'PyInstaller вернул ошибку.' }
    $exe = Join-Path $distDir 'restlex.exe'
    if (-not (Test-Path $exe)) { Die 'dist\restlex.exe не появился.' }
    # Smoke-проверка: PE-версия должна быть не 0.0.0 (иначе deploy зальёт 0.0.0)
    $pe = (Get-Item $exe).VersionInfo.ProductVersion
    if (-not $pe -or $pe -match '^0\.0') { Die "dist\restlex.exe ProductVersion='$pe' — PE-версия не вшилась (проверь file_version_info.txt в restlex.spec)." }
    Ok "dist\restlex.exe (ProductVersion=$pe)"
}

# Ключи организации — только в установщики (их раздаём своим клиентам сами).
# Агент/GUI/Simple собираются БЕЗ ключей: их отдаёт общий публичный сервер обновлений.
# Источник — %USERPROFILE%\.restlex\build-secrets.json (вне репозитория).
$SECRETS_FILE = Join-Path $env:USERPROFILE '.restlex\build-secrets.json'
$KEYED_CRATES = @('restlex-installer', 'restlex-installer-klik')

function Build-RustWin7($crate, $exeName) {
    Info "$crate (win7-таргет)..."
    $env:RUSTFLAGS = $WIN7_RUSTFLAGS
    Remove-Item Env:\RESTLEX_UPDATE_KEY, Env:\RESTLEX_INSTALL_KEY -ErrorAction SilentlyContinue
    if ($KEYED_CRATES -contains $crate) {
        if (Test-Path $SECRETS_FILE) {
            $sec = Get-Content $SECRETS_FILE -Raw | ConvertFrom-Json
            if ($sec.update_key)  { $env:RESTLEX_UPDATE_KEY  = $sec.update_key }
            if ($sec.install_key) { $env:RESTLEX_INSTALL_KEY = $sec.install_key }
            if ($sec.update_key -and $sec.install_key) { Info "${crate}: ключи организации вшиты из $SECRETS_FILE" }
            else { Warn "${crate}: в $SECRETS_FILE нет update_key/install_key — вшито не всё" }
        } else {
            Warn "$SECRETS_FILE нет — $crate собирается без ключей (публичная сборка)."
        }
    }
    Push-Location $rustDir
    try {
        & cargo +nightly build --release -p $crate --target $WIN7_TARGET -Z build-std=std,panic_abort
        if ($LASTEXITCODE -ne 0) { Die "cargo build $crate (win7) вернул ошибку." }
    } finally {
        Pop-Location
        Remove-Item Env:\RUSTFLAGS, Env:\RESTLEX_UPDATE_KEY, Env:\RESTLEX_INSTALL_KEY -ErrorAction SilentlyContinue
    }
    $src = Join-Path $rustTgt "$WIN7_TARGET\release\$exeName"
    if (-not (Test-Path $src)) { Die "$src не найден." }
    Copy-Item $src (Join-Path $distDir $exeName) -Force
    Ok "dist\$exeName"
}

function Build-RustRelease($crate, $exeName) {
    Info "$crate (stable release)..."
    Push-Location $rustDir
    try {
        & cargo build --release -p $crate
        if ($LASTEXITCODE -ne 0) { Die "cargo build $crate вернул ошибку." }
    } finally {
        Pop-Location
    }
    $src = Join-Path $rustTgt "release\$exeName"
    if (-not (Test-Path $src)) { Die "$src не найден." }
    Copy-Item $src (Join-Path $distDir $exeName) -Force
    Ok "dist\$exeName"
}

# ---------------------------------------------------------------------------
# Подпись
# ---------------------------------------------------------------------------
function Sign-Exes($exeNames) {
    Info 'Подпись через EV sign-service...'
    $paths = @($exeNames | ForEach-Object { Join-Path $distDir $_ } | Where-Object { Test-Path $_ })
    if ($paths.Count -eq 0) { Warn 'Нечего подписывать.'; return }
    & python (Join-Path $root '_sign.py') @paths
    if ($LASTEXITCODE -ne 0) { Die 'Подпись завершилась ошибкой.' }
    Ok ("Подписано: {0}" -f ($exeNames -join ', '))
}

# ---------------------------------------------------------------------------
# Главный поток
# ---------------------------------------------------------------------------
if ($CheckVersions) { Test-Versions | Out-Null; exit 0 }

# Разворачиваем 'all'
if ($Components -contains 'all') {
    $Components = @('gui','agent','installer','installer-klik','webserver','update-server')
}

# Bump по умолчанию: пересобранный бинарь обязан приехать с новой версией,
# иначе клиенты не увидят обновление. Отключается -NoBump.
if (-not $NoBump) { Bump-Components $Components }

$versions = Test-Versions

$built = @()   # имена exe в dist\ для последующей подписи

foreach ($c in $Components) {
    switch ($c) {
        'gui'           { Build-Gui;                                            $built += 'restlex.exe' }
        'agent'         { Build-RustWin7   'restlex-agent'     'restlex-agent.exe';         $built += 'restlex-agent.exe' }
        'installer'     { Build-RustWin7   'restlex-installer'      'restlex-installer.exe';      $built += 'restlex-installer.exe' }
        'installer-klik' {
            # Собираем всегда: cargo инкрементален, а вшитые ключи могли смениться
            # без правки исходников (Test-KlikChanged решает только про bump).
            Build-RustWin7 'restlex-installer-klik' 'restlex-installer-klik.exe'
            $built += 'restlex-installer-klik.exe'
        }
        'webserver'     { Build-RustRelease 'restlex-webserver'    'restlex-webserver.exe';     $built += 'restlex-webserver.exe' }
        'update-server' { Build-RustRelease 'restlex-update-server' 'restlex-update-server.exe'; $built += 'restlex-update-server.exe' }
    }
}

if ($Sign)   { Sign-Exes $built }

if ($Deploy) {
    Info 'Деплой...'
    # deploy.ps1 заливает на UPD только те exe, что переданы; передаём именно собранные
    & (Join-Path $root 'deploy.ps1') -Files $built
    if ($LASTEXITCODE -ne 0) { Die 'deploy.ps1 вернул ошибку.' }
    Ok 'Деплой завершён.'
}

Write-Host ""
Ok ("Готово. Собрано: {0}" -f ($built -join ', '))
Write-Host ("Версии: GUI={0} agent={1} web={2} upd={3} installer={4} klik={5}" -f $versions.Gui,$versions.Agent,$versions.Web,$versions.Upd,$versions.Inst,$versions.Klik)
