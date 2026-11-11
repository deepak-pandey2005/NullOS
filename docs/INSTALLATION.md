# NullOS Installation & Distribution Guide

This guide covers building bootable **NullOS** UEFI disk images, configuring boot parameters (`nullos.cfg`), verifying artifacts in QEMU (`x86_64` OVMF & `AArch64` AAVMF), and flashing a GPT+FAT32 USB drive for bare-metal x86_64 hardware testing.

---

## 1. Prerequisites

### Host Packages (Linux)
Install the following tools using your distribution's package manager:
- **Build & Image Tooling**: `make`, `gcc`, `binutils` (`nm`, `ld`), `tar`, `python3`, `sha256sum`
- **GPT & FAT32 Formatting**: `parted`, `dosfstools` (`mkfs.fat`), `mtools` (`mcopy`, `mmd`, `mdir`)
- **Emulators & UEFI Firmware**:
  - `qemu-system-x86_64` + OVMF firmware (`/usr/share/edk2/x64/OVMF.4m.fd` or `/usr/share/OVMF/OVMF.fd`)
  - `qemu-system-aarch64` + AAVMF firmware (`/usr/share/AAVMF/AAVMF_CODE.fd`) *(optional, for `BOOTAA64.EFI` UEFI verification)*

### Rust Targets
Ensure the required Rust bare-metal and UEFI targets are installed:

```bash
rustup target add x86_64-unknown-none
rustup target add x86_64-unknown-uefi
rustup target add aarch64-unknown-uefi
```

---

## 2. Building NullOS Images

### Development Build
Builds the `x86_64` and `AArch64` UEFI bootloaders, userspace `libc.so` and binaries, NullOS SDK demo `.npkg`, debug kernel ELF (`kernel.elf`), binary symbol table (`KERNEL.SYM`), USTAR initramfs (`initramfs.tar`), and GPT disk images:

```bash
./scripts/build.sh
```

Outputs in `build/`:
- `build/disk.img` — 64 MiB GPT disk containing the 62 MiB FAT32 EFI System Partition (`NULLOS_ESP`)
- `build/esp.part` — Standalone 62 MiB FAT32 ESP image
- `build/data.img` — 36 MiB FAT32 persistent storage disk containing userspace `.ELF` utilities, `SDKDEMO.NPK`, and `KERNEL.SYM`
- `build/initramfs.tar` — POSIX USTAR initramfs archive loaded into RamFS (`/bin`, `/lib`, `/etc`, `/pkg`)

### Optimized Release Build
Produces optimized release images, cryptographic checksums, and verifies GPT/FAT32 integrity under `build/release/`:

```bash
./scripts/release.sh
```

To build the release distribution and immediately boot-test both the x86_64 OVMF image and AArch64 AAVMF loader in QEMU:

```bash
./scripts/release.sh --test
```

Release artifacts under `build/release/`:
- `nullos-0.1.0-x86_64.img` — Bootable release GPT disk image
- `nullos-0.1.0-esp.fat32.img` — Standalone FAT32 ESP partition image
- `nullos-0.1.0-data.fat32.img` — Userspace & `KERNEL.SYM` persistent FAT32 disk
- `BOOTX64.EFI`, `BOOTAA64.EFI`, `kernel.elf`, `initramfs.tar`, `kernel.ksym`, `nullos.cfg`
- `SHA256SUMS` & `RELEASE_MANIFEST.txt`

Verify checksums and disk layout without modifying any device:

```bash
(cd build/release && sha256sum -c SHA256SUMS)
./scripts/create_bootable_usb.sh --verify --release
```

---

## 3. ESP Partition Layout & `nullos.cfg`

The 62 MiB FAT32 EFI System Partition (`1 MiB .. 63 MiB`, GPT ESP flag enabled) contains:

```text
/
├── EFI/
│   └── BOOT/
│       ├── BOOTX64.EFI     (x86_64 UEFI bootloader)
│       ├── BOOTAA64.EFI    (AArch64 UEFI bootloader)
│       └── nullos.cfg      (Fallback boot config copy)
├── kernel.elf              (64-bit x86_64 NullOS kernel)
├── initramfs.tar           (USTAR userspace root archive)
├── nullos.cfg              (Primary boot configuration)
└── startup.nsh             (UEFI Shell fallback script)
```

