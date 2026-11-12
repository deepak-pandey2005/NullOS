CC ?= gcc
LD ?= ld

CFLAGS = -Wall -Wextra -O2 -march=x86-64 -mtune=generic -nostdlib -fno-builtin -fno-stack-protector -mno-red-zone -Ilibc/include -mcmodel=small
LDFLAGS = -m elf_x86_64 -static -nostdlib -T linker.ld

BUILD_DIR = build
BIN_DIR = $(BUILD_DIR)/bin
OBJ_DIR = $(BUILD_DIR)/obj

LIBC_DIR = libc
LIBC = $(LIBC_DIR)/build/libc.a
LIBC_SO = $(LIBC_DIR)/build/libc.so
CRT0 = $(LIBC_DIR)/build/crt0.o

BINS = $(BIN_DIR)/hello.elf $(BIN_DIR)/cat.elf $(BIN_DIR)/threads.elf $(BIN_DIR)/async_test.elf $(BIN_DIR)/dyn_test.elf $(BIN_DIR)/dyn_hello.elf $(BIN_DIR)/gui_demo.elf

all: libc_build $(BINS)

libc_build:
	$(MAKE) -C $(LIBC_DIR)

$(OBJ_DIR)/%.o: bin/%.c
	@mkdir -p $(OBJ_DIR)
	$(CC) $(CFLAGS) -c $< -o $@

$(OBJ_DIR)/hello_dyn.o: bin/hello.c
	@mkdir -p $(OBJ_DIR)
	$(CC) $(CFLAGS) -fPIC -c $< -o $@

$(BIN_DIR)/dyn_hello.elf: $(OBJ_DIR)/hello_dyn.o $(CRT0) $(LIBC_SO)
	@mkdir -p $(BIN_DIR)
	$(LD) -m elf_x86_64 -dynamic-linker /lib/ld.so $(CRT0) $< $(LIBC_SO) -o $@

$(BIN_DIR)/%.elf: $(OBJ_DIR)/%.o $(CRT0) $(LIBC)
	@mkdir -p $(BIN_DIR)
	$(LD) $(LDFLAGS) $(CRT0) $< $(LIBC) -o $@

clean:
	$(MAKE) -C $(LIBC_DIR) clean
	rm -rf $(BUILD_DIR)

.PHONY: all libc_build clean
