#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

MODE="gui"
PROFILE="debug"
NO_BUILD=0

for arg in "$@"; do
    case "$arg" in
        --headless|--test)
            MODE="headless"
            ;;
        --cli|-n|--nographic)
            MODE="cli"
            ;;
        --aarch64-uefi)
            MODE="aarch64"
            ;;
        --release)
            PROFILE="release"
            ;;
        --no-build)
            NO_BUILD=1
            ;;
        -h|--help)
            echo "Usage: $0 [--headless|--test|--cli|--aarch64-uefi] [--release] [--no-build]"
            exit 0
            ;;
        *)
            echo "Unknown option: $arg" >&2
            exit 1
            ;;
    esac
done

# Run build unless skipped
if [[ "${NO_BUILD}" -eq 0 ]]; then
    if [[ "${PROFILE}" == "release" ]]; then
        "${SCRIPT_DIR}/build.sh" --release
    else
        "${SCRIPT_DIR}/build.sh"
    fi
fi

if [[ "${PROFILE}" == "release" && -f "${ROOT_DIR}/build/release/nullos-0.1.0-x86_64.img" && "${NO_BUILD}" -eq 1 ]]; then
    DISK_IMG="${ROOT_DIR}/build/release/nullos-0.1.0-x86_64.img"
    DATA_DISK_IMG="${ROOT_DIR}/build/release/nullos-0.1.0-data.fat32.img"
else
    DISK_IMG="${ROOT_DIR}/build/disk.img"
    DATA_DISK_IMG="${ROOT_DIR}/build/data.img"
fi
NVME_DISK_IMG="${ROOT_DIR}/build/nvme.img"
SERIAL_LOG="${ROOT_DIR}/build/serial.log"
rm -f "${SERIAL_LOG}"

if [[ "${MODE}" == "aarch64" ]]; then
    AAVMF_CODE=""
    for path in \
        "/usr/share/AAVMF/AAVMF_CODE.fd" \
        "/usr/share/edk2/aarch64/QEMU_EFI-pflash.raw" \
        "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd"; do
        if [ -f "$path" ]; then
            AAVMF_CODE="$path"
            break
        fi
    done
    if [ -z "${AAVMF_CODE}" ]; then
        echo "ERROR: AArch64 AAVMF firmware not found!" >&2
        exit 1
    fi
    AA64_LOG="${ROOT_DIR}/build/aarch64_uefi.log"
    AA64_DISK="${ROOT_DIR}/build/test_aa64_disk.img"
    cp "${DISK_IMG}" "${AA64_DISK}"
    echo "==> Using AAVMF: ${AAVMF_CODE}"
    echo "==> Booting BOOTAA64.EFI in QEMU AArch64 (virt, headless smoke test)..."
    rm -f "${AA64_LOG}"
    touch "${AA64_LOG}"
    timeout 30s qemu-system-aarch64 \
        -machine virt \
        -cpu cortex-a57 \
        -m 512M \
        -bios "${AAVMF_CODE}" \
        -drive "if=none,id=hd0,format=raw,file=${AA64_DISK}" \
        -device virtio-blk-pci,drive=hd0 \
        -serial stdio \
        -display none \
        -no-reboot > "${AA64_LOG}" 2>&1 &
    AA64_PID=$!

    AA64_OK=false
    for i in {1..60}; do
        sleep 0.5
        if grep -q "AArch64 UEFI" "${AA64_LOG}" 2>/dev/null; then
            sleep 0.5
            AA64_OK=true
            break
        fi
        if ! kill -0 "${AA64_PID}" 2>/dev/null; then
            break
        fi
    done
    kill "${AA64_PID}" 2>/dev/null || true
    wait "${AA64_PID}" 2>/dev/null || true
    rm -f "${AA64_DISK}"
    echo "==> AArch64 UEFI Log:"
    cat "${AA64_LOG}"
    if [ "${AA64_OK}" != "true" ]; then
        echo "ERROR: BOOTAA64.EFI did not reach NullOS AArch64 UEFI banner!" >&2
        exit 1
    fi
    echo "==> AArch64 UEFI smoke test passed!"
    exit 0
fi

# Find OVMF
OVMF_PATH=""
for path in \
    "/usr/share/edk2/x64/OVMF.4m.fd" \
    "/usr/share/edk2/x64/OVMF_CODE.4m.fd" \
    "/usr/share/ovmf/OVMF.fd" \
    "/usr/share/ovmf/x64/OVMF.fd" \
    "/usr/share/OVMF/OVMF.fd" \
    "/usr/share/OVMF/x64/OVMF.fd" \
    "/usr/share/edk2-ovmf/x64/OVMF.fd"; do
    if [ -f "$path" ]; then
        OVMF_PATH="$path"
        break
    fi
