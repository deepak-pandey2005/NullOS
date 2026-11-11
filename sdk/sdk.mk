# NullOS Official SDK Build System Include (Phase 18 - M39)
# Usage inside an application Makefile:
#   APP_NAME := my_app
#   SRCS     := main.c
#   include /path/to/NullOS/sdk/sdk.mk

SDK_ROOT ?= $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
NULLOS_CC ?= $(SDK_ROOT)/bin/nullos-cc
NULLOS_PKG ?= $(SDK_ROOT)/bin/nullos-pkg
NULLOS_DBG ?= $(SDK_ROOT)/bin/nullos-dbg

APP_NAME ?= app
APP_VERSION ?= 1.0.0
APP_MODE ?= static
SRCS ?= main.c
BUILD_DIR ?= build

OUT_ELF := $(BUILD_DIR)/$(APP_NAME).elf
OUT_NPKG := $(BUILD_DIR)/$(APP_NAME).npkg

SDK_MODE_FLAG := $(if $(filter dynamic,$(APP_MODE)),--dynamic,)

all: $(OUT_ELF) $(OUT_NPKG)

$(OUT_ELF): $(SRCS)
	@mkdir -p $(BUILD_DIR)
	$(NULLOS_CC) $(SDK_MODE_FLAG) $(SRCS) -o $(OUT_ELF)

$(OUT_NPKG): $(OUT_ELF)
	$(NULLOS_PKG) pack $(OUT_ELF) $(OUT_NPKG) --name $(APP_NAME) --version $(APP_VERSION) $(SDK_MODE_FLAG)

verify: $(OUT_ELF) $(OUT_NPKG)
	$(NULLOS_DBG) check $(OUT_ELF)
	$(NULLOS_PKG) verify $(OUT_NPKG)

clean:
	rm -rf $(BUILD_DIR)

.PHONY: all verify clean

