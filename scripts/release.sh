#!/usr/bin/env bash
#
# NullOS Release Distribution Builder (Phase 19 - M41 Bootable NullOS Distribution)
#
# Produces optimized release images (`build/release/`):
#   - Bootable GPT UEFI disk image (`nullos-<ver>-x86_64.img`, contains BOOTX64.EFI + BOOTAA64.EFI)
#   - Standalone FAT32 EFI System Partition (`nullos-<ver>-esp.fat32.img`)
#   - Persistent userspace/KSYM FAT32 disk (`nullos-<ver>-data.fat32.img`)
#   - Individual distribution components (`BOOTX64.EFI`, `BOOTAA64.EFI`, `kernel.elf`, `initramfs.tar`, `kernel.ksym`, `nullos.cfg`)
#   - Cryptographic integrity manifest (`SHA256SUMS`) and release summary (`RELEASE_MANIFEST.txt`)
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_DIR="${ROOT_DIR}/build"
RELEASE_DIR="${BUILD_DIR}/release"

VERSION="0.1.0"
RUN_TEST=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            VERSION="$2"
            shift 2
            ;;
        --test)
            RUN_TEST=1
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [--version <semver>] [--test]"
            echo
            echo "Options:"
            echo "  --version <ver>   Release version label (default: 0.1.0)"
            echo "  --test            Boot generated release images under QEMU OVMF & AAVMF after packaging"
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

echo "========================================================"
echo "  NullOS Release Distribution Build (v${VERSION})"
echo "========================================================"

# 1. Build release profile across bootloader (x86_64 + aarch64), userspace, SDK, kernel, and GPT image
"${SCRIPT_DIR}/build.sh" --release

# 2. Populate clean release staging directory
rm -rf "${RELEASE_DIR}"
mkdir -p "${RELEASE_DIR}"

DIST_IMG="${RELEASE_DIR}/nullos-${VERSION}-x86_64.img"
ESP_IMG="${RELEASE_DIR}/nullos-${VERSION}-esp.fat32.img"
DATA_IMG="${RELEASE_DIR}/nullos-${VERSION}-data.fat32.img"

cp "${BUILD_DIR}/disk.img" "${DIST_IMG}"
cp "${BUILD_DIR}/esp.part" "${ESP_IMG}"
cp "${BUILD_DIR}/data.img" "${DATA_IMG}"
cp "${BUILD_DIR}/esp/EFI/BOOT/BOOTX64.EFI" "${RELEASE_DIR}/BOOTX64.EFI"
cp "${BUILD_DIR}/esp/EFI/BOOT/BOOTAA64.EFI" "${RELEASE_DIR}/BOOTAA64.EFI"
cp "${BUILD_DIR}/esp/kernel.elf" "${RELEASE_DIR}/kernel.elf"
cp "${BUILD_DIR}/initramfs.tar" "${RELEASE_DIR}/initramfs.tar"
cp "${BUILD_DIR}/kernel.ksym" "${RELEASE_DIR}/kernel.ksym"
cp "${BUILD_DIR}/esp/nullos.cfg" "${RELEASE_DIR}/nullos.cfg"

# 3. Generate SHA256SUMS manifest
(
    cd "${RELEASE_DIR}"
    sha256sum \
        "nullos-${VERSION}-x86_64.img" \
        "nullos-${VERSION}-esp.fat32.img" \
        "nullos-${VERSION}-data.fat32.img" \
        BOOTX64.EFI \
        BOOTAA64.EFI \
        kernel.elf \
        initramfs.tar \
        kernel.ksym \
        nullos.cfg > SHA256SUMS
)

GIT_REV="$(git -C "${ROOT_DIR}" rev-parse --short HEAD 2>/dev/null || echo "unknown")"
BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

cat > "${RELEASE_DIR}/RELEASE_MANIFEST.txt" <<EOF
NullOS Bootable Distribution Release
=====================================
Version:      ${VERSION}
Profile:      release (--release LTO/opt)
Git Commit:   ${GIT_REV}
Built At:     ${BUILD_DATE}

UEFI Boot Images:
  - x86_64 UEFI Loader : EFI/BOOT/BOOTX64.EFI (full kernel boot)
  - AArch64 UEFI Loader: EFI/BOOT/BOOTAA64.EFI (UEFI pre-kernel verification image)

Disk Layout (nullos-${VERSION}-x86_64.img):
  - Partition Table: GPT (64 MiB raw disk image)
  - Partition 1 (ESP): FAT32 (NULLOS_ESP), 1 MiB..63 MiB (62 MiB), Boot/ESP flag enabled

Files on ESP:
  - \\EFI\\BOOT\\BOOTX64.EFI
  - \\EFI\\BOOT\\BOOTAA64.EFI
  - \\EFI\\BOOT\\nullos.cfg
  - \\kernel.elf
  - \\initramfs.tar
  - \\nullos.cfg
  - \\startup.nsh
EOF

echo "==> Release artifacts generated under ${RELEASE_DIR}:"
ls -lh "${RELEASE_DIR}"
echo
echo "==> Verifying SHA256SUMS..."
(cd "${RELEASE_DIR}" && sha256sum -c SHA256SUMS)

echo "==> Verifying release GPT & FAT32 ESP layout..."
"${SCRIPT_DIR}/create_bootable_usb.sh" --verify --image "${DIST_IMG}"

if [[ "${RUN_TEST}" -eq 1 ]]; then
    echo "==> Testing release image in QEMU x86_64 (OVMF headless)..."
    "${SCRIPT_DIR}/run.sh" --release --headless --no-build
    echo "==> Testing AArch64 UEFI bootloader image in QEMU AArch64 (AAVMF)..."
    "${SCRIPT_DIR}/run.sh" --aarch64-uefi --no-build
fi

echo "==> NullOS v${VERSION} release build complete!"

