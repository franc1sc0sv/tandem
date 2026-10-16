# Construye el instalador NSIS de Tandem (Tandem_<version>_x64-setup.exe) con tandemd.exe y
# tandemw.exe como sidecars. Ver docs/dev/install-windows.md.
#
# Uso (PowerShell 7, desde cualquier carpeta, con pnpm y Rust en el PATH):
#   pwsh tools/windows/installer/build-installer.ps1
#
# Firma Authenticode opcional (si no hay ninguna variable, el instalador sale sin firmar):
#   TANDEM_WINDOWS_CERT_THUMBPRINT  huella SHA-1 de un certificado del almacen del usuario
#   TANDEM_WINDOWS_TIMESTAMP_URL    servidor de sellado de tiempo (por omision, el de DigiCert)
#   TANDEM_WINDOWS_SIGN_COMMAND     orden propia de firma; "%1" es el archivo a firmar
#                                   (por ejemplo, Azure Trusted Signing). Gana sobre la huella.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Checked([string]$what, [scriptblock]$block) {
    & $block
    if ($LASTEXITCODE -ne 0) { throw "$what fallo (codigo $LASTEXITCODE)" }
}

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$ui = Join-Path $repo 'apps\tandem-ui'
$srcTauri = Join-Path $ui 'src-tauri'

# Los sidecars de Tauri llevan el triple del destino en el nombre; al instalarse se les quita.
$hostLine = (rustc -vV | Select-String '^host: ').Line
$triple = $hostLine.Substring('host: '.Length).Trim()

Push-Location $repo
try {
    Invoke-Checked 'cargo build de tandemd' { cargo build --release -p tandemd --bins }
} finally {
    Pop-Location
}

$binaries = Join-Path $srcTauri 'binaries'
New-Item -ItemType Directory -Force -Path $binaries | Out-Null
foreach ($name in 'tandemd', 'tandemw') {
    Copy-Item -Force (Join-Path $repo "target\release\$name.exe") (Join-Path $binaries "$name-$triple.exe")
}

$configs = @((Join-Path $srcTauri 'windows\installer.conf.json'))
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
    Write-Host 'Firma Authenticode activada'
} else {
    Write-Host 'Sin firma Authenticode (no hay TANDEM_WINDOWS_CERT_THUMBPRINT ni TANDEM_WINDOWS_SIGN_COMMAND)'
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
if (-not $installer) { throw 'tauri build no dejo ningun instalador en target\release\bundle\nsis' }
Write-Host "Instalador: $($installer.FullName)"
if ($env:GITHUB_OUTPUT) { "installer=$($installer.FullName)" | Out-File -Append -Encoding utf8 $env:GITHUB_OUTPUT }
