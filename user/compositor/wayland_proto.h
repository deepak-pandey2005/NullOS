/*
 * Minimal Wayland Protocol Types and Constants for NullOS Wayland Compositor.
 *
 * Implements the core wire representation of wl_display, wl_registry,
 * wl_compositor, wl_surface, wl_shm, wl_shm_pool, and wl_shell.
 */

#ifndef _WAYLAND_PROTO_H
#define _WAYLAND_PROTO_H

#include <stdint.h>
#include <stddef.h>

#define WAYLAND_SOCKET_PATH "/tmp/wayland-0"

/* Standard Wayland wire message header (8 bytes) */
struct wl_msg_hdr {
    uint32_t sender_id;
    uint16_t opcode;
    uint16_t size;
};

/* Core well-known Wayland Object IDs */
#define WL_DISPLAY_ID       1

/* Core Interfaces and Global IDs */
#define WL_REGISTRY_ID      2
#define WL_COMPOSITOR_ID    3
#define WL_SHM_ID           4
#define WL_SHELL_ID         5

/* Core wl_display Opcodes */
#define WL_DISPLAY_SYNC          0
#define WL_DISPLAY_GET_REGISTRY  1
#define WL_DISPLAY_ERROR         0
#define WL_DISPLAY_DELETE_ID     1

/* wl_registry Opcodes */
#define WL_REGISTRY_BIND         0
#define WL_REGISTRY_GLOBAL       0
#define WL_REGISTRY_GLOBAL_REMOVE 1

/* wl_compositor Opcodes */
#define WL_COMPOSITOR_CREATE_SURFACE 0
#define WL_COMPOSITOR_CREATE_REGION  1

/* wl_shm Formats */
#define WL_SHM_FORMAT_ARGB8888 0
#define WL_SHM_FORMAT_XRGB8888 1

/* wl_shm Opcodes */
#define WL_SHM_CREATE_POOL 0
#define WL_SHM_FORMAT      0

/* wl_surface Opcodes */
#define WL_SURFACE_DESTROY       0
#define WL_SURFACE_ATTACH        1
#define WL_SURFACE_DAMAGE        2
#define WL_SURFACE_FRAME         3
#define WL_SURFACE_SET_OPAQUE_REGION 4
#define WL_SURFACE_SET_INPUT_REGION  5
#define WL_SURFACE_COMMIT        6

/* wl_shell Opcodes */
#define WL_SHELL_GET_SHELL_SURFACE 0

/* wl_shell_surface Opcodes */
#define WL_SHELL_SURFACE_PONG         0
#define WL_SHELL_SURFACE_MOVE         1
#define WL_SHELL_SURFACE_RESIZE       2
#define WL_SHELL_SURFACE_SET_TOPLEVEL 3
#define WL_SHELL_SURFACE_SET_TRANSIENT 4
#define WL_SHELL_SURFACE_SET_FULLSCREEN 5
#define WL_SHELL_SURFACE_SET_POPUP    6
#define WL_SHELL_SURFACE_SET_MAXIMIZED 7
#define WL_SHELL_SURFACE_SET_TITLE    8
#define WL_SHELL_SURFACE_SET_CLASS    9

#endif /* _WAYLAND_PROTO_H */
