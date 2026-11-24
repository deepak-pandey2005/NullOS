#!/usr/bin/env bash
set -e

echo "==> Configuring tinywl build environment..."
HOST_CC="gcc"
MUSL_CC="musl-gcc"

echo "Compiler available: $MUSL_CC"
