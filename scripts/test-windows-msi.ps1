param(
    [Parameter(Mandatory = $true)]
    [string] $OlderMsi,
    [Parameter(Mandatory = $true)]
    [string] $NewerMsi,
    [string] $ProductName = "token-station",
    [string] $ExecutableName = "token-station-desktop.exe",
    [string] $LogDirectory = $env:RUNNER_TEMP
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Resolve-ExistingFile([string] $Path, [string] $Label) {
    $resolved = Resolve-Path -LiteralPath $Path -ErrorAction Stop
    if (-not (Test-Path -LiteralPath $resolved.Path -PathType Leaf)) {
        throw "$Label is not a file: $Path"
    }
    return $resolved.Path
}

function Invoke-Msi([string[]] $Arguments, [string] $Label) {
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = "msiexec.exe"
    $startInfo.UseShellExecute = $false
    $startInfo.Arguments = ($Arguments | ForEach-Object {
        if ($_.Contains('"')) {
            throw "$Label contains an unsupported quote in an argument"
        }
        if ($_ -match '\s') { '"{0}"' -f $_ } else { $_ }
    }) -join " "
    $process = [System.Diagnostics.Process]::Start($startInfo)
    if ($null -eq $process) {
        throw "$Label failed to start msiexec.exe"
    }
    $process.WaitForExit()
    return $process.ExitCode
}

function Invoke-InstalledSelfTest([string] $Label) {
    $safeLabel = $Label -replace '[^a-zA-Z0-9_-]', '-'
    $reportPath = Join-Path $LogDirectory "token-station-$safeLabel-self-test.json"
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $executable
    $startInfo.UseShellExecute = $false
    $startInfo.Arguments = '--self-test-bundled-plugins "{0}"' -f $reportPath
    $process = [System.Diagnostics.Process]::Start($startInfo)
    if ($null -eq $process) {
        throw "$Label failed to start the installed executable self-test"
    }
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) {
        throw "$Label installed executable self-test failed with exit code $($process.ExitCode)"
    }
    if (-not (Test-Path -LiteralPath $reportPath -PathType Leaf)) {
        throw "$Label installed executable did not write its self-test report"
    }
    $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
    if ($report.passed -ne $true -or
        $report.bundle.id -ne "com.tokenstation.desktop" -or
        $report.storage.data_directory_private -ne $true -or
        $report.storage.private_file_verified -ne $true -or
        $report.storage.credential_read -ne $false -or
        $report.gateway.loadable -ne $true) {
        throw "$Label installed executable self-test report is incomplete"
    }
    $expectedPlugins = @(
        "agent-anthropic",
        "agent-gemini",
        "agent-openai",
        "agent-openai-responses",
        "provider-openai-compatible"
    )
    $actualPlugins = @($report.plugins | ForEach-Object {
        if ($_.source -ne "builtin" -or $_.loadable -ne $true) {
            throw "$Label plugin $($_.id) is not a loadable builtin"
        }
        [string] $_.id
    } | Sort-Object)
    if (($actualPlugins -join ",") -ne (($expectedPlugins | Sort-Object) -join ",")) {
        throw "$Label installed executable has an unexpected builtin plugin set"
    }
}

function Get-MsiProperty([string] $Msi, [string] $Name) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $installer.GetType().InvokeMember(
        "OpenDatabase",
        "InvokeMethod",
        $null,
        $installer,
        @($Msi, 0)
    )
    $query = "SELECT ``Value`` FROM ``Property`` WHERE ``Property``='$Name'"
    $view = $database.GetType().InvokeMember(
        "OpenView",
        "InvokeMethod",
        $null,
        $database,
        @($query)
    )
    $view.GetType().InvokeMember("Execute", "InvokeMethod", $null, $view, $null)
    $record = $view.GetType().InvokeMember("Fetch", "InvokeMethod", $null, $view, $null)
    if ($null -eq $record) {
        return $null
    }
    return $record.GetType().InvokeMember(
        "StringData",
        "GetProperty",
        $null,
        $record,
        1
    )
}

function Get-InstalledVersion([string] $UpgradeCode) {
    $roots = @(
        "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*",
        "HKCU:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*"
    )
    foreach ($root in $roots) {
        $match = Get-ItemProperty -Path $root -ErrorAction SilentlyContinue |
            Where-Object { $_.PSChildName -and $_.DisplayName -eq $ProductName } |
            Select-Object -First 1
        if ($null -ne $match) {
            return [string] $match.DisplayVersion
        }
    }
    throw "installed product was not registered under HKCU (UpgradeCode=$UpgradeCode)"
}

