#!/usr/bin/env bash
#
# NullOS CI Regression Test Runner (Phase 19 - M42 Reliability)
#
# Executes end-to-end compile gates, headless QEMU regression suites (debug & release),
# AArch64 UEFI boot verification, release artifact checksum verification, and strict
# serial log regression assertions across Milestones M37–M42.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
SERIAL_LOG="${ROOT_DIR}/build/serial.log"

echo "========================================================"
echo "  NullOS CI Regression Suite (Phase 19 - M42)           "
echo "========================================================"

echo "==> [1/5] Cargo Check Gate (bootloader x86_64/aarch64 + kernel debug/release)..."
cargo check --manifest-path "${ROOT_DIR}/bootloader/Cargo.toml" --target x86_64-unknown-uefi
cargo check --manifest-path "${ROOT_DIR}/bootloader/Cargo.toml" --target aarch64-unknown-uefi
cargo check --manifest-path "${ROOT_DIR}/kernel/Cargo.toml" --target x86_64-unknown-none
cargo check --manifest-path "${ROOT_DIR}/kernel/Cargo.toml" --target x86_64-unknown-none --release

echo "==> [2/5] Running Debug QEMU Headless Regression Suite (Phases 7-19)..."
"${SCRIPT_DIR}/run.sh" --headless

echo "==> [3/5] Asserting Critical Regression Markers in Debug Serial Log..."
REQUIRED_MARKERS=(
    "Config nullos.cfg loaded"
    "Initramfs archive loaded"
    "All M40 Debugging & Observability checks passed!"
    "Testing Bootable Distribution Initramfs"
    "Testing Kernel Reliability, Unit, Integration, FS, Net, SMP, Stress, MemCorruption, Fuzzing & Soak Stability (M42)"
    "All Reliability, Fuzzing, Stress, SMP & Stability checks passed!"
    "Distribution and Reliability tests passed successfully!"
)
for marker in "${REQUIRED_MARKERS[@]}"; do
    if ! grep -Fq "${marker}" "${SERIAL_LOG}"; then
        echo "ERROR: Missing regression marker in ${SERIAL_LOG}: '${marker}'" >&2
        exit 1
    fi
done
echo "==> All debug serial regression markers verified."

echo "==> [4/5] Running Release Build + OVMF Release Boot + AArch64 AAVMF Smoke Check..."
"${SCRIPT_DIR}/release.sh" --test

echo "==> [5/5] Verifying Release GPT + FAT32 USB Image Layout..."
"${SCRIPT_DIR}/create_bootable_usb.sh" --verify --release

echo "========================================================"
echo "  NullOS CI Regression Suite PASSED cleanly!            "
echo "========================================================"

