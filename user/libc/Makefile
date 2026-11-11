CC ?= gcc
AR ?= ar
AS ?= as
LD ?= ld

CFLAGS = -Wall -Wextra -O2 -march=x86-64 -mtune=generic -nostdlib -fno-builtin -fno-stack-protector -mno-red-zone -Iinclude -mcmodel=small
ASFLAGS = --64

BUILD_DIR = build
SRCS = string.c stdio.c stdlib.c unistd.c syscall.c pthread.c semaphore.c dlfcn.c errno.c
OBJS = $(patsubst %.c,$(BUILD_DIR)/%.o,$(SRCS))
PIC_OBJS = $(patsubst %.c,$(BUILD_DIR)/pic/%.o,$(SRCS))

all: $(BUILD_DIR)/crt0.o $(BUILD_DIR)/libc.a $(BUILD_DIR)/libc.so

$(BUILD_DIR)/crt0.o: src/crt0.s
	@mkdir -p $(BUILD_DIR)
	$(AS) $(ASFLAGS) $< -o $@

$(BUILD_DIR)/%.o: src/%.c
	@mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD_DIR)/pic/%.o: src/%.c
	@mkdir -p $(BUILD_DIR)/pic
	$(CC) $(CFLAGS) -fPIC -c $< -o $@

$(BUILD_DIR)/libc.a: $(OBJS)
	$(AR) rcs $@ $(OBJS)

$(BUILD_DIR)/libc.so: $(PIC_OBJS)
	$(LD) -shared -nostdlib -soname libc.so -o $@ $(PIC_OBJS)

clean:
	rm -rf $(BUILD_DIR)

.PHONY: all clean