done

if [ -z "${OVMF_PATH}" ]; then
    echo "ERROR: OVMF firmware not found!" >&2
    exit 1
fi

echo "==> Using OVMF: ${OVMF_PATH}"

KVM_FLAG=()
if [ -r /dev/kvm ] && [ -w /dev/kvm ] && qemu-system-x86_64 -accel kvm -version > /dev/null 2>&1; then
    KVM_FLAG=(-accel kvm -cpu host)
else
    KVM_FLAG=(-accel tcg,thread=multi -cpu max)
fi

QEMU_ARGS=(
    -machine q35
    "${KVM_FLAG[@]}"
    -smp 4
    -m 512M
    -netdev user,id=net0
    -device virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56
    -device qemu-xhci,id=xhci
    -device usb-kbd,bus=xhci.0
    -device usb-mouse,bus=xhci.0
    -audiodev none,id=snd0
    -device intel-hda,id=hda
    -device hda-duplex,bus=hda.0,audiodev=snd0
    -bios "${OVMF_PATH}"
    -drive "format=raw,file=${DISK_IMG}"
    -drive "format=raw,file=${DATA_DISK_IMG},if=virtio"
    -drive "if=none,id=nvmedrv,format=raw,file=${NVME_DISK_IMG}"
    -device nvme,serial=NULLOS-NVME-01,drive=nvmedrv
    -serial stdio
    -no-reboot
)

if [[ "${MODE}" == "headless" ]]; then
    echo "==> Running QEMU in headless test mode..."
    TEST_DISK="${ROOT_DIR}/build/test_disk.img"
    TEST_DATA="${ROOT_DIR}/build/test_data.img"
    TEST_NVME="${ROOT_DIR}/build/test_nvme.img"
    cp "${DISK_IMG}" "${TEST_DISK}"
    cp "${DATA_DISK_IMG}" "${TEST_DATA}"
    cp "${NVME_DISK_IMG}" "${TEST_NVME}"

    HEADLESS_ARGS=(
        -machine q35
        "${KVM_FLAG[@]}"
        -smp 4
        -m 512M
        -netdev user,id=net0
        -device virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56
        -device qemu-xhci,id=xhci
        -device usb-kbd,bus=xhci.0
        -device usb-mouse,bus=xhci.0
        -audiodev none,id=snd0
        -device intel-hda,id=hda
        -device hda-duplex,bus=hda.0,audiodev=snd0
        -bios "${OVMF_PATH}"
        -drive "format=raw,file=${TEST_DISK}"
        -drive "format=raw,file=${TEST_DATA},if=virtio"
        -drive "if=none,id=nvmedrv,format=raw,file=${TEST_NVME}"
        -device nvme,serial=NULLOS-NVME-01,drive=nvmedrv
        -serial stdio
        -no-reboot
        -display none
        -fw_cfg name=opt/test,string=1
    )
    
    touch "${SERIAL_LOG}"
    qemu-system-x86_64 "${HEADLESS_ARGS[@]}" > "${SERIAL_LOG}" 2>&1 &
    QEMU_PID=$!
    
    TEST_PASSED=false
    for i in {1..360}; do
        sleep 0.5
        if grep -q -E "All shell verification commands executed successfully|All Phase 7.*passed successfully" "${SERIAL_LOG}" 2>/dev/null; then
            sleep 0.5
            echo "==> Kernel reached and all automated tests passed!"
            TEST_PASSED=true
            break
        fi
        if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
            break
        fi
    done
    
    kill "${QEMU_PID}" 2>/dev/null || true
    wait "${QEMU_PID}" 2>/dev/null || true
    rm -f "${TEST_DISK}" "${TEST_DATA}" "${TEST_NVME}"
    
    echo "==> Serial Log:"
    if [ -f "${SERIAL_LOG}" ]; then
        cat "${SERIAL_LOG}"
    fi

    if [ "${TEST_PASSED}" != "true" ]; then
        echo "ERROR: Automated tests failed or timed out!" >&2
        exit 1
    fi
elif [[ "${MODE}" == "cli" ]]; then
    echo "==> Launching QEMU in interactive terminal CLI mode (Press Ctrl+A then X to exit)..."
    QEMU_ARGS+=(-display none)
    qemu-system-x86_64 "${QEMU_ARGS[@]}"
else
    echo "==> Launching QEMU..."
    qemu-system-x86_64 "${QEMU_ARGS[@]}"
fi
