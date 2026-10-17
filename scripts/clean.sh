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
