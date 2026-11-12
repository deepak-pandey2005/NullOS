#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

PROFILE="debug"
CARGO_PROFILE_FLAG=()
for arg in "$@"; do
    case "$arg" in
        --release)
            PROFILE="release"
            CARGO_PROFILE_FLAG=(--release)
            ;;
        --debug)
            PROFILE="debug"
            CARGO_PROFILE_FLAG=()
            ;;
        *)
            echo "Unknown option: $arg" >&2
            echo "Usage: $0 [--debug|--release]" >&2
            exit 1
            ;;
    esac
done

echo "==> Building Bootloader (x86_64-unknown-uefi, profile=${PROFILE})..."
cargo build --manifest-path "${ROOT_DIR}/bootloader/Cargo.toml" --target x86_64-unknown-uefi "${CARGO_PROFILE_FLAG[@]}"

echo "==> Building Bootloader (aarch64-unknown-uefi, profile=${PROFILE})..."
cargo build --manifest-path "${ROOT_DIR}/bootloader/Cargo.toml" --target aarch64-unknown-uefi "${CARGO_PROFILE_FLAG[@]}"

echo "==> Building Userspace Libc and Binaries..."
make -C "${ROOT_DIR}/user"

echo "==> Building NullOS SDK Sysroot & Demo Package..."
"${ROOT_DIR}/sdk/bin/nullos-sdk" check

echo "==> Building Kernel (x86_64-unknown-none, profile=${PROFILE})..."
cargo build --manifest-path "${ROOT_DIR}/kernel/Cargo.toml" --target x86_64-unknown-none "${CARGO_PROFILE_FLAG[@]}"

BUILD_DIR="${ROOT_DIR}/build"
ESP_DIR="${BUILD_DIR}/esp"
ESP_PART="${BUILD_DIR}/esp.part"
DISK_IMG="${BUILD_DIR}/disk.img"
INITRAMFS_STAGE="${BUILD_DIR}/initramfs_root"
INITRAMFS_TAR="${BUILD_DIR}/initramfs.tar"

mkdir -p "${ESP_DIR}/EFI/BOOT"

# Copy binaries into esp directory
cp "${ROOT_DIR}/target/x86_64-unknown-uefi/${PROFILE}/bootloader.efi" "${ESP_DIR}/EFI/BOOT/BOOTX64.EFI"
cp "${ROOT_DIR}/target/aarch64-unknown-uefi/${PROFILE}/bootloader.efi" "${ESP_DIR}/EFI/BOOT/BOOTAA64.EFI"
cp "${ROOT_DIR}/target/x86_64-unknown-none/${PROFILE}/kernel" "${ESP_DIR}/kernel.elf"

# Generate boot configuration (nullos.cfg)
cat > "${ESP_DIR}/nullos.cfg" <<EOF
# NullOS UEFI Boot Configuration
version=0.1.0
profile=${PROFILE}
kernel=\kernel.elf
initrd=\initramfs.tar
cmdline=console=com1 loglevel=info root=ramfs profile=${PROFILE}
timeout=0
EOF
cp "${ESP_DIR}/nullos.cfg" "${ESP_DIR}/EFI/BOOT/nullos.cfg"

# Stage and pack USTAR initramfs archive (initramfs.tar)
rm -rf "${INITRAMFS_STAGE}" "${INITRAMFS_TAR}"
mkdir -p "${INITRAMFS_STAGE}/bin" "${INITRAMFS_STAGE}/lib" "${INITRAMFS_STAGE}/etc" "${INITRAMFS_STAGE}/pkg"

for bin_name in hello.elf cat.elf threads.elf async_test.elf dyn_test.elf dyn_hello.elf gui_demo.elf compositor.elf; do
    if [ -f "${ROOT_DIR}/user/build/bin/${bin_name}" ]; then
        cp "${ROOT_DIR}/user/build/bin/${bin_name}" "${INITRAMFS_STAGE}/bin/${bin_name}"
    fi
done
if [ -f "${ROOT_DIR}/sdk/examples/sdk_demo/build/sdk_demo.elf" ]; then
    cp "${ROOT_DIR}/sdk/examples/sdk_demo/build/sdk_demo.elf" "${INITRAMFS_STAGE}/bin/sdk_demo.elf"
fi
if [ -f "${ROOT_DIR}/user/libc/build/libc.so" ]; then
    cp "${ROOT_DIR}/user/libc/build/libc.so" "${INITRAMFS_STAGE}/lib/libc.so"
