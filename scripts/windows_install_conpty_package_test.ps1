param(
    [Parameter(Mandatory = $true)]
    [string]$ArchivePath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$installerPath = (Resolve-Path -LiteralPath "$PSScriptRoot\..\distribution\install.ps1").Path
$bootstrapPath = (Resolve-Path -LiteralPath "$PSScriptRoot\..\distribution\install.cmd").Path
$bootstrapContent = Get-Content -LiteralPath $bootstrapPath -Raw
foreach ($forbiddenCommand in @("Invoke-RestMethod", "Invoke-WebRequest", "Invoke-Expression", "iex")) {
    if ($bootstrapContent -match "(?i)\b$forbiddenCommand\b") {
        throw "CMD bootstrap uses forbidden PowerShell network execution: $forbiddenCommand"
    }
}
if ($bootstrapContent -notmatch "(?i)\bcurl\.exe\b" -or $bootstrapContent -match "(?i)herdr\.dev|HERDR_INSTALLER_URL") {
    throw "CMD bootstrap must download the Lawn installer through curl.exe"
}
$parseErrors = $null
$tokens = $null
$installerAst = [System.Management.Automation.Language.Parser]::ParseFile(
    $installerPath,
    [ref]$tokens,
    [ref]$parseErrors
)
if ($parseErrors.Count -ne 0) {
    throw ($parseErrors | Out-String)
}
$forbiddenPowerShellCommands = @(
    $installerAst.FindAll(
        { param($node) $node -is [System.Management.Automation.Language.CommandAst] },
        $true
    ) |
        ForEach-Object { $_.GetCommandName() } |
        Where-Object {
            $_ -in @("Invoke-RestMethod", "Invoke-WebRequest", "Invoke-Expression", "irm", "iwr", "iex")
        }
)
if ($forbiddenPowerShellCommands.Count -ne 0) {
    throw "installer uses forbidden PowerShell network execution: $($forbiddenPowerShellCommands -join ', ')"
}
foreach ($functionName in @("Prepend-PathEntry", "Update-PathRegistryEntry")) {
    $definition = $installerAst.FindAll(
        {
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $node.Name -eq $functionName
        },
        $true
    ) | Select-Object -First 1
    if ($null -eq $definition) {
        throw "installer is missing function $functionName"
    }
    Invoke-Expression $definition.Extent.Text
}

$pathTestVariable = "LAWN_INSTALLER_PATH_TEST"
$oldPathTestVariable = [Environment]::GetEnvironmentVariable($pathTestVariable, "Process")
$ownedPathTestVariable = "LAWN_INSTALLER_OWNED_PATH_TEST"
$oldOwnedPathTestVariable = [Environment]::GetEnvironmentVariable($ownedPathTestVariable, "Process")
$testRegistryPath = "Software\LawnInstallerTests-$([Guid]::NewGuid().ToString('N'))"
$testEnvironmentKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($testRegistryPath)
if ($null -eq $testEnvironmentKey) {
    throw "unable to create temporary installer test registry key"
}
try {
    [Environment]::SetEnvironmentVariable($pathTestVariable, "C:\expanded", "Process")
    [Environment]::SetEnvironmentVariable($ownedPathTestVariable, "C:\Lawn", "Process")
    $testEnvironmentKey.SetValue(
        "Path",
        "C:\Lawn\releases\deleted;%$ownedPathTestVariable%\bin;C:\Lawn\bin;%$pathTestVariable%\bin;`"C:\Program Files\Tool`";C:\existing;C:\Lawn\releases\old\nested;C:\Lawn\releases-old\old;C:\Lawn\releases\deleted",
        [Microsoft.Win32.RegistryValueKind]::ExpandString
    )
    $ownedEntries = @("C:\Lawn\bin", "C:\Lawn\current")
    $pathChanged = Update-PathRegistryEntry `
        -EnvironmentKey $testEnvironmentKey `
        -Entry "C:\Lawn\releases\new" `
        -OwnedEntriesToRemove $ownedEntries `
        -OwnedEntryParentToRemove "C:\Lawn\releases"
    if (-not $pathChanged) {
        throw "installer PATH update reported no change"
    }
    if (Update-PathRegistryEntry `
        -EnvironmentKey $testEnvironmentKey `
        -Entry "C:\Lawn\releases\new" `
        -OwnedEntriesToRemove $ownedEntries `
        -OwnedEntryParentToRemove "C:\Lawn\releases") {
        throw "installer PATH update was not idempotent"
    }

    $options = [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
    $rawPath = $testEnvironmentKey.GetValue("Path", $null, $options)
    $expectedPath = "C:\Lawn\releases\new;%$pathTestVariable%\bin;`"C:\Program Files\Tool`";C:\existing;C:\Lawn\releases\old\nested;C:\Lawn\releases-old\old"
    if ($rawPath -cne $expectedPath) {
        throw "installer changed raw PATH: expected '$expectedPath', got '$rawPath'"
    }
    if ($testEnvironmentKey.GetValueKind("Path") -ne [Microsoft.Win32.RegistryValueKind]::ExpandString) {
        throw "installer changed the PATH registry value kind"
    }
} finally {
    $testEnvironmentKey.Dispose()
    [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($testRegistryPath, $false)
    [Environment]::SetEnvironmentVariable($pathTestVariable, $oldPathTestVariable, "Process")
    [Environment]::SetEnvironmentVariable($ownedPathTestVariable, $oldOwnedPathTestVariable, "Process")
}

$archive = (Resolve-Path -LiteralPath $ArchivePath).Path
$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
# Keep staged package paths below Windows PowerShell 5.1's MAX_PATH limit.
$root = Join-Path $env:RUNNER_TEMP ("lw-" + [Guid]::NewGuid().ToString("N").Substring(0, 12))
$probe = Join-Path $root "probe"
$lawnHome = Join-Path $root "home"
$installDir = Join-Path $root "bin"
$oldHome = $env:LAWN_HOME
$oldInstallDir = $env:LAWN_INSTALL_DIR
$oldPath = $env:Path
$registryOptions = [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
$userKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
if ($null -eq $userKey) { throw "unable to open the current user's environment registry key" }
$hadPath = $userKey.GetValueNames() -contains "Path"
$oldUserPath = if ($hadPath) { $userKey.GetValue("Path", $null, $registryOptions) } else { $null }
$oldUserPathKind = if ($hadPath) { $userKey.GetValueKind("Path") } else { $null }
$userKey.Dispose()

try {
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    Expand-Archive -LiteralPath $archive -DestinationPath $probe
    $versionOutput = (& (Join-Path $probe "lawn.exe") --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $versionOutput -notmatch '^lawn ([0-9]+\.[0-9]+\.[0-9]+)$') {
        throw "Windows release ZIP does not contain a stable Lawn binary: $versionOutput"
    }
    $version = $Matches[1]
    $env:LAWN_HOME = $lawnHome
    $env:LAWN_INSTALL_DIR = $installDir

    & $installerPath -Channel stable -LocalPackagePath $archive -LocalPackageFormat zip `
        -LocalPackageIdentity $version -LocalPackageSha256 $hash
    $releases = Join-Path $lawnHome "packages\standalone\releases"
    $current = Join-Path $lawnHome "packages\standalone\current"
    $original = Join-Path $releases "$version-x86_64-pc-windows-msvc"
    $required = @(
        "lawn.exe", "conpty\herdr-conpty.json", "conpty\conpty.dll",
        "conpty\x64\OpenConsole.exe", "conpty\arm64\OpenConsole.exe",
        "THIRD-PARTY-NOTICES\Microsoft.Windows.Console.ConPTY-LICENSE.txt",
        "THIRD-PARTY-NOTICES\Microsoft.Windows.Console.ConPTY-NOTICE.md"
    )
    foreach ($relative in $required) {
        if (-not (Test-Path -LiteralPath (Join-Path $installDir $relative) -PathType Leaf)) {
            throw "installer did not activate required file $relative"
        }
    }
    if (-not (Test-Path -LiteralPath (Join-Path $original "lawn.exe") -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $current "lawn.exe") -PathType Leaf)) {
        throw "installer did not activate a versioned Lawn release"
    }
    $activeBefore = [string](Get-Item -LiteralPath $current -Force).Target
    if (-not $activeBefore.Equals($original, [StringComparison]::OrdinalIgnoreCase)) {
        throw "current junction does not point to the original release"
    }
    & $installerPath -Channel stable -LocalPackagePath $archive -LocalPackageFormat zip `
        -LocalPackageIdentity $version -LocalPackageSha256 $hash
    if (@(Get-ChildItem -LiteralPath $releases -Directory).Count -ne 1) {
        throw "idempotent installation created another release"
    }

    $badChecksumRejected = $false
    try {
        & $installerPath -Channel stable -LocalPackagePath $archive -LocalPackageFormat zip `
            -LocalPackageIdentity "$version-custom.$('1' * 64)" -LocalPackageSha256 ('0' * 64)
    } catch {
        if ($_.Exception.Message -notlike "Downloaded Lawn checksum did not match*") { throw }
        $badChecksumRejected = $true
    }
    if (-not $badChecksumRejected) { throw "installer accepted a wrong ZIP checksum" }

    $wrongIdentityRejected = $false
    try {
        & $installerPath -Channel stable -LocalPackagePath $archive -LocalPackageFormat zip `
            -LocalPackageIdentity "9.9.9" -LocalPackageSha256 $hash
    } catch {
        if ($_.Exception.Message -notlike "Downloaded Lawn command failed product/version verification*") { throw }
        $wrongIdentityRejected = $true
    }
    if (-not $wrongIdentityRejected) { throw "installer accepted a wrong binary identity" }
    if (-not ([string](Get-Item -LiteralPath $current -Force).Target).Equals($original, [StringComparison]::OrdinalIgnoreCase)) {
        throw "failed installation changed the current Lawn release"
    }

    $tampered = Join-Path $root "missing-conpty.zip"
    Remove-Item -LiteralPath (Join-Path $probe "conpty\conpty.dll") -Force
    Compress-Archive -Path (Join-Path $probe '*') -DestinationPath $tampered
    $tamperedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $tampered).Hash.ToLowerInvariant()
    $badBundleRejected = $false
    try {
        & $installerPath -Channel stable -LocalPackagePath $tampered -LocalPackageFormat zip `
            -LocalPackageIdentity "$version-custom.$('2' * 64)" -LocalPackageSha256 $tamperedHash
    } catch {
        if ($_.Exception.Message -notlike "Downloaded Lawn package is incomplete*") { throw }
        $badBundleRejected = $true
    }
    if (-not $badBundleRejected) { throw "installer accepted a ZIP without ConPTY" }

    $hostDir = Join-Path $original "conpty\x64"
    $externalHostDir = Join-Path $root "external-conpty-host"
    Move-Item -LiteralPath $hostDir -Destination $externalHostDir
    New-Item -ItemType Junction -Path $hostDir -Target $externalHostDir | Out-Null
    & $installerPath -Channel stable -LocalPackagePath $archive -LocalPackageFormat zip `
        -LocalPackageIdentity $version -LocalPackageSha256 $hash
    if ((Get-Item -LiteralPath $hostDir -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "installer accepted a reparse-point ConPTY directory"
    }

    $newIdentity = "$version-custom.$('3' * 64)"
    $stagedDll = Join-Path $releases ".staging.$newIdentity-x86_64-pc-windows-msvc.$PID\conpty\conpty.dll"
    $lockState = @{ Handle = $null }
    $lockStagedFile = {
        if ($null -eq $lockState.Handle) {
            $lockState.Handle = [System.IO.File]::Open(
                $stagedDll, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read
            )
        }
    }.GetNewClosure()
    $breakpoint = Set-PSBreakpoint -Script $installerPath -Variable "backupDir" -Mode Write -Action $lockStagedFile
    try {
        $activationFailed = $false
        try {
            & $installerPath -Channel stable -LocalPackagePath $archive -LocalPackageFormat zip `
                -LocalPackageIdentity $newIdentity -LocalPackageSha256 $hash
        } catch { $activationFailed = $true }
        if ($null -eq $lockState.Handle -or -not $activationFailed) {
            throw "locked staged ConPTY file did not prevent activation"
        }
        if (-not ([string](Get-Item -LiteralPath $current -Force).Target).Equals($original, [StringComparison]::OrdinalIgnoreCase) -or
            -not (Test-Path -LiteralPath (Join-Path $current "conpty\conpty.dll") -PathType Leaf)) {
            throw "failed activation did not preserve the existing release"
        }
    } finally {
        Remove-PSBreakpoint -Breakpoint $breakpoint
        if ($null -ne $lockState.Handle) { $lockState.Handle.Dispose() }
    }

    & $installerPath -Channel stable -LocalPackagePath $archive -LocalPackageFormat zip `
        -LocalPackageIdentity $newIdentity -LocalPackageSha256 $hash
    $newRelease = Join-Path $releases "$newIdentity-x86_64-pc-windows-msvc"
    if (-not ([string](Get-Item -LiteralPath $current -Force).Target).Equals($newRelease, [StringComparison]::OrdinalIgnoreCase) -or
        -not (Test-Path -LiteralPath (Join-Path $installDir "conpty\conpty.dll") -PathType Leaf)) {
        throw "Lawn local package update did not activate the new release"
    }
    if (@(Get-ChildItem -LiteralPath $releases -Directory -Filter '.backup.*').Count -ne 0) {
        throw "installer left a release backup behind"
    }
} finally {
    $env:LAWN_HOME = $oldHome
    $env:LAWN_INSTALL_DIR = $oldInstallDir
    $env:Path = $oldPath
    $userKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
    try {
        if ($hadPath) {
            $userKey.SetValue("Path", $oldUserPath, $oldUserPathKind)
        } else {
            $userKey.DeleteValue("Path", $false)
        }
    } finally { $userKey.Dispose() }
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
