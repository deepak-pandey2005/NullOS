#!/usr/bin/env bash
#
# NullOS — Real Hardware Bootable USB Creation Tool
#
# This script prepares and flashes a bootable USB drive for testing NullOS
# on bare-metal x86_64 hardware via standard UEFI.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_DIR="${ROOT_DIR}/build"
DISK_IMG="${BUILD_DIR}/disk.img"
ESP_DIR="${BUILD_DIR}/esp"

# Terminal Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

print_banner() {
    echo -e "${CYAN}${BOLD}"
    echo "=========================================================="
    echo "         NullOS Bare-Metal USB Flashing Tool              "
    echo "=========================================================="
    echo -e "${NC}"
}

print_help() {
    echo "Usage: $0 [OPTIONS]"
    echo
    echo "Options:"
    echo "  -d, --device <dev>   Target device (e.g., /dev/sdb or sdb) [requires sudo]"
    echo "  -i, --image <path>   Custom GPT disk image path (default: build/disk.img)"
    echo "  -r, --release        Use release distribution image (build/release/nullos-0.1.0-x86_64.img)"
    echo "  -v, --verify         Verify GPT partition table and FAT32 ESP files (non-destructive)"
    echo "  -b, --build          Rebuild bootloader and kernel before flashing"
    echo "  -y, --yes            Skip confirmation prompt (CAUTION)"
    echo "  -h, --help           Display this help guide"
    echo
    echo "Examples:"
    echo "  $0 --verify                          # Verify GPT + FAT32 ESP contents"
    echo "  $0 --verify --release                # Verify release distribution image"
    echo "  sudo $0                              # Interactive drive selection"
    echo "  sudo $0 --release --device /dev/sdc  # Flash release image to /dev/sdc"
    echo "  sudo $0 -b -d sdc                    # Rebuild and flash to /dev/sdc"
}

TARGET_DEV=""
CUSTOM_IMG=""
USE_RELEASE=0
VERIFY_ONLY=0
REBUILD=0
AUTO_CONFIRM=0
ORIG_ARGS=("$@")

while [[ $# -gt 0 ]]; do
    case "$1" in
        -d|--device)
            TARGET_DEV="$2"
            shift 2
            ;;
        -i|--image)
            CUSTOM_IMG="$2"
            shift 2
            ;;
        -r|--release)
            USE_RELEASE=1
            shift
            ;;
        -v|--verify)
            VERIFY_ONLY=1
            shift
            ;;
        -b|--build)
            REBUILD=1
            shift
            ;;
        -y|--yes)
            AUTO_CONFIRM=1
            shift
            ;;
        -h|--help)
            print_banner
            print_help
            exit 0
            ;;
        *)
            echo -e "${RED}[ERROR] Unknown option: $1${NC}"
            print_help
            exit 1
            ;;
    esac
done

if [[ -n "${CUSTOM_IMG}" ]]; then
    DISK_IMG="${CUSTOM_IMG}"
elif [[ "${USE_RELEASE}" -eq 1 ]]; then
    DISK_IMG="${BUILD_DIR}/release/nullos-0.1.0-x86_64.img"
fi

verify_image_layout() {
    local img="$1"
    if [[ ! -f "${img}" ]]; then
        echo -e "${RED}[ERROR] Image not found: ${img}${NC}"
        exit 1
    fi
    echo -e "${BLUE}[*] Verifying GPT partition table in ${img}...${NC}"
    parted -s "${img}" unit MiB print

    echo -e "${BLUE}[*] Verifying FAT32 ESP contents (partition @ 1MiB)...${NC}"
    mdir -i "${img}@@1M" ::
    mdir -i "${img}@@1M" ::/EFI/BOOT

    local missing=0
    for req in "::/EFI/BOOT/BOOTX64.EFI" "::/EFI/BOOT/BOOTAA64.EFI" "::/kernel.elf" "::/initramfs.tar" "::/nullos.cfg" "::/startup.nsh"; do
        if ! mdir -i "${img}@@1M" "${req}" >/dev/null 2>&1; then
            echo -e "${RED}[ERROR] Missing required ESP file: ${req}${NC}"
            missing=1
        fi
    done
    if [[ "${missing}" -ne 0 ]]; then
        exit 1
    fi
    echo -e "${GREEN}[OK] Hardware USB boot image verified: GPT + FAT32 ESP + dual-arch UEFI + kernel + initramfs + nullos.cfg.${NC}"
}