### Boot Configuration (`nullos.cfg`)
`BOOTX64.EFI` / `BOOTAA64.EFI` parses `\nullos.cfg` (or `\EFI\BOOT\nullos.cfg`) prior to loading the kernel and initramfs:

```ini
# NullOS UEFI Boot Configuration
version=0.1.0
profile=release
kernel=\kernel.elf
initrd=\initramfs.tar
cmdline=console=com1 loglevel=info root=ramfs
timeout=0
```

| Key | Description | Default |
|---|---|---|
| `version` | Release identifier displayed during boot | `0.1.0` |
| `kernel` | ESP path to the ELF64 kernel executable | `\kernel.elf` |
| `initrd` | ESP path to the USTAR `initramfs.tar` image | `\initramfs.tar` |
| `cmdline` | Kernel command line forwarded via `BootInfo` (include `test` to auto-run tests) | `console=com1 loglevel=info` |
| `timeout` | Pre-boot delay in seconds (`0`..`30`) | `0` |

---

## 4. Testing in QEMU

### x86_64 (OVMF UEFI)
- **Graphical window + serial stdio**: `./scripts/run.sh`
- **Terminal CLI mode**: `./scripts/run.sh --cli`
- **Headless automated test suite** *(debug build)*: `./scripts/run.sh --headless`
- **Headless automated test suite** *(release build)*: `./scripts/run.sh --release --headless`

### AArch64 (`BOOTAA64.EFI` under AAVMF)
`BOOTAA64.EFI` initializes UEFI firmware protocols (ConOut, GOP graphics, ACPI RSDP, `nullos.cfg`, and `initramfs.tar` loading). Note that the NullOS kernel currently implements the `x86_64` HAL only; when `BOOTAA64.EFI` runs on `aarch64` alongside an `x86_64` `kernel.elf`, it reports the UEFI verification status cleanly and returns to firmware rather than executing incompatible machine code.

```bash
./scripts/run.sh --aarch64-uefi
```

---

## 5. Flashing a Bootable USB Drive (Real x86_64 Hardware)

> **WARNING**: Writing a raw GPT disk image erases all existing partitions and data on the target USB drive. Double-check the device node (`/dev/sdX`) before proceeding.

1. **Verify the built image first** (non-destructive):
   ```bash
   ./scripts/create_bootable_usb.sh --verify --release
   ```

2. **Insert your USB flash drive** and flash the release GPT image:
   ```bash
   sudo ./scripts/create_bootable_usb.sh --release --device /dev/sdX
   ```
   *(Or run `sudo ./scripts/create_bootable_usb.sh --release` interactively to pick from detected USB drives. The script checks that the target is not a host root/system mount before writing.)*

3. **Configure BIOS/UEFI on the target x86_64 machine**:
   - **Disable Secure Boot** (NullOS is unsigned custom OS code).
   - **Set Boot Mode to UEFI** (disable Legacy BIOS / CSM fallback if possible so GOP provides a linear 32-bit framebuffer).
   - Open your motherboard UEFI Boot Menu (`F12`, `F11`, `F9`, or `F8` depending on vendor) and select `UEFI: <Your USB Drive>`.

### Real Hardware Notes
- **Display**: NullOS uses the UEFI Graphics Output Protocol (`GOP`) linear framebuffer initialized by `BOOTX64.EFI`, so console text and graphics work without legacy VGA BIOS calls.
- **Storage & Input**: NullOS includes drivers for PCI enumeration, NVMe, USB xHCI HID keyboard/mouse, PS/2 keyboard, and serial COM1 (`0x3F8`). When VirtIO hypervisor devices are absent on bare metal, the kernel cleanly skips VirtIO and runs from the UEFI-loaded `initramfs.tar` in RamFS alongside any detected NVMe/USB hardware.

---

## 6. Troubleshooting

- **`OVMF firmware not found`**: Install `edk2-ovmf` / `ovmf` so `/usr/share/edk2/x64/OVMF.4m.fd` or `/usr/share/OVMF/OVMF.fd` is available.
- **`Could not find kernel.elf`**: Verify ESP contents with `./scripts/create_bootable_usb.sh --verify` and ensure `kernel=\kernel.elf` matches the file on the FAT32 partition.
- **Real hardware stops at firmware menu**: Confirm **Secure Boot is Disabled** and that the USB partition table is GPT with the `esp` flag enabled (`parted /dev/sdX print`).