$older = Resolve-ExistingFile $OlderMsi "older MSI"
$newer = Resolve-ExistingFile $NewerMsi "newer MSI"
$installDir = Join-Path $env:LOCALAPPDATA "Programs\$ProductName"
$executable = Join-Path $installDir $ExecutableName
$upgradeCode = Get-MsiProperty $newer "UpgradeCode"
$olderVersion = Get-MsiProperty $older "ProductVersion"
$newerVersion = Get-MsiProperty $newer "ProductVersion"

$normalizedUpgradeCode = $upgradeCode.Trim("{}").ToUpperInvariant()
if ($normalizedUpgradeCode -ne "BF3D3988-99EA-56E4-B81C-2AA4521C29C9") {
    throw "unexpected UpgradeCode: $upgradeCode"
}
if ([version] $newerVersion -le [version] $olderVersion) {
    throw "newer MSI version must be greater than older MSI version"
}

New-Item -ItemType Directory -Path $LogDirectory -Force | Out-Null
$olderLog = Join-Path $LogDirectory "token-station-msi-older-install.log"
$upgradeLog = Join-Path $LogDirectory "token-station-msi-upgrade.log"
$downgradeLog = Join-Path $LogDirectory "token-station-msi-downgrade.log"
$uninstallLog = Join-Path $LogDirectory "token-station-msi-uninstall.log"

# Remove a stale test installation from an interrupted runner before beginning.
Invoke-Msi @("/x", $newer, "/qn", "/norestart") "pre-clean newer" | Out-Null
Invoke-Msi @("/x", $older, "/qn", "/norestart") "pre-clean older" | Out-Null

try {
    $exit = Invoke-Msi @(
        "/i", $older, "/qn", "/norestart",
        "ALLUSERS=2", "MSIINSTALLPERUSER=1",
        "/l*v", $olderLog
    ) "install older"
    if ($exit -ne 0) {
        throw "older MSI installation failed with exit code $exit"
    }
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "per-user executable was not installed at $executable"
    }
    if ((Get-InstalledVersion $upgradeCode) -ne $olderVersion) {
        throw "installed version does not match older MSI"
    }
    Invoke-InstalledSelfTest "older-install"

    $app = Start-Process -FilePath $executable -PassThru
    Start-Sleep -Seconds 5
    if ($app.HasExited) {
        throw "installed desktop app exited during the launch health check"
    }
    Stop-Process -Id $app.Id -Force
    $app.WaitForExit()

    $exit = Invoke-Msi @(
        "/i", $newer, "/qn", "/norestart",
        "ALLUSERS=2", "MSIINSTALLPERUSER=1",
        "/l*v", $upgradeLog
    ) "upgrade"
    if ($exit -ne 0) {
        throw "MSI upgrade failed with exit code $exit"
    }
    if ((Get-InstalledVersion $upgradeCode) -ne $newerVersion) {
        throw "registered version did not advance after upgrade"
    }
    Invoke-InstalledSelfTest "newer-upgrade"

    $exit = Invoke-Msi @(
        "/i", $older, "/qn", "/norestart",
        "ALLUSERS=2", "MSIINSTALLPERUSER=1",
        "/l*v", $downgradeLog
    ) "downgrade"
    if ($exit -eq 0) {
        throw "older MSI unexpectedly downgraded the installed product"
    }
    if ((Get-InstalledVersion $upgradeCode) -ne $newerVersion) {
        throw "blocked downgrade changed the installed version"
    }

    $exit = Invoke-Msi @(
        "/x", $newer, "/qn", "/norestart", "/l*v", $uninstallLog
    ) "uninstall"
    if ($exit -ne 0) {
        throw "MSI uninstall failed with exit code $exit"
    }
    if (Test-Path -LiteralPath $executable) {
        throw "desktop executable remains after uninstall: $executable"
    }
} finally {
    if (Test-Path -LiteralPath $executable) {
        Invoke-Msi @("/x", $newer, "/qn", "/norestart") "cleanup newer" | Out-Null
        Invoke-Msi @("/x", $older, "/qn", "/norestart") "cleanup older" | Out-Null
    }
}

Write-Host "Windows MSI install/start/upgrade/downgrade-block/uninstall: PASS"