print_banner

if [[ "${VERIFY_ONLY}" -eq 1 ]]; then
    if [[ "${REBUILD}" -eq 1 ]]; then
        if [[ "${USE_RELEASE}" -eq 1 ]]; then
            "${ROOT_DIR}/scripts/release.sh"
        else
            "${ROOT_DIR}/scripts/build.sh"
        fi
    fi
    verify_image_layout "${DISK_IMG}"
    exit 0
fi

# 1. Require root privileges for raw device operations
if [[ $EUID -ne 0 ]]; then
    echo -e "${YELLOW}[!] This script requires root permissions to write to block devices.${NC}"
    echo -e "${YELLOW}[!] Re-running with sudo...${NC}"
    exec sudo "$0" "${ORIG_ARGS[@]}"
fi

# 2. Rebuild binaries and disk image if requested or missing
if [[ $REBUILD -eq 1 || ! -f "${DISK_IMG}" ]]; then
    echo -e "${BLUE}[*] Building NullOS UEFI Bootloader, Kernel, and GPT Disk Image...${NC}"
    if [[ "${USE_RELEASE}" -eq 1 ]]; then
        "${ROOT_DIR}/scripts/release.sh"
    else
        "${ROOT_DIR}/scripts/build.sh"
    fi
    echo
fi

verify_image_layout "${DISK_IMG}"

# 3. Detect and list storage devices
echo -e "${CYAN}${BOLD}Detected Storage Devices:${NC}"
echo "--------------------------------------------------------------------------------"
lsblk -d -o NAME,SIZE,TYPE,TRAN,MODEL,VENDOR,HOTPLUG,RM,ROTA
echo "--------------------------------------------------------------------------------"

# Highlight likely USB drives
USB_CANDIDATES=$(lsblk -d -n -o NAME,TRAN,HOTPLUG,RM | awk '$2=="usb" || $3=="1" || $4=="1" {print $1}')

if [[ -n "${USB_CANDIDATES}" ]]; then
    echo -e "${GREEN}[+] Detected potential USB drive(s): ${BOLD}${USB_CANDIDATES}${NC}"
else
    echo -e "${YELLOW}[!] No obvious removable USB drives auto-detected via TRAN=usb/HOTPLUG=1.${NC}"
fi
echo

# 4. Device Selection
if [[ -z "${TARGET_DEV}" ]]; then
    echo -e "${YELLOW}${BOLD}Please enter the target device node (e.g., 'sdc' or '/dev/sdc'):${NC}"
    read -r -p "Target drive: " TARGET_DEV
fi

# Normalize device path
if [[ ! "${TARGET_DEV}" =~ ^/dev/ ]]; then
    TARGET_DEV="/dev/${TARGET_DEV}"
fi

# 5. Device Validation & Strict Safety Checks
if [[ ! -b "${TARGET_DEV}" ]]; then
    echo -e "${RED}[ERROR] '${TARGET_DEV}' is not a valid block device!${NC}"
    exit 1
fi

# Ensure user didn't specify a partition (e.g., sdc1 instead of sdc)
DEV_TYPE=$(lsblk -d -n -o TYPE "${TARGET_DEV}" 2>/dev/null || true)
if [[ "${DEV_TYPE}" != "disk" ]]; then
    echo -e "${RED}[ERROR] '${TARGET_DEV}' is of type '${DEV_TYPE}', not a whole 'disk'.${NC}"
    echo -e "${YELLOW}Please specify the entire drive (e.g. /dev/sdc), not a single partition.${NC}"
    exit 1
fi

# Safety check: Verify the drive does NOT contain active root or system mount points
ACTIVE_MOUNTS=$(lsblk -n -o MOUNTPOINT "${TARGET_DEV}" 2>/dev/null | grep -E '^/(|boot|home|var|usr|etc)($|/)' || true)
if [[ -n "${ACTIVE_MOUNTS}" ]]; then
    echo -e "${RED}${BOLD}[FATAL SAFETY ERROR] Device ${TARGET_DEV} contains critical system mount points:${NC}"
    echo "${ACTIVE_MOUNTS}"
    echo -e "${RED}Aborting to prevent destruction of your host operating system!${NC}"
    exit 1
