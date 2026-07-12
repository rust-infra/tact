# Build Tact Book CHM on Windows (requires HTML Help Workshop).
#
# Prerequisites:
#   1. Run build-chm.sh first (Git Bash / WSL) OR ensure book/output/chm exists
#   2. Install HTML Help Workshop (free, legacy Microsoft tool):
#      https://learn.microsoft.com/en-us/previous-versions/windows/desktop/htmlhelp/microsoft-html-help-downloads
#   3. pandoc: https://pandoc.org/installing.html
#
# Usage:
#   powershell -File book/scripts/build-chm.ps1
#   powershell -File book/scripts/build-chm.ps1 -SkipConvert   # compile only

param(
    [switch]$SkipConvert
)

$ErrorActionPreference = "Stop"
$Root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$Book = Join-Path $Root "book"
$Out = Join-Path $Book "output\chm"
$Hhp = Join-Path $Out "tact-book.hhp"
$Chm = Join-Path $Out "tact-book.chm"

function Find-Hhc {
    $candidates = @(
        "${env:ProgramFiles(x86)}\HTML Help Workshop\hhc.exe",
        "$env:ProgramFiles\HTML Help Workshop\hhc.exe"
    )
    foreach ($p in $candidates) {
        if (Test-Path $p) { return $p }
    }
    $cmd = Get-Command hhc -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

if (-not $SkipConvert) {
    $bash = Get-Command bash -ErrorAction SilentlyContinue
    if (-not $bash) {
        Write-Error "bash not found. Install Git for Windows, or run build-chm.sh from WSL first."
    }
    Write-Host "[book] Running build-chm.sh (Markdown -> HTML)..."
    & $bash.Source (Join-Path $Book "scripts\build-chm.sh")
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

if (-not (Test-Path $Hhp)) {
    Write-Error "Missing $Hhp — run build-chm.sh first."
}

$hhc = Find-Hhc
if (-not $hhc) {
    Write-Error @"
hhc.exe not found. Install HTML Help Workshop:
  https://learn.microsoft.com/en-us/previous-versions/windows/desktop/htmlhelp/microsoft-html-help-downloads
Then re-run this script.
"@
}

Write-Host "[book] Compiling tact-book.chm with $hhc"
Push-Location $Out
try {
    & $hhc "tact-book.hhp"
} finally {
    Pop-Location
}

if (Test-Path $Chm) {
    Write-Host "[book] OK $Chm"
} else {
    Write-Error "Compilation failed — tact-book.chm was not created."
}
