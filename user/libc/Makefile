CC ?= gcc
AR ?= ar
AS ?= as

CFLAGS = -Wall -Wextra -O2 -nostdlib -fno-builtin -fno-stack-protector -mno-red-zone -Iinclude -mcmodel=small
ASFLAGS = --64

BUILD_DIR = build
OBJS = $(BUILD_DIR)/string.o $(BUILD_DIR)/stdio.o $(BUILD_DIR)/stdlib.o $(BUILD_DIR)/unistd.o $(BUILD_DIR)/syscall.o $(BUILD_DIR)/pthread.o $(BUILD_DIR)/semaphore.o

all: $(BUILD_DIR)/crt0.o $(BUILD_DIR)/libc.a

$(BUILD_DIR)/crt0.o: src/crt0.s
	@mkdir -p $(BUILD_DIR)
	$(AS) $(ASFLAGS) $< -o $@

$(BUILD_DIR)/%.o: src/%.c
	@mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD_DIR)/libc.a: $(OBJS)
	$(AR) rcs $@ $(OBJS)

clean:
	rm -rf $(BUILD_DIR)

.PHONY: all clean