fi

# Display device information
DEV_MODEL=$(lsblk -d -n -o MODEL "${TARGET_DEV}" | xargs)
DEV_SIZE=$(lsblk -d -n -o SIZE "${TARGET_DEV}" | xargs)
DEV_TRAN=$(lsblk -d -n -o TRAN "${TARGET_DEV}" | xargs)

echo
echo -e "${YELLOW}=========================== TARGET DEVICE WARNING ===========================${NC}"
echo -e " Target Device : ${RED}${BOLD}${TARGET_DEV}${NC}"
echo -e " Model         : ${DEV_MODEL:-Generic Storage Device}"
echo -e " Size          : ${DEV_SIZE}"
echo -e " Transport     : ${DEV_TRAN:-Unknown}"
echo -e " Source Image  : ${DISK_IMG} (64 MB GPT with UEFI ESP)"
echo -e "${RED}${BOLD} ALL DATA CURRENTLY ON ${TARGET_DEV} WILL BE PERMANENTLY ERASED!${NC}"
echo -e "${YELLOW}=============================================================================${NC}"
echo

# 6. Explicit Confirmation
if [[ $AUTO_CONFIRM -ne 1 ]]; then
    read -r -p "Are you ABSOLUTELY sure you want to write NullOS to ${TARGET_DEV}? Type 'YES' to proceed: " CONFIRM
    if [[ "${CONFIRM}" != "YES" ]]; then
        echo -e "${YELLOW}[*] Operation cancelled by user. No data was written.${NC}"
        exit 0
    fi
fi

# 7. Unmount any active partitions on the target drive
echo
echo -e "${BLUE}[*] Unmounting all partitions on ${TARGET_DEV}...${NC}"
for part in $(lsblk -n -o NAME "${TARGET_DEV}" | tail -n +2); do
    if mountpoint -q "/dev/${part}" 2>/dev/null || grep -qs "/dev/${part} " /proc/mounts; then
        echo "  -> Unmounting /dev/${part}..."
        umount "/dev/${part}" || true
    fi
done

# 8. Flash disk image to USB drive
echo -e "${BLUE}[*] Writing NullOS GPT image to ${TARGET_DEV}...${NC}"
dd if="${DISK_IMG}" of="${TARGET_DEV}" bs=4M status=progress conv=fsync oflag=direct

# 9. Sync filesystem buffers and inform the kernel
echo -e "${BLUE}[*] Synchronizing hardware write caches...${NC}"
sync
udevadm settle 2>/dev/null || sleep 1
partprobe "${TARGET_DEV}" 2>/dev/null || true

echo
echo -e "${GREEN}${BOLD}==========================================================${NC}"
echo -e "${GREEN}${BOLD}       NullOS Bootable USB Successfully Created!          ${NC}"
echo -e "${GREEN}${BOLD}==========================================================${NC}"
echo
echo -e "${CYAN}${BOLD}Hardware Boot Instructions for Testing on Real PCs:${NC}"
echo -e "  1. ${BOLD}Disable Secure Boot${NC} in your PC's UEFI/BIOS settings:"
echo "     (NullOS is custom operating system code and is not signed by Microsoft's CA)"
echo -e "  2. ${BOLD}Ensure UEFI Mode${NC} is selected (do not use CSM / Legacy BIOS mode)."
echo -e "  3. Plug the USB drive into a ${BOLD}USB 2.0 or USB 3.0${NC} port on your test PC."
echo -e "  4. Power on the PC and press the ${BOLD}UEFI Boot Menu key${NC}:"
echo "       - Dell:           F12"
echo "       - Lenovo:         F12 or Fn+F12"
echo "       - HP:             F9 or Esc"
echo "       - ASUS:           F8 or F12"
echo "       - Acer:           F12"
echo "       - MSI:            F11"
echo "       - Gigabyte:       F12"
echo "  5. Select the USB Drive (UEFI: <drive name>) from the boot menu."
echo -e "  6. NullOS will boot via ${BOLD}BOOTX64.EFI${NC}, set up GOP graphics,"
echo "     and display kernel diagnostics & user mode demo directly on your screen!"
echo