fi
if [ -f "${ROOT_DIR}/sdk/examples/sdk_demo/build/sdk_demo.npkg" ]; then
    cp "${ROOT_DIR}/sdk/examples/sdk_demo/build/sdk_demo.npkg" "${INITRAMFS_STAGE}/pkg/sdk_demo.npkg"
fi

cat > "${INITRAMFS_STAGE}/etc/nullos-release" <<EOF
NAME="NullOS"
VERSION="0.1.0"
ARCH="x86_64"
PROFILE="${PROFILE}"
BOOT="UEFI GPT+FAT32"
EOF
cp "${ESP_DIR}/nullos.cfg" "${INITRAMFS_STAGE}/etc/nullos.cfg"

tar --format=ustar --numeric-owner --owner=0 --group=0 -cf "${INITRAMFS_TAR}" -C "${INITRAMFS_STAGE}" bin etc lib pkg
cp "${INITRAMFS_TAR}" "${ESP_DIR}/initramfs.tar"
echo "==> Packed USTAR initramfs archive at ${INITRAMFS_TAR}"

# Create UEFI Shell startup script
printf "\\EFI\\BOOT\\BOOTX64.EFI\r\n" > "${ESP_DIR}/startup.nsh"

# 1. Create a 62MB FAT32 EFI System Partition image
rm -f "${ESP_PART}"
truncate -s 62M "${ESP_PART}"
mkfs.fat -F32 -n "NULLOS_ESP" "${ESP_PART}" > /dev/null

mmd -i "${ESP_PART}" ::EFI
mmd -i "${ESP_PART}" ::EFI/BOOT
mcopy -i "${ESP_PART}" "${ESP_DIR}/EFI/BOOT/BOOTX64.EFI" ::EFI/BOOT/BOOTX64.EFI
mcopy -i "${ESP_PART}" "${ESP_DIR}/EFI/BOOT/BOOTAA64.EFI" ::EFI/BOOT/BOOTAA64.EFI
mcopy -i "${ESP_PART}" "${ESP_DIR}/EFI/BOOT/nullos.cfg" ::EFI/BOOT/nullos.cfg
mcopy -i "${ESP_PART}" "${ESP_DIR}/kernel.elf" ::kernel.elf
mcopy -i "${ESP_PART}" "${ESP_DIR}/initramfs.tar" ::initramfs.tar
mcopy -i "${ESP_PART}" "${ESP_DIR}/nullos.cfg" ::nullos.cfg
mcopy -i "${ESP_PART}" "${ESP_DIR}/startup.nsh" ::startup.nsh

# 2. Create a 64MB GPT disk image with EFI System Partition (ESP)
rm -f "${DISK_IMG}"
truncate -s 64M "${DISK_IMG}"
parted -s "${DISK_IMG}" mklabel gpt
parted -s "${DISK_IMG}" mkpart ESP fat32 1MiB 63MiB
parted -s "${DISK_IMG}" set 1 esp on
dd if="${ESP_PART}" of="${DISK_IMG}" bs=1M seek=1 conv=notrunc status=none

echo "==> GPT disk image with multi-arch UEFI ESP created at ${DISK_IMG}"

# 3. Create a 36MB FAT32 VirtIO block data disk image if not present
DATA_DISK_IMG="${BUILD_DIR}/data.img"
if [ ! -f "${DATA_DISK_IMG}" ]; then
    truncate -s 36M "${DATA_DISK_IMG}"
    mkfs.fat -F 32 -s 1 -n "NULLOS_DATA" "${DATA_DISK_IMG}" > /dev/null
    
    # Pre-populate with initial test files
    TEMP_NOTE="${BUILD_DIR}/temp_note.txt"
    echo "Welcome to NullOS Persistent FAT32 Storage!" > "${TEMP_NOTE}"
    mcopy -i "${DATA_DISK_IMG}" "${TEMP_NOTE}" ::README.TXT
    
    echo "This file was read directly from the VirtIO block device via FAT32 driver." > "${TEMP_NOTE}"
    mcopy -i "${DATA_DISK_IMG}" "${TEMP_NOTE}" ::NOTES.TXT
    rm -f "${TEMP_NOTE}"
    
    echo "==> Created 36MB FAT32 VirtIO data disk with sample files at ${DATA_DISK_IMG}"
fi

# Copy compiled user ELF binaries into persistent FAT32 VirtIO storage
if [ -f "${ROOT_DIR}/user/build/bin/hello.elf" ]; then
    mcopy -o -i "${DATA_DISK_IMG}" "${ROOT_DIR}/user/build/bin/hello.elf" ::HELLO.ELF
    echo "==> Copied HELLO.ELF to FAT32 data disk."
