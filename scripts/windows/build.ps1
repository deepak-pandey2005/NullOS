<#
.SYNOPSIS
    Builds NullOS bootloader, userspace, and kernel on Windows.
.DESCRIPTION
    Compiles the UEFI bootloader (x86_64-unknown-uefi), userspace binaries
    (if make/gcc are available), and the kernel (x86_64-unknown-none).
    Prepares the ESP folder structure and data disks for QEMU.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Split-Path -Parent (Split-Path -Parent $ScriptDir)
$BuildDir = Join-Path $RootDir "build"
$EspDir = Join-Path $BuildDir "esp"
$DataDir = Join-Path $BuildDir "data"

Write-Host "==> Checking Rust toolchain and targets..." -ForegroundColor Cyan

# Check rustup targets
$installedTargets = rustup target list --installed
if ($installedTargets -notcontains "x86_64-unknown-uefi") {
    Write-Host "==> Adding target x86_64-unknown-uefi..." -ForegroundColor Yellow
    rustup target add x86_64-unknown-uefi
}
if ($installedTargets -notcontains "x86_64-unknown-none") {
    Write-Host "==> Adding target x86_64-unknown-none..." -ForegroundColor Yellow
    rustup target add x86_64-unknown-none
}

# Check rust-src component
$installedComponents = rustup component list --installed
if ($installedComponents -notmatch "rust-src") {
    Write-Host "==> Adding component rust-src..." -ForegroundColor Yellow
    rustup component add rust-src
}

Write-Host "==> Building Bootloader (x86_64-unknown-uefi)..." -ForegroundColor Cyan
cargo build --manifest-path (Join-Path $RootDir "bootloader\Cargo.toml") --target x86_64-unknown-uefi
if ($LASTEXITCODE -ne 0) {
    Write-Error "Bootloader build failed with exit code $LASTEXITCODE"
    exit $LASTEXITCODE
}

Write-Host "==> Checking Userspace Libc and Binaries..." -ForegroundColor Cyan
$hasMake = Get-Command "make" -ErrorAction SilentlyContinue
$hasGcc = Get-Command "gcc" -ErrorAction SilentlyContinue

if ($hasMake -and $hasGcc) {
    Write-Host "==> Building Userspace Libc and Binaries using make/gcc..." -ForegroundColor Cyan
    & make -C (Join-Path $RootDir "user")
} else {
    Write-Host "==> [NOTICE] 'make' or 'gcc' not found in PATH; skipping userspace C binaries." -ForegroundColor Yellow
    Write-Host "    (Kernel and shell will boot normally without userland binaries)." -ForegroundColor Yellow
}

Write-Host "==> Building Kernel (x86_64-unknown-none)..." -ForegroundColor Cyan
cargo build --manifest-path (Join-Path $RootDir "kernel\Cargo.toml") --target x86_64-unknown-none
if ($LASTEXITCODE -ne 0) {
    Write-Error "Kernel build failed with exit code $LASTEXITCODE"
    exit $LASTEXITCODE
}

# Setup build directories
New-Item -ItemType Directory -Force -Path (Join-Path $EspDir "EFI\BOOT") | Out-Null
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null

$BootloaderBin = Join-Path $RootDir "target\x86_64-unknown-uefi\debug\bootloader.efi"
$KernelBin = Join-Path $RootDir "target\x86_64-unknown-none\debug\kernel"

if (-not (Test-Path $BootloaderBin)) {
    Write-Error "Bootloader binary not found at $BootloaderBin"
    exit 1
}
if (-not (Test-Path $KernelBin)) {
    Write-Error "Kernel binary not found at $KernelBin"
    exit 1
}

Copy-Item -Force $BootloaderBin (Join-Path $EspDir "EFI\BOOT\BOOTX64.EFI")
Copy-Item -Force $KernelBin (Join-Path $EspDir "kernel.elf")

# Create startup.nsh
Set-Content -Path (Join-Path $EspDir "startup.nsh") -Value "\EFI\BOOT\BOOTX64.EFI`r`n" -NoNewline

# Populate Data directory with initial sample files
$ReadmeFile = Join-Path $DataDir "README.TXT"
if (-not (Test-Path $ReadmeFile)) {
    Set-Content -Path $ReadmeFile -Value "Welcome to NullOS Persistent Storage on Windows!`r`n"
}
$NotesFile = Join-Path $DataDir "NOTES.TXT"
if (-not (Test-Path $NotesFile)) {
    Set-Content -Path $NotesFile -Value "This storage directory is mapped into NullOS via VirtIO block emulation.`r`n"
}

# Copy compiled user ELF binaries into data directory if present
$UserBinDir = Join-Path $RootDir "user\build\bin"
if (Test-Path (Join-Path $UserBinDir "hello.elf")) {
    Copy-Item -Force (Join-Path $UserBinDir "hello.elf") (Join-Path $DataDir "HELLO.ELF")
}
if (Test-Path (Join-Path $UserBinDir "cat.elf")) {
    Copy-Item -Force (Join-Path $UserBinDir "cat.elf") (Join-Path $DataDir "CAT.ELF")
}
if (Test-Path (Join-Path $UserBinDir "threads.elf")) {
    Copy-Item -Force (Join-Path $UserBinDir "threads.elf") (Join-Path $DataDir "THREADS.ELF")
}

Write-Host "==> NullOS build complete! Boot files ready in: $EspDir" -ForegroundColor Green

