#pragma once

#include <stddef.h>
#include <stdint.h>

namespace Kernel::Graphics {

// Type-safe color representations (XRGB Format: 0x00RRGGBB)
enum class Color : uint32_t {
  DeepBlue = 0x0F172A,
  Cyan = 0x38BDF8,
  White = 0xFFFFFF,
  Black = 0x000000
};

// The raw struct passed by the 64-bit bootloader
struct BootloaderInfo {
  uint32_t *baseAddress;
  uint64_t width;
  uint64_t height;
  uint64_t pitch;
  uint16_t bpp;
};

class Framebuffer {
public:
  // Delete copy constructor and assignment operator to enforce a single
  // instance
  Framebuffer(const Framebuffer &) = delete;
  Framebuffer &operator=(const Framebuffer &) = delete;

  // Static access to the Singleton instance
  static Framebuffer &Get() {
    static Framebuffer instance;
    return instance;
  }

  // Core API
  void Initialize(const BootloaderInfo &info);
  inline void PutPixel(uint32_t x, uint32_t y, Color color);
  void Clear(Color color);
  void DrawRect(uint32_t x, uint32_t y, uint32_t width, uint32_t height,
                Color color);

  // Getters for window managers / UI layers
  uint64_t GetWidth() const { return m_Width; }
  uint64_t GetHeight() const { return m_Height; }

private:
  Framebuffer() = default; // Private constructor for Singleton pattern

  uint32_t *m_BaseAddress{nullptr};
  uint64_t m_Width{0};
  uint64_t m_Height{0};
  uint64_t m_Pitch{0};
  uint16_t m_Bpp{0};
};

} // namespace Kernel::Graphics
