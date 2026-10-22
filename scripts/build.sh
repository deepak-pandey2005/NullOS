#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

echo "==> Building Bootloader (x86_64-unknown-uefi)..."
cargo build --manifest-path "${ROOT_DIR}/bootloader/Cargo.toml" --target x86_64-unknown-uefi

echo "==> Building Kernel (x86_64-unknown-none)..."
cargo build --manifest-path "${ROOT_DIR}/kernel/Cargo.toml" --target x86_64-unknown-none

BUILD_DIR="${ROOT_DIR}/build"
ESP_DIR="${BUILD_DIR}/esp"
ESP_PART="${BUILD_DIR}/esp.part"
DISK_IMG="${BUILD_DIR}/disk.img"

mkdir -p "${ESP_DIR}/EFI/BOOT"

# Copy binaries into esp directory
cp "${ROOT_DIR}/target/x86_64-unknown-uefi/debug/bootloader.efi" "${ESP_DIR}/EFI/BOOT/BOOTX64.EFI"
cp "${ROOT_DIR}/target/x86_64-unknown-none/debug/kernel" "${ESP_DIR}/kernel.elf"

# Create startup.nsh
printf "\\EFI\\BOOT\\BOOTX64.EFI\r\n" > "${ESP_DIR}/startup.nsh"

# 1. Create a 62MB FAT32 filesystem partition
rm -f "${ESP_PART}"
truncate -s 62M "${ESP_PART}"
mkfs.fat -F32 "${ESP_PART}" > /dev/null

mmd -i "${ESP_PART}" ::EFI
mmd -i "${ESP_PART}" ::EFI/BOOT
mcopy -i "${ESP_PART}" "${ESP_DIR}/EFI/BOOT/BOOTX64.EFI" ::EFI/BOOT/BOOTX64.EFI
mcopy -i "${ESP_PART}" "${ESP_DIR}/kernel.elf" ::kernel.elf
mcopy -i "${ESP_PART}" "${ESP_DIR}/startup.nsh" ::startup.nsh

# 2. Create a 64MB GPT disk image with EFI System Partition (ESP)
rm -f "${DISK_IMG}"
truncate -s 64M "${DISK_IMG}"
parted -s "${DISK_IMG}" mklabel gpt
parted -s "${DISK_IMG}" mkpart ESP fat32 1MiB 63MiB
parted -s "${DISK_IMG}" set 1 esp on
dd if="${ESP_PART}" of="${DISK_IMG}" bs=1M seek=1 conv=notrunc status=none

echo "==> GPT disk image with ESP created at ${DISK_IMG}"

# 3. Create a 16MB raw VirtIO block data disk image if not present
DATA_DISK_IMG="${BUILD_DIR}/data.img"
if [ ! -f "${DATA_DISK_IMG}" ]; then
    truncate -s 16M "${DATA_DISK_IMG}"
    echo "==> Created 16MB VirtIO data disk at ${DATA_DISK_IMG}"
fi
