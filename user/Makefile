# NullOS Standard Userspace Binaries Makefile
# Builds static userspace applications using standard musl-gcc
CC := musl-gcc

CFLAGS = -Wall -Wextra -O2 -march=x86-64 -mtune=generic -static

BUILD_DIR = build
BIN_DIR = $(BUILD_DIR)/bin

BINS = $(BIN_DIR)/hello.elf $(BIN_DIR)/cat.elf $(BIN_DIR)/threads.elf $(BIN_DIR)/async_test.elf

all: $(BINS)

$(BIN_DIR)/%.elf: bin/%.c
	@mkdir -p $(BIN_DIR)
	$(CC) $(CFLAGS) $< -o $@

clean:
	rm -rf $(BUILD_DIR)

.PHONY: all clean
