<#
.SYNOPSIS
    Builds and runs NullOS in QEMU on Windows.
.DESCRIPTION
    Locates QEMU, OVMF UEFI firmware, builds the OS (unless -NoBuild is specified),
    and launches QEMU with all necessary hardware and display configurations.
.PARAMETER Headless
    Runs QEMU in headless test mode and asserts automated tests pass.
.PARAMETER Test
    Alias for -Headless.
.PARAMETER NoBuild
    Skips the build step and runs existing binaries.
.PARAMETER QemuPath
    Explicit path to qemu-system-x86_64.exe.
.PARAMETER OvmfPath
    Explicit path to OVMF.fd / OVMF_CODE.fd firmware.
#>
[CmdletBinding()]
param(
    [switch]$Headless,
    [switch]$Test,
    [switch]$NoBuild,
    [string]$QemuPath,
    [string]$OvmfPath
)

$ErrorActionPreference = "Stop"

if ($Test) {
    $Headless = $true
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Split-Path -Parent $ScriptDir
$BuildDir = Join-Path $RootDir "build"
$EspDir = Join-Path $BuildDir "esp"
$DataDir = Join-Path $BuildDir "data"
$DiskImg = Join-Path $BuildDir "disk.img"
$DataDiskImg = Join-Path $BuildDir "data.img"
$SerialLog = Join-Path $BuildDir "serial.log"

# Step 1: Run build unless -NoBuild was passed
if (-not $NoBuild) {
    & (Join-Path $ScriptDir "build.ps1")
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Build step failed. Aborting QEMU run."
        exit $LASTEXITCODE
    }
}

# Step 2: Locate QEMU executable
if (-not $QemuPath) {
    $qemuCmd = Get-Command "qemu-system-x86_64" -ErrorAction SilentlyContinue
    if ($qemuCmd) {
        $QemuPath = $qemuCmd.Source
    } else {
        $candidateQemuPaths = @(
            "$env:ProgramFiles\qemu\qemu-system-x86_64.exe",
            "$env:LOCALAPPDATA\Programs\qemu\qemu-system-x86_64.exe",
            "C:\Program Files\qemu\qemu-system-x86_64.exe",
            "C:\qemu\qemu-system-x86_64.exe",
            "$env:USERPROFILE\scoop\apps\qemu\current\qemu-system-x86_64.exe",
            "C:\tools\qemu\qemu-system-x86_64.exe"
        )
        foreach ($p in $candidateQemuPaths) {
            if (Test-Path $p) {
                $QemuPath = $p
                break
            }
        }
    }
}

if (-not $QemuPath -or -not (Test-Path $QemuPath)) {
    Write-Host "ERROR: QEMU (qemu-system-x86_64.exe) not found!" -ForegroundColor Red
    Write-Host "Please install QEMU using winget or Chocolatey:" -ForegroundColor Yellow
    Write-Host "  winget install SoftwareFreedomConservancy.QEMU" -ForegroundColor Yellow
    Write-Host "Or specify its path with -QemuPath 'C:\path\to\qemu-system-x86_64.exe'" -ForegroundColor Yellow
    exit 1
}

Write-Host "==> Using QEMU: $QemuPath" -ForegroundColor Cyan

# Step 3: Locate OVMF UEFI firmware
if (-not $OvmfPath) {
    $candidateOvmfPaths = @(
        (Join-Path $BuildDir "OVMF.fd"),
        (Join-Path $BuildDir "OVMF_CODE.fd"),
        "$env:ProgramFiles\qemu\share\edk2-x86_64-code.fd",
        "$env:ProgramFiles\qemu\share\OVMF.fd",
        "$env:ProgramFiles\qemu\OVMF.fd",
        "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
        "C:\Program Files\qemu\share\OVMF.fd",
        "$env:USERPROFILE\scoop\apps\qemu\current\share\edk2-x86_64-code.fd",
        "C:\msys64\mingw64\share\qemu\edk2-x86_64-code.fd",
        "C:\msys64\usr\share\ovmf\OVMF.fd"
    )
    foreach ($p in $candidateOvmfPaths) {
        if (Test-Path $p) {
            $OvmfPath = $p
            break
        }
    }
}

