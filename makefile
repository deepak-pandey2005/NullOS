# --- Toolchain Configuration ---
CXX      := x86_64-elf-g++
AS       := nasm
LD       := x86_64-elf-ld
XORRISO  := xorriso

# --- Compilation Flags ---
# -MMD -MP automatically generates header dependency rules so changing a .h forces a recompile
CXXFLAGS := -Wall -Wextra -std=c++20 -O2 \
            -ffreestanding -fno-exceptions -fno-rtti \
            -mno-red-zone -mcmodel=kernel \
            -Ikernel/include -MMD -MP \
						-Ilimine

ASFLAGS  := -f elf64
LDFLAGS  := -T kernel/arch/x86_64/linker.ld -nostdlib -Wl,--no-warn-rwx-segments

# --- Directory & File Layout ---
BUILD_DIR := build
ISO_DIR   := iso_root
KERNEL_ELF:= $(ISO_DIR)/boot/kernel.elf
ISO_IMAGE := NullOS.iso

# Automatically locate all C++ and Assembly source files
CXX_SRC   := $(shell find kernel -name '*.cpp')
AS_SRC    := $(shell find kernel -name '*.asm')

# Map source files directly into their respective object paths inside the build/ directory
OBJ       := $(patsubst kernel/%, $(BUILD_DIR)/kernel/%, $(CXX_SRC:.cpp=.o) $(AS_SRC:.asm=.o))
DEP       := $(OBJ:.o=.d)

# --- Build Rules ---
.PHONY: all clean run iso

all: iso

# 1. Compile C++ Source Files
$(BUILD_DIR)/kernel/%.o: kernel/%.cpp
	@mkdir -p $(dir $@)
	@echo "  CXX  $<"
	@$(CXX) $(CXXFLAGS) -c $< -o $@

# 2. Compile Assembly Source Files
$(BUILD_DIR)/kernel/%.o: kernel/%.asm
	@mkdir -p $(dir $@)
	@echo "  AS   $<"
	@$(AS) $(ASFLAGS) $< -o $@

# 3. Link Object Files into the Final Kernel ELF Binary
$(KERNEL_ELF): $(OBJ)
	@mkdir -p $(dir $@)
	@echo "  LD   $@"
	@$(CXX) $(LDFLAGS) $(OBJ) -o $@

# 4. Generate the Bootable ISO Image and Deploy Limine MBR
$(ISO_IMAGE): $(KERNEL_ELF)
	@echo "  ISO  $@"
	@$(XORRISO) -as mkisofs -b boot/limine/limine-bios-cd.bin \
		-no-emul-boot -boot-load-size 4 -boot-info-table \
		--efi-boot boot/limine/limine-uefi-cd.bin \
		-efi-boot-part --efi-boot-image --protective-msdos-label \
		$(ISO_DIR) -o $(ISO_IMAGE) > /dev/null 2>&1
	@echo "  BOOTLOALDER Deploying Limine..."
	@./limine/limine bios-install $(ISO_IMAGE)

iso: $(ISO_IMAGE)

# 5. Emulate the Operating System in QEMU
run: iso
	qemu-system-x86_64 -M q35 -m 2G -cdrom $(ISO_IMAGE) -boot d -serial stdio

# 6. Housekeeping: Wipe Build Artifacts
clean:
	@echo "  CLEAN"
	@rm -rf $(BUILD_DIR) $(ISO_IMAGE) $(KERNEL_ELF)

# Include header dependency tracking tracking files
-include $(DEP)
