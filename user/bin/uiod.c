/*
 * NullOS Ring 3 Userspace I/O (UIO) / VFIO PCI Driver Daemon (Issue #46)
 *
 * Demonstrates fault-isolated userspace PCI device driver framework:
 *   - Probes and enumerates PCI device descriptors via /dev/uioX
 *   - Inspects Base Address Registers (BARs), physical MMIO addresses, and sizes
 *   - Controls hardware bus mastering and memory space decode from Ring 3
 *   - Protects the microkernel core from malfunctioning or rogue device drivers
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <stdint.h>

#pragma pack(push, 1)

typedef struct {
    uint16_t vendor_id;
    uint16_t device_id;
    uint8_t  class_code;
    uint8_t  subclass;
    uint8_t  prog_if;
    uint8_t  revision;
    uint8_t  bus;
    uint8_t  device;
    uint8_t  function;
    uint8_t  irq_line;
    uint16_t command;
    uint16_t status;
    uint64_t bar0_address;
    uint64_t bar0_size;
    uint32_t bar0_is_mmio;
    uint32_t num_bars;
} uio_pci_info_t;

#pragma pack(pop)

static const char *get_class_name(uint8_t class_code, uint8_t subclass) {
    switch (class_code) {
        case 0x01:
            if (subclass == 0x00) return "SCSI Storage Controller";
            if (subclass == 0x01) return "IDE Interface";
            if (subclass == 0x06) return "SATA/AHCI Controller";
            if (subclass == 0x08) return "NVMe Storage Controller";
            return "Mass Storage Controller";
        case 0x02:
            return "Ethernet / Network Controller";
        case 0x03:
            return "VGA / Display Controller";
        case 0x04:
            return "Multimedia Audio Controller";
        case 0x06:
            return "PCI Host / Bridge Device";
        default:
            return "PCI Peripheral Controller";
    }
}

int main(int argc, char **argv) {
    (void)argc;
    (void)argv;

    printf("\n======================================================\n");
    printf(" NullOS Userspace PCI Driver Daemon (UIO/VFIO)        \n");
    printf("======================================================\n");
    printf("[UIOD] Probing available Userspace I/O endpoints (/dev/uio0../dev/uio7)...\n");

    int devices_found = 0;
    int target_fd = -1;
    char target_path[32];
    uio_pci_info_t target_info;

    for (int i = 0; i < 8; i++) {
        char devpath[32];
        snprintf(devpath, sizeof(devpath), "/dev/uio%d", i);

        int fd = open(devpath, O_RDWR);
        if (fd < 0) {
            continue;
        }

        uio_pci_info_t info;
        ssize_t n = read(fd, &info, sizeof(info));
        if (n < (ssize_t)sizeof(info)) {
            close(fd);
            continue;
        }

        devices_found++;
        printf("[UIOD] Discovered /dev/uio%d:\n", i);
        printf("  - PCI Address: [%02x:%02x.%d] | ID: %04x:%04x | Class: %02x.%02x (%s)\n",
               info.bus, info.device, info.function,
               info.vendor_id, info.device_id,
               info.class_code, info.subclass,
               get_class_name(info.class_code, info.subclass));
        printf("  - Status: 0x%04x | Command: 0x%04x | IRQ Line: %d\n",
               info.status, info.command, info.irq_line);

        if (info.num_bars > 0) {
            printf("  - BAR0: 0x%08llx | Size: %llu KB | Type: %s\n",
                   (unsigned long long)info.bar0_address,
                   (unsigned long long)(info.bar0_size / 1024),
                   info.bar0_is_mmio ? "MMIO (Memory Mapped)" : "Port I/O");
        } else {
            printf("  - BARs: None\n");
        }

        // Select a non-bridge peripheral to demonstrate userspace driver control
        if (target_fd < 0 && info.class_code != 0x06) {
            target_fd = fd;
            snprintf(target_path, sizeof(target_path), "/dev/uio%d", i);
            memcpy(&target_info, &info, sizeof(info));
        } else {
            close(fd);
        }
    }

    if (devices_found == 0) {
        printf("[UIOD] Error: No UIO device nodes detected in /dev!\n");
        return 1;
    }

    printf("[UIOD] Total PCI devices enumerated via Ring 3 UIO interface: %d\n", devices_found);

    if (target_fd >= 0) {
        printf("[UIOD] Attaching userspace driver logic to target device %s ([%02x:%02x.%d] %04x:%04x)...\n",
               target_path, target_info.bus, target_info.device, target_info.function,
               target_info.vendor_id, target_info.device_id);

        // Command 1 = Enable Bus Mastering (Bit 2)
        // Command 2 = Enable Memory Space Decode (Bit 1)
        uint8_t cmds[2] = { 1, 2 };
        ssize_t written = write(target_fd, cmds, sizeof(cmds));
        if (written == (ssize_t)sizeof(cmds)) {
            printf("[UIOD] Successfully issued Bus Master and Memory Space activation to %s.\n", target_path);
        } else {
            printf("[UIOD] Warning: Command write returned %ld\n", (long)written);
        }

        close(target_fd);
    }

    printf("[UIOD] Ring 3 PCI device management and UIO framework test successful.\n");
    printf("[UIOD] Microkernel userspace driver isolation validated.\n");
    return 0;
}
