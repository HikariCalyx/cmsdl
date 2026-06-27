# This script is sourced from https://get.msvc.win/64 and is used to install Microsoft Visual C++ Redistributable Runtimes on Windows.
# For source code, see https://github.com/msvc-win/get.msvc.win .
[Net.ServicePointManager]::SecurityProtocol = "Tls12, Tls11, Tls"

Add-Type -AssemblyName System.IO.Compression.FileSystem

function Main {
    if ($PSVersionTable.PSEdition -ne 'Desktop' -and $env:OS -ne 'Windows_NT') {
        Write-Warning "Please run it on Windows. "
        return 1
    }

    $procArch = $Env:PROCESSOR_ARCHITECTURE
    [int]$buildVersion = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').CurrentBuild
    if ($procArch -eq 'x86') {
        Write-Warning "This script does not support 32-bit Windows. Exiting..."
        return 1
    }
    elseif ($procArch -eq 'ARM64' -and $buildVersion -lt 21390) {
        Write-Warning "X64 emulation is not supported in this version of ARM64 Windows. Only x86 and ARM64 Runtime will be installed."
    }
    elseif ($procArch -eq 'ARM') {
        Write-Warning "This script does not support Windows RT. Exiting..."
        return 1
    }
    elseif ($procArch -eq 'ia64') {
        Write-Warning "This script does not support Itanium. Exiting..."
        return 1
    }

    if ((Get-Host).Version.Major -lt 3) {
        Write-Warning "Outdated PowerShell detected. Please update your OS, or use our offline installer. "
        Write-Host "To learn more, visit: https://msvc.win/#faq"
        return 1
    }

    $flagPath = "$env:TEMP\elevation_success.flag"
    if (Test-Path $flagPath) { Remove-Item $flagPath -ErrorAction SilentlyContinue }

    # --- Elevation check ---
    $IsAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)

    if (-not $IsAdmin) {
        Write-Host "Elevation required. Relaunching..."
        $tempScript = [IO.Path]::ChangeExtension([IO.Path]::GetTempFileName(), ".ps1")
        Invoke-WebRequest -Uri "https://get.msvc.win/64" -OutFile $tempScript
        Start-Process powershell -Verb RunAs -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File `"$tempScript`""
        Start-Sleep -Seconds 10
        if (-Not (Test-Path $flagPath)) {
            Write-Warning "User did not confirm UAC prompt or something went wrong."
            return 1
        }
        else {
            Remove-Item $flagPath -ErrorAction SilentlyContinue
            Write-Host "Setup ran successfully."
            return 0
        }
    }

    # --- Elevated section ---
    New-Item -Path $flagPath -ItemType File -Force | Out-Null
    Write-Host "Running with elevated privileges..."

    $tempDir = New-Item -ItemType Directory -Path ([IO.Path]::Combine($env:TEMP, [guid]::NewGuid().ToString()))
    $aria2Dir = Join-Path $tempDir "aria2"
    $aria2Exe = Join-Path $aria2Dir "aria2c.exe"

    # --- Check User IP location ---
    $countryCode = Get-CountryCode-Cloudflare
    if (-not $countryCode) {
        $countryCode = Get-CountryCode-MapleStoryCN
    }

    # --- Check Proxy Settings ---
    $webProxy = [System.Net.WebRequest]::GetSystemWebProxy()
    $uri = New-Object Uri("http://example.com")
    $proxyUri = $webProxy.GetProxy($uri)

    $useProxy = $proxyUri.Host -ne "example.com"
    if ($useProxy) {
        $proxyAddress = $proxyUri.Authority
        Write-Host "Detected system proxy: $proxyAddress"
    }

    # --- Ensure aria2c is available ---
    if (-not (Test-Path $aria2Exe)) {
        Write-Host "Downloading aria2c..."
        $aria2Zip = Join-Path $tempDir "aria2.zip"
        if ($countryCode -eq 'CN') {
            $aria2Url = "https://gitee.com/HikariCalyx/OSTRemote/releases/download/v1.0/aria2-1.37.0-win-64bit-build1.zip"
            # Workaround for specific Chinese ISPs that would not resolve domains properly like aka.ms
            $regionalWorkaround = "--async-dns-server=223.5.5.5"
        }
        else {
            $aria2Url = "https://github.com/aria2/aria2/releases/download/release-1.37.0/aria2-1.37.0-win-64bit-build1.zip"
            $regionalWorkaround = ""
        }
        if ($useProxy) {
            $regionalWorkaround = "--all-proxy=$proxyAddress"
        }
        Invoke-WebRequest -Uri $aria2Url -OutFile $aria2Zip
        Expand-Archive -Path $aria2Zip -DestinationPath $aria2Dir -Force
        $found = Get-ChildItem -Path $aria2Dir -Recurse -Filter "aria2c.exe" | Select-Object -First 1
        if ($found) { Copy-Item $found.FullName -Destination $aria2Exe -Force }
    }

    # --- Define all downloads ---
    $downloads = @(
        @{ Name = "vstor_redist.exe"; Url = "https://download.microsoft.com/download/8/6/4/8641e164-7796-4b34-81c7-30d24a5bd533/vstor_redist.exe" },
        @{ Name = "vcredist_2005_x86.exe"; Url = "https://download.microsoft.com/download/8/b/4/8b42259f-5d70-43f4-ac2e-4b208fd8d66a/vcredist_x86.exe"; Year = "2005" },
        @{ Name = "vcredist_2005_x64.exe"; Url = "https://download.microsoft.com/download/8/b/4/8b42259f-5d70-43f4-ac2e-4b208fd8d66a/vcredist_x64.exe"; Year = "2005" },
        @{ Name = "vcredist_2008_x86.exe"; Url = "https://download.microsoft.com/download/5/D/8/5D8C65CB-C849-4025-8E95-C3966CAFD8AE/vcredist_x86.exe"; Year = "2008" },
        @{ Name = "vcredist_2008_x64.exe"; Url = "https://download.microsoft.com/download/5/D/8/5D8C65CB-C849-4025-8E95-C3966CAFD8AE/vcredist_x64.exe"; Year = "2008" },
        @{ Name = "vcredist_2010_x86.exe"; Url = "https://download.microsoft.com/download/1/6/5/165255E7-1014-4D0A-B094-B6A430A6BFFC/vcredist_x86.exe"; Year = "2010" },
        @{ Name = "vcredist_2010_x64.exe"; Url = "https://download.microsoft.com/download/1/6/5/165255E7-1014-4D0A-B094-B6A430A6BFFC/vcredist_x64.exe"; Year = "2010" },
        @{ Name = "vcredist_2012_x86.exe"; Url = "https://download.microsoft.com/download/1/6/B/16B06F60-3B20-4FF2-B699-5E9B7962F9AE/VSU_4/vcredist_x86.exe"; Year = "2012" },
        @{ Name = "vcredist_2012_x64.exe"; Url = "https://download.microsoft.com/download/1/6/B/16B06F60-3B20-4FF2-B699-5E9B7962F9AE/VSU_4/vcredist_x64.exe"; Year = "2012" },
        @{ Name = "vcredist_2013_x86.exe"; Url = "https://download.visualstudio.microsoft.com/download/pr/10912113/5da66ddebb0ad32ebd4b922fd82e8e25/vcredist_x86.exe"; Year = "2013" },
        @{ Name = "vcredist_2013_x64.exe"; Url = "https://download.visualstudio.microsoft.com/download/pr/10912041/cee5d6bca2ddbcd039da727bf4acb48a/vcredist_x64.exe"; Year = "2013" }
    )

    if ($buildVersion -ge 10240) {
        $downloads += @{ Name = "vcredist_v14_x86.exe"; Url = "https://aka.ms/vc14/vc_redist.x86.exe"; Year = "v14" }
        $downloads += @{ Name = "vcredist_v14_x64.exe"; Url = "https://aka.ms/vc14/vc_redist.x64.exe"; Year = "v14" }
    } else {
        Write-Host "Older Windows detected. Visual C++ 2015-2022 Redistributable Runtime will be installed instead. "
        $downloads += @{ Name = "vcredist_2015-2022_x86.exe"; Url = "https://aka.ms/vs/17/release/vc_redist.x86.exe"; Year = "2015-2022" }
        $downloads += @{ Name = "vcredist_2015-2022_x64.exe"; Url = "https://aka.ms/vs/17/release/vc_redist.x64.exe"; Year = "2015-2022" }
    }

    if ($procArch -eq "ARM64" -and $buildVersion -lt 21390) {
        $downloads += @{ Name = "vcredist_v14_arm64.exe"; Url = "https://aka.ms/vc14/vc_redist.arm64.exe"; Year = "v14" }
    }

    $ariaListPath = Join-Path $tempDir "fileList.txt"

    # --- Download everything up front ---
    foreach ($item in $downloads) {
        $line = "$($item.Url)`n  out=$($item.Name)"
        Add-Content -Path $ariaListPath -Value $line
    }
    
    Write-Host "Downloading files..."
    $argumentList = @(
        "--dir=`"$tempDir`"",
        "--allow-overwrite=true",
        "--retry-wait=5",
        "--max-connection-per-server=8",
        "--split=8",
        "--min-split-size=1M",
        "-i `"$ariaListPath`""
    )
    if (![string]::IsNullOrEmpty($regionalWorkaround)) {
        $argumentList += $regionalWorkaround
    }
    $proc = Start-Process -FilePath $aria2Exe -ArgumentList $argumentList -NoNewWindow -PassThru -Wait
    if ($proc.ExitCode -ne 0) {
        Write-Warning "aria2c failed (exit code $($proc.ExitCode)). Retrying with --check-certificate=false..."
        $argsRetry = $argumentList + "--check-certificate=false"
        Start-Process -FilePath $aria2Exe -ArgumentList $argsRetry -NoNewWindow -Wait
    }

    # --- Check Signature ---
    foreach ($item in $downloads) {
        $outFile = Join-Path $tempDir $item.Name
        if (-not (Test-Path -Path $outFile)) {
            continue
        }
        Write-Host "Verifying signature of $($item.Name)..."
        $signature = Get-AuthenticodeSignature $outFile
        if ($signature.SignerCertificate.Subject -like "*CN=Microsoft Corporation*" -and $signature.Status -eq "Valid") {
            if ($env:WT_SESSION) {
                Write-Host "✅ Valid Microsoft signature found from $($item.Name)."
            }
            else {
                Write-Host "[OKAY] Valid Microsoft signature found from $($item.Name)."
            }
        }
        else {
            if ($env:WT_SESSION) {
                Write-Warning "⚠️ $($item.Name) is not signed by Microsoft. The download may get corrupted or your PC is infected with virus."
            }
            else {
                Write-Warning "[WARN] $($item.Name) is not signed by Microsoft. The download may get corrupted or your PC is infected with virus."
            }
            $response = Read-Host -Prompt "Would you like to proceed anyway? (Y/N)"
            if ($response -eq 'Y' -or $response -eq 'y') {
                continue
            }
            elseif ($response -eq 'N' -or $response -eq 'n') {
                return 1
            }
            else {
                Write-Host "Invalid input. Please enter Y or N."
            }
        }
    }

    # --- Install VC++ Redistributables ---
    foreach ($item in $downloads | Where-Object { $_.Name -like "vcredist*" }) {
        $exePath = Join-Path $tempDir $item.Name
        if (-not (Test-Path -Path $exePath)) {
            continue
        }
        $arguments = if ($item.Year -in @("2005", "2008")) { "/q" } else { "/quiet /norestart" }
        Write-Host "Installing $($item.Name)..."
        Start-Process -FilePath $exePath -ArgumentList $arguments -Wait
    }
    
    # --- Install Visual Studio 2010 Tools for Office Runtime ---
    Write-Host "Installing Visual Studio 2010 Tools for Office Runtime ..."
    Start-Process -FilePath "$tempDir\vstor_redist.exe" -ArgumentList "/q /norestart" -Wait

    # --- Cleanup ---
    Write-Host "Cleaning up temporary files..."
    Start-Sleep -Seconds 2
    Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -Path $flagPath -Force -ErrorAction SilentlyContinue
}

function Get-CountryCode-Cloudflare {
    param(
        [string]$Url = "https://www.qualcomm.cn/cdn-cgi/trace"
    )

    try {
        $raw = Invoke-RestMethod -Uri $Url -Headers @{
            "User-Agent" = "Mozilla/5.0"
        } -TimeoutSec 5 -ErrorAction Stop
    }
    catch {
        return $null   # Cloudflare unreachable or outage
    }

    # Parse key=value lines
    $dict = @{}
    foreach ($line in ($raw -split "`n")) {
        if ($line -match "=") {
            $k, $v = $line -split "=", 2
            $dict[$k] = $v
        }
    }

    return $dict["loc"]
}

function Get-CountryCode-MapleStoryCN {
    try
    {
        $geoInfo = Invoke-RestMethod -Uri 'https://mxdact.web.sdo.com/project/2025gmzr/'
        if ($geoInfo -match "Your location can not visit!") {
            $countryCode = 'ZZ'
        }
        else
        {
            $countryCode = 'CN'
        }
    }
    catch
    {
        $countryCode = 'ZZ'
    }
    return $countryCode
}

Main