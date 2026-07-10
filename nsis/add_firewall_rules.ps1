# Add MapleStory CN executables to Windows Firewall allow list.
# Usage: powershell -ExecutionPolicy Bypass -File add_firewall_rules.ps1 -InstallDir "C:\MapleStory"
# Called from NSIS with: ExecWait 'powershell -ExecutionPolicy Bypass -File "$INSTDIR\add_firewall_rules.ps1" -InstallDir "$INSTDIR"'

param(
    [Parameter(Mandatory=$true)]
    [string]$InstallDir
)

$ErrorActionPreference = "Stop"

# Check if Windows Firewall is enabled on at least one profile.
# netsh advfirewall show allprofiles outputs lines like:
#   Domain Profile Settings:
#   State                                 ON
# We parse the State value for each profile.
$FirewallOutput = netsh advfirewall show allprofiles 2>$null
if ($LASTEXITCODE -eq 0 -and $FirewallOutput) {
    $FirewallEnabled = $false
    $FirewallOutput -split "`r`n" | ForEach-Object {
        if ($_ -match '^\s*State\s+ON\s*$') {
            $FirewallEnabled = $true
        }
    }
    if (-not $FirewallEnabled) {
        Write-Host "Windows Firewall is disabled on all profiles. Skipping firewall rules."
        exit 0
    }
}
else {
    Write-Warning "Could not query Windows Firewall status. Proceeding anyway..."
}

# Resolve $INSTDIR/mxd as the base game directory
$MxdDir = Join-Path $InstallDir "mxd"
if (-not (Test-Path $MxdDir)) {
    Write-Warning "Game directory not found: $MxdDir"
    exit 1
}

# List of executables relative to $MxdDir
$Executables = @(
    "CrashReportClient.exe",
    "MapleStory.exe",
    "NxOverlay\DwarfAxe.exe",
    "Patcher.exe",
    "SDO\sdologin\CrashSender.exe",
    "SDO\sdologin\Launcher.exe",
    "SDO\sdologin\Launcher64.exe",
    "SDO\sdologin\sdologin.exe",
    "SDO\sdologin\sdolplugin.exe",
    "SDO\sdologin\unload.exe",
    "SDO\sdologin\update.exe",
    "SDO\sdologin\WebBrowser\wow_helper.exe",
    "SDO\sdologin\wkeplugin.exe"
)

$FailedCount = 0

foreach ($exe in $Executables) {
    $FullPath = Join-Path $MxdDir $exe
    $ExeName = [System.IO.Path]::GetFileNameWithoutExtension($exe)
    $RuleName = "MapleStoryCN - $ExeName"

    if (-not (Test-Path $FullPath)) {
        Write-Warning "Skipping (file not found): $FullPath"
        continue
    }

    try {
        # Remove existing rule if present (ignore error if not found)
        netsh advfirewall firewall delete rule name="$RuleName" >$null 2>&1

        # Add inbound allow rule
        netsh advfirewall firewall add rule `
            name="$RuleName" `
            dir=in `
            action=allow `
            program="$FullPath" `
            enable=yes `
            profile=any `
            >$null

        # Add outbound allow rule
        netsh advfirewall firewall add rule `
            name="$RuleName" `
            dir=out `
            action=allow `
            program="$FullPath" `
            enable=yes `
            profile=any `
            >$null

        Write-Host "Added firewall rule: $RuleName"
    }
    catch {
        Write-Warning "Failed to add firewall rule for: $FullPath"
        $FailedCount++
    }
}

if ($FailedCount -gt 0) {
    Write-Warning "$FailedCount rule(s) could not be added."
    exit 1
}

Write-Host "All firewall rules added successfully."
exit 0
