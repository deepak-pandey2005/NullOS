#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Run build
"${SCRIPT_DIR}/build.sh"

DISK_IMG="${ROOT_DIR}/build/disk.img"
DATA_DISK_IMG="${ROOT_DIR}/build/data.img"
SERIAL_LOG="${ROOT_DIR}/build/serial.log"
rm -f "${SERIAL_LOG}"

# Find OVMF
OVMF_PATH=""
for path in \
    "/usr/share/edk2/x64/OVMF.4m.fd" \
    "/usr/share/edk2/x64/OVMF_CODE.4m.fd" \
    "/usr/share/ovmf/OVMF.fd" \
    "/usr/share/ovmf/x64/OVMF.fd" \
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
if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
    KVM_FLAG=(-enable-kvm -cpu host)
else
    KVM_FLAG=(-cpu qemu64)
fi

QEMU_ARGS=(
    -machine q35
    "${KVM_FLAG[@]}"
    -smp 4
    -m 512M
    -netdev user,id=net0
    -device virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56
    -bios "${OVMF_PATH}"
    -drive "format=raw,file=${DISK_IMG}"
    -drive "format=raw,file=${DATA_DISK_IMG},if=virtio"
    -serial stdio
    -no-reboot
)

if [[ "${1:-}" == "--headless" || "${1:-}" == "--test" ]]; then
    echo "==> Running QEMU in headless test mode..."
    TEST_DISK="${ROOT_DIR}/build/test_disk.img"
    TEST_DATA="${ROOT_DIR}/build/test_data.img"
    cp "${DISK_IMG}" "${TEST_DISK}"
    cp "${DATA_DISK_IMG}" "${TEST_DATA}"

    HEADLESS_ARGS=(
        -machine q35
        "${KVM_FLAG[@]}"
        -smp 4
        -m 512M
        -netdev user,id=net0
        -device virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56
        -bios "${OVMF_PATH}"
        -drive "format=raw,file=${TEST_DISK}"
        -drive "format=raw,file=${TEST_DATA},if=virtio"
        -serial stdio
        -no-reboot
        -display none
    )
    
    touch "${SERIAL_LOG}"
    qemu-system-x86_64 "${HEADLESS_ARGS[@]}" > "${SERIAL_LOG}" 2>&1 &
    QEMU_PID=$!
    
    for i in {1..80}; do
        sleep 0.5
        if grep -q -E "All shell verification commands executed successfully|All Phase 7 Advanced Shell tests passed successfully|All Phase 7 & 8 Advanced Shell and Network tests passed successfully|All Phase 7, 8 & 9 Shell, Network and SMP tests passed successfully|All Phase 7, 8, 9 & 10 Shell, Network, SMP and Security tests passed successfully" "${SERIAL_LOG}" 2>/dev/null; then
            sleep 0.5
            echo "==> Kernel reached and all automated tests passed!"
            break
        fi
        if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
            break
        fi
    done
    
    kill "${QEMU_PID}" 2>/dev/null || true
    wait "${QEMU_PID}" 2>/dev/null || true
    rm -f "${TEST_DISK}" "${TEST_DATA}"
    
    echo "==> Serial Log:"
    if [ -f "${SERIAL_LOG}" ]; then
        cat "${SERIAL_LOG}"
    fi
elif [[ "${1:-}" == "--cli" || "${1:-}" == "-n" || "${1:-}" == "--nographic" ]]; then
    echo "==> Launching QEMU in interactive terminal CLI mode (Press Ctrl+A then X to exit)..."
    QEMU_ARGS+=(-display none)
    qemu-system-x86_64 "${QEMU_ARGS[@]}"
else
    echo "==> Launching QEMU..."
    qemu-system-x86_64 "${QEMU_ARGS[@]}"
fi
