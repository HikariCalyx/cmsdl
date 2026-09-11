# Reset Windows networking components to fix "unable to login to server" errors.
# Usage: powershell -ExecutionPolicy Bypass -File reset_network.ps1
# Called from NSIS with:
#   ExecWait 'powershell.exe -ExecutionPolicy Bypass -Command "Start-Process powershell -Verb RunAs -Wait -ArgumentList @(\"-NoProfile\",\"-ExecutionPolicy\",\"Bypass\",\"-File\",\"$TEMP\reset_network.ps1\")"' $0
#
# The resets briefly interrupt the network connection, and the two netsh
# resets only fully take effect after a reboot.
# Exits with 0 when every command succeeds, 1 otherwise.

$ErrorActionPreference = "Continue"

$LogPath = Join-Path $env:TEMP "reset_network.log"

function Write-Log {
    param([string]$Message)
    Write-Host $Message
    Add-Content -Path $LogPath -Value $Message -ErrorAction SilentlyContinue
}

# --- Elevation check: relaunch elevated when not already Administrator ---
# When called from the installer the UAC prompt is raised by NSIS, so this
# branch only runs when the script is launched directly.
$IsAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $IsAdmin) {
    Write-Host "Elevation required. Relaunching..."
    try {
        $proc = Start-Process powershell -Verb RunAs -Wait -PassThru -ArgumentList @(
            "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "`"$PSCommandPath`"")
    }
    catch {
        Write-Warning "Failed to elevate: $($_.Exception.Message)"
        exit 1
    }
    exit $proc.ExitCode
}

# Each command repairs a different part of the network stack.
$Commands = @(
    @("ipconfig", "/release"),
    @("ipconfig", "/flushdns"),
    @("ipconfig", "/renew"),
    @("netsh", "int", "ip", "reset"),
    @("netsh", "winsock", "reset")
)

Write-Log "=== Network reset started: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') ==="

$Failed = $false
foreach ($Command in $Commands) {
    $Exe     = $Command[0]
    $CmdArgs = @($Command[1..($Command.Count - 1)])

    Write-Log ""
    Write-Log "[CMD] $Exe $($CmdArgs -join ' ')"
    & $Exe @CmdArgs
    $ExitCode = $LASTEXITCODE
    Write-Log "[EXIT] $ExitCode"

    if ($ExitCode -ne 0) {
        $Failed = $true
    }
}

Write-Log ""
if ($Failed) {
    Write-Log "One or more commands reported an error."
}
else {
    Write-Log "All commands completed successfully."
}
Write-Log "Log file: $LogPath"

if ($Failed) { exit 1 }
exit 0
