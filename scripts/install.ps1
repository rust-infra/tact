# Install tact-ui on Windows (prefer GitHub release, else build from source).
#Requires -Version 5.1
param(
    [string]$InstallDir = "",
    [switch]$FromSource,
    [switch]$Release,
    [switch]$ReleaseOnly,
    [string]$GitRef = "main",
    [switch]$SkipDeps,
    [switch]$NoModifyPath,
    [switch]$Help
)

$ErrorActionPreference = "Stop"

$Repo = if ($env:TACT_INSTALL_REPO) { $env:TACT_INSTALL_REPO } else { "rust-infra/tact" }
$BinaryName = "tact-ui"
$CratePackage = "tact-ui"
$DefaultVersion = "0.19.0"
# Matches workspace.package.rust-version (edition 2024).
$MinRustcVersion = "1.85.0"

function Show-Help {
    @"
Usage: install.ps1 [OPTIONS]

Install the tact-ui binary on Windows.

By default the installer downloads a matching GitHub release asset when one
exists, otherwise it builds from source (Rust 1.85+ / edition 2024 required).

Options:
  -InstallDir PATH     Install directory (default: %USERPROFILE%\.local\bin)
  -FromSource          Build from source only (skip release download)
  -Release             Prefer a GitHub release binary; fall back to source build
  -ReleaseOnly         Require a GitHub release binary (no source fallback)
  -GitRef REF          Git branch/tag when cloning (default: main)
  -SkipDeps            Skip rustup / optional SQLite helper installation
  -NoModifyPath        Do not add the install directory to the user PATH
  -Help                Show this help

Environment:
  TACT_INSTALL_REPO    GitHub repo (owner/name)
  TACT_INSTALL_DIR     Override install directory

Examples:
  irm https://raw.githubusercontent.com/rust-infra/tact/main/scripts/install.ps1 | iex
  .\scripts\install.ps1 -FromSource
  .\scripts\install.ps1 -Release -InstallDir "$env:USERPROFILE\.local\bin"
"@
}

if ($Help) {
    Show-Help
    exit 0
}

function Write-Step([string]$Message) {
    Write-Host "==> $Message"
}

function Write-Warn([string]$Message) {
    Write-Warning $Message
}

function Fail([string]$Message) {
    throw $Message
}

function Test-RepoRoot([string]$Path) {
    return ((Test-Path (Join-Path $Path "Cargo.toml")) -and (Test-Path (Join-Path $Path "crates\tact")))
}

function Get-DefaultInstallDir {
    Join-Path $env:USERPROFILE ".local\bin"
}

function Get-TargetTriple {
    if ([Environment]::Is64BitOperatingSystem) {
        return "x86_64-pc-windows-msvc"
    }
    Fail "unsupported Windows architecture (32-bit)"
}

function ConvertTo-Version([string]$Text) {
    # Strip optional pre-release / metadata so [version] parses (e.g. 1.85.0-nightly → 1.85.0).
    $clean = ($Text -split '[-+]')[0]
    return [version]$clean
}

function Test-RustcMeetsMinimum {
    $rustc = Get-Command rustc -ErrorAction SilentlyContinue
    if (-not $rustc) { return $false }
    $line = & rustc -V 2>$null
    if (-not $line) { return $false }
    $parts = $line -split '\s+'
    if ($parts.Count -lt 2) { return $false }
    try {
        return (ConvertTo-Version $parts[1]) -ge (ConvertTo-Version $MinRustcVersion)
    }
    catch {
        return $false
    }
}

function Resolve-Version([string]$Root) {
    $cargoToml = Join-Path $Root "Cargo.toml"
    if (Test-Path $cargoToml) {
        $inPkg = $false
        foreach ($line in Get-Content $cargoToml) {
            if ($line -match '^\[workspace\.package\]') {
                $inPkg = $true
                continue
            }
            if ($line -match '^\[') {
                $inPkg = $false
                continue
            }
            if ($inPkg -and ($line -match '^version = "(.+)"')) {
                return $Matches[1]
            }
        }
        $match = Select-String -Path $cargoToml -Pattern '^version = "(.+)"' | Select-Object -First 1
        if ($match) {
            return $match.Matches[0].Groups[1].Value
        }
    }
    return $DefaultVersion
}

function Ensure-Rust {
    if ($SkipDeps) { return }

    if ((Test-RustcMeetsMinimum) -and (Get-Command cargo -ErrorAction SilentlyContinue)) {
        return
    }

    $rustup = Get-Command rustup -ErrorAction SilentlyContinue
    if ($rustup) {
        Write-Step "Updating stable Rust toolchain (need >= $MinRustcVersion for edition 2024)..."
        & rustup toolchain install stable --profile minimal
        if ($LASTEXITCODE -ne 0) { Fail "rustup toolchain install failed" }
        & rustup default stable
        if ($LASTEXITCODE -ne 0) { Fail "rustup default stable failed" }
    }
    elseif (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Step "Rust toolchain not found; installing via rustup (stable >= $MinRustcVersion)..."
        $rustupInit = Join-Path $env:TEMP "rustup-init.exe"
        Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupInit -UseBasicParsing
        & $rustupInit -y --default-toolchain stable | Out-Host
        Remove-Item $rustupInit -Force -ErrorAction SilentlyContinue
    }
    else {
        Fail "rustc is too old; install Rust >= $MinRustcVersion from https://rustup.rs"
    }

    $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    if (Test-Path $cargoBin) {
        $env:Path = "$cargoBin;$env:Path"
    }

    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Fail "cargo still not found after rustup install; restart PowerShell and retry"
    }
    if (-not (Test-RustcMeetsMinimum)) {
        Fail "rustc is still below $MinRustcVersion; run: rustup update stable"
    }
}