# If still not found, automatically download OVMF.fd to build/OVMF.fd
if (-not $OvmfPath -or -not (Test-Path $OvmfPath)) {
    $TargetOvmf = Join-Path $BuildDir "OVMF.fd"
    Write-Host "==> OVMF UEFI firmware not found locally. Downloading to $TargetOvmf..." -ForegroundColor Yellow
    try {
        $OvmfUrl = "https://raw.githubusercontent.com/clearlinux/common/master/OVMF.fd"
        New-Item -ItemType Directory -Force -Path $BuildDir | Out-Null
        Invoke-WebRequest -Uri $OvmfUrl -OutFile $TargetOvmf -UseBasicParsing
        $OvmfPath = $TargetOvmf
        Write-Host "==> Downloaded OVMF firmware successfully!" -ForegroundColor Green
    } catch {
        Write-Host "WARNING: Failed to download OVMF automatically. $_" -ForegroundColor Red
        Write-Host "Please download OVMF.fd and place it in the 'build' folder or pass -OvmfPath." -ForegroundColor Yellow
        exit 1
    }
}

Write-Host "==> Using OVMF: $OvmfPath" -ForegroundColor Cyan

# Step 4: Configure boot drive
# Prefer full GPT disk image if present; otherwise use QEMU on-the-fly FAT directory mount
if (Test-Path $DiskImg) {
    $BootDrive = "format=raw,file=$DiskImg"
} else {
    $BootDrive = "format=raw,file=fat:rw:$EspDir"
}

# Data drive
$DataDriveArg = @()
if (Test-Path $DataDiskImg) {
    $DataDriveArg = @("-drive", "format=raw,file=$DataDiskImg,if=virtio")
} elseif (Test-Path $DataDir) {
    $DataDriveArg = @("-drive", "format=raw,file=fat:rw:$DataDir,if=virtio")
}

# Base QEMU arguments
$QemuArgs = @(
    "-machine", "q35",
    "-accel", "tcg,thread=multi",
    "-cpu", "max",
    "-smp", "4",
    "-m", "512M",
    "-netdev", "user,id=net0",
    "-device", "virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56",
    "-bios", $OvmfPath,
    "-drive", $BootDrive
) + $DataDriveArg + @(
    "-serial", "stdio",
    "-no-reboot"
)

if ($Headless) {
    Write-Host "==> Running QEMU in headless automated test mode..." -ForegroundColor Cyan
    $QemuArgs += @("-display", "none")

    if (Test-Path $SerialLog) {
        Remove-Item -Force $SerialLog
    }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $QemuPath
    $psi.Arguments = [string]::Join(" ", ($QemuArgs | ForEach-Object {
        if ($_ -match "[\s,]") { "`"$_`"" } else { $_ }
    }))
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true

    $proc = [System.Diagnostics.Process]::Start($psi)

    $outputBuilder = New-Object System.Text.StringBuilder
    $testPassed = $false
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $timeoutSeconds = 180

    while (-not $proc.HasExited -and $timer.Elapsed.TotalSeconds -lt $timeoutSeconds) {
        Start-Sleep -Milliseconds 500
        while (-not $proc.StandardOutput.EndOfStream) {
            $line = $proc.StandardOutput.ReadLine()
            Write-Host $line
            [void]$outputBuilder.AppendLine($line)
            if ($line -match "All shell verification commands executed successfully|All Phase 7.*passed successfully") {
                $testPassed = $true
                break
            }
        }
        if ($testPassed) {
            break
        }
    }

    if (-not $proc.HasExited) {
        $proc.Kill()
    }

    # Save log
    Set-Content -Path $SerialLog -Value $outputBuilder.ToString()

    if ($testPassed) {
        Write-Host "==> All automated tests passed successfully on Windows!" -ForegroundColor Green
        exit 0
    } else {
        Write-Host "ERROR: Automated tests failed or timed out!" -ForegroundColor Red
        exit 1
    }
} else {
    Write-Host "==> Starting NullOS Graphical QEMU session..." -ForegroundColor Green
    & $QemuPath $QemuArgs
}