fi
if [ -f "${ROOT_DIR}/user/build/bin/cat.elf" ]; then
    mcopy -o -i "${DATA_DISK_IMG}" "${ROOT_DIR}/user/build/bin/cat.elf" ::CAT.ELF
    echo "==> Copied CAT.ELF to FAT32 data disk."
fi
if [ -f "${ROOT_DIR}/user/build/bin/threads.elf" ]; then
    mcopy -o -i "${DATA_DISK_IMG}" "${ROOT_DIR}/user/build/bin/threads.elf" ::THREADS.ELF
    echo "==> Copied THREADS.ELF to FAT32 data disk."
fi
if [ -f "${ROOT_DIR}/user/build/bin/async_test.elf" ]; then
    mcopy -o -i "${DATA_DISK_IMG}" "${ROOT_DIR}/user/build/bin/async_test.elf" ::ASYNC.ELF
    echo "==> Copied ASYNC.ELF to FAT32 data disk."
fi
if [ -f "${ROOT_DIR}/user/libc/build/libc.so" ]; then
    mcopy -o -i "${DATA_DISK_IMG}" "${ROOT_DIR}/user/libc/build/libc.so" ::LIBC.SO
    echo "==> Copied LIBC.SO to FAT32 data disk."
fi
if [ -f "${ROOT_DIR}/user/build/bin/dyn_test.elf" ]; then
    mcopy -o -i "${DATA_DISK_IMG}" "${ROOT_DIR}/user/build/bin/dyn_test.elf" ::DYN_TEST.ELF
    echo "==> Copied DYN_TEST.ELF to FAT32 data disk."
fi
if [ -f "${ROOT_DIR}/user/build/bin/dyn_hello.elf" ]; then
    mcopy -o -i "${DATA_DISK_IMG}" "${ROOT_DIR}/user/build/bin/dyn_hello.elf" ::DYN_HELLO.ELF
    echo "==> Copied DYN_HELLO.ELF to FAT32 data disk."
fi
if [ -f "${ROOT_DIR}/sdk/examples/sdk_demo/build/sdk_demo.elf" ]; then
    mcopy -o -i "${DATA_DISK_IMG}" "${ROOT_DIR}/sdk/examples/sdk_demo/build/sdk_demo.elf" ::SDK_DEMO.ELF
    echo "==> Copied SDK_DEMO.ELF to FAT32 data disk."
fi
if [ -f "${ROOT_DIR}/sdk/examples/sdk_demo/build/sdk_demo.npkg" ]; then
    mcopy -o -i "${DATA_DISK_IMG}" "${ROOT_DIR}/sdk/examples/sdk_demo/build/sdk_demo.npkg" ::SDKDEMO.NPK
    echo "==> Copied SDKDEMO.NPK to FAT32 data disk."
fi

# Generate compact KSYM v1 binary symbol table for M40 Kernel Symbolizer (/disk/KERNEL.SYM)
KSYM_FILE="${BUILD_DIR}/kernel.ksym"
python3 - "${ESP_DIR}/kernel.elf" "${KSYM_FILE}" <<'PYEOF'
import struct, subprocess, sys
elf_path, out_path = sys.argv[1], sys.argv[2]
nm = subprocess.check_output(["nm", "-n", "-S", "--defined-only", elf_path], text=True)
entries = []
for line in nm.splitlines():
    parts = line.split()
    if len(parts) == 4:
        addr_s, size_s, kind, name = parts
        size = int(size_s, 16)
    elif len(parts) == 3:
        addr_s, kind, name = parts
        size = 0
    else:
        continue
    if kind not in ("T", "t", "W", "w"):
        continue
    addr = int(addr_s, 16)
    nb = name.encode("utf-8", "ignore")[:240]
    entries.append((addr, size, nb))
with open(out_path, "wb") as f:
    f.write(struct.pack("<4sIII", b"KSYM", 1, len(entries), 0))
    for addr, size, nb in entries:
        f.write(struct.pack("<QQH", addr, size, len(nb)))
        f.write(nb)
PYEOF
mcopy -o -i "${DATA_DISK_IMG}" "${KSYM_FILE}" ::KERNEL.SYM
echo "==> Generated and copied KERNEL.SYM to FAT32 data disk."

# 4. Create an 8MB NVMe disk image if not present
NVME_DISK_IMG="${BUILD_DIR}/nvme.img"
if [ ! -f "${NVME_DISK_IMG}" ]; then
    truncate -s 8M "${NVME_DISK_IMG}"
    echo "==> Created 8MB NVMe backing image at ${NVME_DISK_IMG}"
fi


