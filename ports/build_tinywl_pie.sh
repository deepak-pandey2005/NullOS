#!/usr/bin/env bash
set -e

echo "==> Building tinywl with fixed load address..."
TINYWL_SRC="ports/wlroots/tinywl/tinywl.c"
OUT_BIN="ports/bin/tinywl.elf"

rm -f "$OUT_BIN"

gcc -O2 -no-pie \
    -Iports/wlroots/include \
    -Iports/wlroots/subprojects/wayland-protocols/include \
    -Iports/build/include \
    -Iports/build/protocol \
    -Iports/build/subprojects/wayland-protocols/include \
    -I/usr/include/pixman-1 \
    -I/usr/include/libdrm \
    -DWLR_USE_UNSTABLE \
    "$TINYWL_SRC" \
    ports/build/libwlroots-0.21.a \
    -lwayland-server -lxkbcommon -lpixman-1 -lm -lrt -lpthread -ldl \
    -Wl,-Ttext-segment=0x400000 \
    -o "$OUT_BIN"

if [ -f "$OUT_BIN" ]; then
    echo "[OK] Built $OUT_BIN"
    file "$OUT_BIN"
    readelf -l "$OUT_BIN" | grep -E "LOAD|INTERP"
fi
