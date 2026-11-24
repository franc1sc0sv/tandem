# Builds the Tandem NSIS installer (Tandem_<version>_x64-setup.exe) with tandemd.exe,
# tandemw.exe and tandem.exe (the CLI and MCP server, T-357) as sidecars. See docs/dev/install-windows.md.
#
# Usage (PowerShell 7, from any folder, with pnpm and Rust on the PATH):
#   pwsh tools/windows/installer/build-installer.ps1
#
# Optional Authenticode signing (if no variable is set, the installer comes out unsigned):
#   TANDEM_WINDOWS_CERT_THUMBPRINT  SHA-1 thumbprint of a certificate in the user store
#   TANDEM_WINDOWS_TIMESTAMP_URL    timestamp server (DigiCert's by default)
#   TANDEM_WINDOWS_SIGN_COMMAND     custom signing command; "%1" is the file to sign
#                                   (for example, Azure Trusted Signing). Wins over the thumbprint.
#
# Updater package (T-122): with TANDEM_UPDATER_ARTIFACTS=1 it also leaves the minisign signature
# of the installer (<installer>.sig) for latest.json. It needs TAURI_SIGNING_PRIVATE_KEY (and
# TAURI_SIGNING_PRIVATE_KEY_PASSWORD if the key has a password); see docs/dev/release.md.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Checked([string]$what, [scriptblock]$block) {
    & $block
    if ($LASTEXITCODE -ne 0) { throw "$what failed (code $LASTEXITCODE)" }
}

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$ui = Join-Path $repo 'apps\tandem-ui'
$srcTauri = Join-Path $ui 'src-tauri'

# Tauri sidecars carry the target triple in the name; it is stripped on install.
$hostLine = (rustc -vV | Select-String '^host: ').Line
$triple = $hostLine.Substring('host: '.Length).Trim()

Push-Location $repo
try {
    Invoke-Checked 'cargo build of tandemd' { cargo build --release -p tandemd --bins }
    Invoke-Checked 'cargo build of tandem' { cargo build --release -p tandem-cli --bins }
} finally {
    Pop-Location
}

$binaries = Join-Path $srcTauri 'binaries'
New-Item -ItemType Directory -Force -Path $binaries | Out-Null
foreach ($name in 'tandemd', 'tandemw', 'tandem') {
    Copy-Item -Force (Join-Path $repo "target\release\$name.exe") (Join-Path $binaries "$name-$triple.exe")
}

$configs = @((Join-Path $srcTauri 'windows\installer.conf.json'))
if ($env:TANDEM_UPDATER_ARTIFACTS -eq '1') {
    if (-not $env:TAURI_SIGNING_PRIVATE_KEY) { throw 'TANDEM_UPDATER_ARTIFACTS=1 needs TAURI_SIGNING_PRIVATE_KEY (docs/dev/release.md)' }
    $configs += (Join-Path $srcTauri 'tauri.updater.json')
    Write-Host 'With signed updater package'
}
$signing = $null
if ($env:TANDEM_WINDOWS_SIGN_COMMAND) {
    $signing = @{ signCommand = $env:TANDEM_WINDOWS_SIGN_COMMAND }
} elseif ($env:TANDEM_WINDOWS_CERT_THUMBPRINT) {
    $timestamp = if ($env:TANDEM_WINDOWS_TIMESTAMP_URL) { $env:TANDEM_WINDOWS_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
    $signing = @{
        certificateThumbprint = $env:TANDEM_WINDOWS_CERT_THUMBPRINT
        digestAlgorithm = 'sha256'
        timestampUrl = $timestamp
    }
}
if ($signing) {
    $signConfig = Join-Path ([IO.Path]::GetTempPath()) 'tandem-sign.conf.json'
    @{ bundle = @{ windows = $signing } } | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 $signConfig
    $configs += $signConfig
    Write-Host 'Authenticode signing enabled'
} else {
    Write-Host 'No Authenticode signing (neither TANDEM_WINDOWS_CERT_THUMBPRINT nor TANDEM_WINDOWS_SIGN_COMMAND is set)'
}

$configArgs = @()
foreach ($c in $configs) { $configArgs += @('--config', $c) }

Push-Location $ui
try {
    Invoke-Checked 'tauri build' { pnpm tauri build --features app @configArgs }
} finally {
    Pop-Location
}

$installer = Get-ChildItem (Join-Path $repo 'target\release\bundle\nsis\*-setup.exe') |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $installer) { throw 'tauri build left no installer in target\release\bundle\nsis' }
Write-Host "Installer: $($installer.FullName)"
if ($env:GITHUB_OUTPUT) { "installer=$($installer.FullName)" | Out-File -Append -Encoding utf8 $env:GITHUB_OUTPUT }
