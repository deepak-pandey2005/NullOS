<#
.SYNOPSIS
    Cleans NullOS build artifacts and temporary files on Windows.
.PARAMETER All
    Also cleans Cargo target directory and userland build artifacts.
#>
[CmdletBinding()]
param(
    [switch]$All
)

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Split-Path -Parent (Split-Path -Parent $ScriptDir)
$BuildDir = Join-Path $RootDir "build"

Write-Host "==> Cleaning NullOS build directory..." -ForegroundColor Cyan

if (Test-Path $BuildDir) {
    Remove-Item -Recurse -Force $BuildDir
}

if ($All) {
    Write-Host "==> Performing deep clean (cargo clean & userland clean)..." -ForegroundColor Cyan
    cargo clean --manifest-path (Join-Path $RootDir "bootloader\Cargo.toml")
    cargo clean --manifest-path (Join-Path $RootDir "kernel\Cargo.toml")
    
    $UserBuildDir = Join-Path $RootDir "user\build"
    if (Test-Path $UserBuildDir) {
        Remove-Item -Recurse -Force $UserBuildDir
    }
    $UserLibcBuild = Join-Path $RootDir "user\libc\build"
    if (Test-Path $UserLibcBuild) {
        Remove-Item -Recurse -Force $UserLibcBuild
    }
}

Write-Host "==> Clean complete." -ForegroundColor Green

