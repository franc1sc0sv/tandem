# Prueba de punta a punta del instalador NSIS en una maquina limpia (lo corre el CI en
# windows-latest): instalar, comprobar el arranque con la sesion (la tarea programada con los
# privilegios mas altos si el instalador corre como administrador, como en el CI; si no, la
# clave Run) y que el agente arranca sin
# config y la genera solo (T-082: el instalador no trae plantilla), reinstalar encima
# (actualizacion), desinstalar y comprobar que no queda nada del programa.
#
# Uso: pwsh tools/windows/installer/test-installer.ps1 -Installer <Tandem_..._x64-setup.exe>
# Ojo: instala y desinstala de verdad para el usuario actual; no correrlo en la PC de uso diario.

param([Parameter(Mandatory = $true)][string]$Installer)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$installDir = Join-Path $env:LOCALAPPDATA 'Tandem'
$dataDir = Join-Path $env:APPDATA 'Tandem'
$config = Join-Path $dataDir 'tandem.toml'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$launcher = Join-Path $installDir 'tandemw.exe'
$daemon = Join-Path $installDir 'tandemd.exe'

function Fail([string]$msg) {
    $logs = Join-Path $dataDir 'logs'
    if (Test-Path $logs) {
        Get-ChildItem $logs -File | ForEach-Object {
            Write-Host "--- $($_.FullName)"
            Get-Content $_.FullName -Tail 40
        }
    }
    throw "FALLO: $msg"
}

function Ok([string]$msg) { Write-Host "ok: $msg" }

function Get-RunValue {
    $props = Get-ItemProperty -Path $runKey -ErrorAction SilentlyContinue
    if ($props -and ($props.PSObject.Properties.Name -contains 'tandemd')) { return $props.tandemd }
    return $null
}

# T-104 (A7): con administrador el arranque es una tarea programada con los privilegios mas altos.
$taskName = 'Tandem\tandemd'

function Get-TaskXml {
    $xml = & schtasks.exe /Query /TN $taskName /XML 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    return ($xml -join "`n")
}

function Test-Elevated {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    return ([Security.Principal.WindowsPrincipal]$id).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Wait-Until([scriptblock]$cond, [int]$seconds, [string]$what) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        if (& $cond) { return }
        Start-Sleep -Milliseconds 500
    }
    Fail "$what (esperado en $seconds s)"
}

# Start-Process -Wait espera tambien a los descendientes, y el instalador deja corriendo
# tandemw/tandemd: nunca volveria. Se espera solo al proceso (leer Handle antes conserva
# el codigo de salida).
function Invoke-Silent([string]$exe) {
    $p = Start-Process -FilePath $exe -ArgumentList '/S' -PassThru
    $null = $p.Handle
    if (-not $p.WaitForExit(300000)) { Fail "$exe no termino en 5 min" }
    return $p.ExitCode
}

function Install-Silent {
    $code = Invoke-Silent $Installer
    if ($code -ne 0) { Fail "el instalador salio con $code" }
}

function Assert-Installed {
    foreach ($f in 'tandemd.exe', 'tandemw.exe', 'uninstall.exe') {
        if (-not (Test-Path (Join-Path $installDir $f))) { Fail "falta $f en $installDir" }
    }
    Ok "archivos en $installDir"
    # La tarea elevada solo se registra si el ejecutable esta donde solo escriben administradores
    # (Program Files); en %LOCALAPPDATA% (instalacion por usuario) queda la clave Run sin elevar.
    $adminOnly = $installDir.StartsWith($env:ProgramFiles, [StringComparison]::OrdinalIgnoreCase)
    if ((Test-Elevated) -and $adminOnly) {
        $xml = Get-TaskXml
        if (-not $xml) { Fail "como administrador se esperaba la tarea $taskName" }
        if ($xml -notmatch 'HighestAvailable') { Fail 'la tarea no pide los privilegios mas altos' }
        if (-not $xml.Contains($launcher)) { Fail "la tarea no lanza $launcher" }
        if ($null -ne (Get-RunValue)) { Fail 'con la tarea registrada no debe quedar la clave Run (arrancaria dos veces)' }
        Ok "tarea $taskName con los privilegios mas altos"
    } else {
        $expected = '"' + $launcher + '"'
        $actual = Get-RunValue
        if ($actual -ne $expected) { Fail "clave Run: se esperaba $expected y hay '$actual'" }
        if ($null -ne (Get-TaskXml)) { Fail 'una ruta escribible por el usuario no debe tener la tarea elevada' }
        Ok "clave Run -> $actual"
    }
    Wait-Until { Get-Process -Name tandemd -ErrorAction SilentlyContinue } 20 'tandemd no arranco tras instalar'
    Ok 'tandemd corriendo'
    # La genera el agente al arrancar, con los monitores que detecta.
    Wait-Until { Test-Path $config } 30 "tandemd no genero $config"
    & $daemon --check-config $config
    if ($LASTEXITCODE -ne 0) { Fail 'la config generada no es valida' }
    # T-097: la config generada empieza en el lado A; el lado de verdad lo deciden las claves al emparejar.
    if (-not (Select-String -Path $config -Pattern '^machine = "a"' -Quiet)) { Fail 'la config generada no empieza en el lado A' }
    Ok 'config generada por el agente y valida'
}

if ((Test-Path $installDir) -or (Get-RunValue) -or (Get-TaskXml)) { Fail 'la maquina no esta limpia: ya hay un Tandem instalado' }
if (Test-Path $config) { Fail "la maquina no esta limpia: ya hay $config" }

Write-Host '== instalar'
Install-Silent
Assert-Installed

Write-Host '== reinstalar encima (actualizacion): la config del usuario se conserva'
Add-Content -Path $config -Value '# cambio del usuario'
$before = Get-Content -Raw $config
Install-Silent
Assert-Installed
if ((Get-Content -Raw $config) -ne $before) { Fail 'reinstalar cambio la config del usuario' }
Ok 'config del usuario intacta'

Write-Host '== desinstalar'
$code = Invoke-Silent (Join-Path $installDir 'uninstall.exe')
if ($code -ne 0) { Fail "el desinstalador salio con $code" }
# El desinstalador de NSIS se copia a %TEMP% y sigue desde ahi: se espera a que termine.
Wait-Until { -not (Test-Path $daemon) -and -not (Test-Path $launcher) } 60 'el desinstalador no borro los binarios'
Wait-Until { -not (Test-Path (Join-Path $installDir 'uninstall.exe')) } 30 'el desinstalador no termino'
Ok 'binarios borrados'
if (Get-Process -Name tandemd, tandemw -ErrorAction SilentlyContinue) { Fail 'tandemd sigue corriendo' }
Ok 'tandemd detenido'
if ($null -ne (Get-RunValue)) { Fail 'la clave Run sigue registrada' }
if ($null -ne (Get-TaskXml)) { Fail "la tarea $taskName sigue registrada" }
Ok 'arranque con la sesion quitado'
if (-not (Test-Path $config)) { Fail 'la desinstalacion normal no debe borrar la config del usuario' }
Ok 'config del usuario conservada'

Remove-Item -Recurse -Force $dataDir
Write-Host 'instalador OK'
exit 0