function Install-WindowsDeps {
    if ($SkipDeps) { return }

    if (Get-Command scoop -ErrorAction SilentlyContinue) {
        Write-Step "Ensuring SQLite (scoop)..."
        scoop list sqlite 2>$null | Out-Null
        if ($LASTEXITCODE -ne 0) {
            scoop install main/sqlite 2>$null | Out-Null
        }
        return
    }

    if (Get-Command choco -ErrorAction SilentlyContinue) {
        Write-Step "Ensuring SQLite (chocolatey)..."
        choco install sqlite -y | Out-Null
        return
    }

    Write-Warn "Windows builds compile SQLite from source via sqlx; Visual Studio C++ build tools may be required."
}

function Install-BinaryFile([string]$SourcePath, [string]$DestDir) {
    if (-not (Test-Path $DestDir)) {
        New-Item -ItemType Directory -Path $DestDir -Force | Out-Null
    }
    $dest = Join-Path $DestDir "$BinaryName.exe"
    Copy-Item -Path $SourcePath -Destination $dest -Force
    Write-Step "Installed $BinaryName -> $dest"
}

function Build-FromSource([string]$Root) {
    Write-Step "Building $BinaryName from source (rustc >= $MinRustcVersion)..."
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Fail "cargo not found"
    }

    $cargoArgs = @("build", "--release", "-p", $CratePackage)
    if (Test-Path (Join-Path $Root "Cargo.lock")) {
        $cargoArgs += "--locked"
    }

    Push-Location $Root
    try {
        & cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) {
            Fail "cargo build failed"
        }
    }
    finally {
        Pop-Location
    }

    $built = Join-Path $Root "target\release\$BinaryName.exe"
    if (-not (Test-Path $built)) {
        Fail "build succeeded but binary missing: $built"
    }
    Install-BinaryFile $built $InstallDir
}

function Try-InstallRelease([string]$Version, [string]$Triple) {
    $assetName = "$BinaryName-v$Version-$Triple.zip"
    $url = "https://github.com/$Repo/releases/download/v$Version/$assetName"
    $tmp = Join-Path $env:TEMP ("tact-install-" + [Guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $tmp -Force | Out-Null

    Write-Step "Trying release asset: $assetName"
    try {
        Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp $assetName) -UseBasicParsing
    }
    catch {
        Write-Warn "release asset not found at $url"
        Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
        return $false
    }

    Expand-Archive -Path (Join-Path $tmp $assetName) -DestinationPath $tmp -Force
    $candidate = Join-Path $tmp "$BinaryName.exe"
    if (-not (Test-Path $candidate)) {
        $candidate = Get-ChildItem -Path $tmp -Filter "$BinaryName.exe" -Recurse -ErrorAction SilentlyContinue |
            Select-Object -First 1 -ExpandProperty FullName
    }

    if (-not $candidate -or -not (Test-Path $candidate)) {
        Write-Warn "release archive did not contain $BinaryName.exe"
        Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
        return $false
    }

    Install-BinaryFile $candidate $InstallDir
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    return $true
}

function Ensure-PathEntry([string]$Dir) {
    if ($NoModifyPath) { return }

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $segments = @()
    if ($userPath) {
        $segments = $userPath -split ';' | Where-Object { $_ -and ($_ -ne $Dir) }
    }
    if ($userPath -and ($userPath -split ';' | Where-Object { $_ -eq $Dir })) {
        return
    }

    $newPath = (@($Dir) + $segments) -join ';'
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    if ($env:Path -notlike "*$Dir*") {
        $env:Path = "$Dir;$env:Path"
    }
    Write-Step "Added $Dir to user PATH (restart terminals to pick up everywhere)"
}

if (-not $InstallDir) {
    if ($env:TACT_INSTALL_DIR) {
        $InstallDir = $env:TACT_INSTALL_DIR
    }
    else {
        $InstallDir = Get-DefaultInstallDir
    }
}

$preferSource = $FromSource.IsPresent

if ($Release.IsPresent) {
    $preferSource = $false
}

$srcRoot = $null
$work = $null

if (Test-RepoRoot (Get-Location).Path) {
    $srcRoot = (Get-Location).Path
    Write-Step "Using current repository: $srcRoot"
}
else {
    $work = Join-Path $env:TEMP ("tact-src-" + [Guid]::NewGuid().ToString())
    Write-Step "Cloning https://github.com/$Repo.git ($GitRef)..."
    & git clone --depth 1 --branch $GitRef "https://github.com/$Repo.git" $work
    if ($LASTEXITCODE -ne 0) {
        Fail "git clone failed"
    }
    $srcRoot = $work
}

$version = Resolve-Version $srcRoot
$triple = Get-TargetTriple

if ($preferSource) {
    Ensure-Rust
    Install-WindowsDeps
    Build-FromSource $srcRoot
    Ensure-PathEntry $InstallDir
    Write-Step "Done. Run: $BinaryName --help"
    exit 0
}

if (Try-InstallRelease $version $triple) {
    Ensure-PathEntry $InstallDir
    Write-Step "Done. Run: $BinaryName --help"
    exit 0
}

if ($ReleaseOnly) {
    Fail "no release asset found for v$version ($triple); publish a release or omit -ReleaseOnly"
}

Write-Warn "falling back to source build"
Ensure-Rust
Install-WindowsDeps
Build-FromSource $srcRoot
Ensure-PathEntry $InstallDir
Write-Step "Done. Run: $BinaryName --help"
