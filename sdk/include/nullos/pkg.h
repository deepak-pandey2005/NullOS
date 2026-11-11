/**
 * @file nullos/pkg.h
 * @brief NullOS Binary Package Format (.npkg) Specification (Phase 18 - M39)
 *
 * Binary layout (Little-Endian, fixed 64-byte header + ELF/payload bytes):
 *   Offset  Size  Field
 *   0x00    4     magic          ("NPKG" = 0x474B504E)
 *   0x04    2     format_version (1)
 *   0x06    2     flags          (bit 0: executable ELF, bit 1: dynamic libc.so)
 *   0x08    24    name           (NUL-padded UTF-8 package name)
 *   0x20    12    version        (NUL-padded SemVer string, e.g. "1.0.0")
 *   0x2C    4     header_size    (64)
 *   0x30    8     payload_size   (Size of embedded payload in bytes)
 *   0x38    4     payload_crc32  (IEEE 802.3 CRC-32 over embedded payload bytes)
 *   0x3C    4     reserved       (0)
 */

#ifndef _NULLOS_PKG_H
#define _NULLOS_PKG_H

#include <stdint.h>

#define NPKG_MAGIC          0x474B504Eu /* "NPKG" in little-endian memory order */
#define NPKG_FORMAT_VERSION 1u
#define NPKG_HEADER_SIZE    64u

#define NPKG_FLAG_EXEC_ELF  (1u << 0)
#define NPKG_FLAG_DYN_LIBC  (1u << 1)

typedef struct __attribute__((packed)) {
    uint32_t magic;
    uint16_t format_version;
    uint16_t flags;
    char     name[24];
    char     version[12];
    uint32_t header_size;
    uint64_t payload_size;
    uint32_t payload_crc32;
    uint32_t reserved;
} NpkgHeader;

#endif /* _NULLOS_PKG_H */

