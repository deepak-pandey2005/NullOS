#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

echo "==> Cleaning NullOS build artifacts and temporary files..."

# Clean build directory (disk.img, esp.part, esp mount directory, serial logs)
if [ -d "${ROOT_DIR}/build" ]; then
    echo "  -> Removing build/ directory..."
    rm -rf "${ROOT_DIR}/build"
fi

# Clean userspace libc and user binaries (user/build/, user/libc/build/)
if [ -f "${ROOT_DIR}/user/Makefile" ]; then
    echo "  -> Cleaning userspace libc and binaries..."
    make -C "${ROOT_DIR}/user" clean > /dev/null 2>&1 || true
    rm -rf "${ROOT_DIR}/user/build" "${ROOT_DIR}/user/libc/build"
fi

# Clean SDK sysroot, examples, and templates
if [ -d "${ROOT_DIR}/sdk" ]; then
    echo "  -> Cleaning SDK sysroot and demo packages..."
    rm -rf "${ROOT_DIR}/sdk/sysroot"
    for ex_dir in "${ROOT_DIR}/sdk/examples"/* "${ROOT_DIR}/sdk/templates"/*; do
        if [ -d "${ex_dir}/build" ]; then
            rm -rf "${ex_dir}/build"
        fi
        if [ -f "${ex_dir}/Makefile" ]; then
            make -C "${ex_dir}" clean > /dev/null 2>&1 || true
        fi
    done
fi

# Clean Rust target directory if requested via --all or -a
if [[ "${1:-}" == "--all" || "${1:-}" == "-a" ]]; then
    echo "  -> Cleaning Cargo build targets..."
    (cd "${ROOT_DIR}" && cargo clean)
fi

# Remove stray editor or OS temporary files
echo "  -> Removing temporary and backup files..."
find "${ROOT_DIR}" -type f \( \
    -name "*~" -o \
    -name "*.swp" -o \
    -name "*.swo" -o \
    -name ".DS_Store" -o \
    -name "*.tmp" -o \
    -name "*.log" \
\) -not -path "*/.git/*" -delete

echo "==> Clean complete."
echo "    Tip: Run './scripts/clean.sh --all' to also clear Cargo target caches."
