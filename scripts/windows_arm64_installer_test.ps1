param(
    [Parameter(Mandatory = $true)]
    [string]$ArchivePath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$architecture = [System.Runtime.InteropServices.RuntimeInformation, mscorlib]::OSArchitecture.ToString()
Write-Host "OS architecture: $architecture"
Write-Host "Process architecture: $([System.Runtime.InteropServices.RuntimeInformation, mscorlib]::ProcessArchitecture)"
Write-Host "PowerShell: $($PSVersionTable.PSVersion) $($PSVersionTable.PSEdition)"
Write-Host "Windows: $([Environment]::OSVersion.VersionString)"
if ($architecture -ne "Arm64") {
    throw "This test requires Windows ARM64, found $architecture."
}

$root = Join-Path $env:RUNNER_TEMP ("lawn-windows-arm64-installer-test-" + [Guid]::NewGuid().ToString("N"))
$oldHome = $env:LAWN_HOME
$oldInstallDir = $env:LAWN_INSTALL_DIR
$oldPath = $env:Path
$env:LAWN_HOME = Join-Path $root "home"
$env:LAWN_INSTALL_DIR = Join-Path $root "bin"
$installer = (Resolve-Path (Join-Path $PSScriptRoot "..\distribution\install.ps1")).Path
$archive = (Resolve-Path -LiteralPath $ArchivePath).Path
$hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
$registryKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
$registryOptions = [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
$hadPath = $registryKey.GetValueNames() -contains "Path"
$oldUserPath = if ($hadPath) { $registryKey.GetValue("Path", $null, $registryOptions) } else { $null }
$oldUserPathKind = if ($hadPath) { $registryKey.GetValueKind("Path") } else { $null }
$registryKey.Dispose()
try {
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    Expand-Archive -LiteralPath $archive -DestinationPath (Join-Path $root "probe")
    $versionOutput = (& (Join-Path $root "probe\lawn.exe") --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $versionOutput -notmatch '^lawn ([0-9]+\.[0-9]+\.[0-9]+)$') {
        throw "Windows ZIP does not contain a stable Lawn executable: $versionOutput"
    }
    & $installer -Channel stable -LocalPackagePath $archive -LocalPackageFormat zip `
        -LocalPackageIdentity $Matches[1] -LocalPackageSha256 $hash
    $installedLawn = Join-Path $env:LAWN_INSTALL_DIR "lawn.exe"
    if (-not (Test-Path -LiteralPath $installedLawn -PathType Leaf)) {
        throw "installer exited without activating lawn.exe"
    }
    if ((& $installedLawn --version | Out-String).Trim() -cne $versionOutput) {
        throw "installed x86_64 Lawn executable failed under Windows ARM64 emulation"
    }
    $releases = Join-Path $env:LAWN_HOME "packages\standalone\releases"
    if (@(Get-ChildItem -LiteralPath $releases -Force -Directory -Filter ".staging.*").Count -ne 0) {
        throw "installer left a staging directory"
    }
} finally {
    $env:LAWN_HOME = $oldHome
    $env:LAWN_INSTALL_DIR = $oldInstallDir
    $env:Path = $oldPath
    $registryKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
    try {
        if ($hadPath) {
            $registryKey.SetValue("Path", $oldUserPath, $oldUserPathKind)
        } else {
            $registryKey.DeleteValue("Path", $false)
        }
    } finally { $registryKey.Dispose() }
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host "Windows ARM64 Lawn installer test passed."
