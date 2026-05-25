#include <kernel/Framebuffer.hpp>

namespace Kernel::Graphics {

void Framebuffer::Initialize(const BootloaderInfo &info) {
  m_BaseAddress = info.baseAddress;
  m_Width = info.width;
  m_Height = info.height;
  m_Pitch = info.pitch;
  m_Bpp = info.bpp;
}

void Framebuffer::PutPixel(uint32_t x, uint32_t y, Color color) {
  // Safe Bounds Checking: Prevents memory corruption/Kernel Panic
  if (x >= m_Width || y >= m_Height || m_BaseAddress == nullptr) {
    return;
  }

  // m_Pitch / 4 converts the byte-stride down into a 32-bit index stride
  uint64_t pixelOffset = y * (m_Pitch / 4) + x;

  m_BaseAddress[pixelOffset] = static_cast<uint32_t>(color);
}

void Framebuffer::Clear(Color color) {
  if (m_BaseAddress == nullptr)
    return;

  uint32_t rawColor = static_cast<uint32_t>(color);

  for (uint32_t y = 0; y < m_Height; ++y) {
    // Find the exact address starting this specific horizontal row
    uint32_t *row = reinterpret_cast<uint32_t *>(
        reinterpret_cast<uintptr_t>(m_BaseAddress) + (y * m_Pitch));

    for (uint32_t x = 0; x < m_Width; ++x) {
      row[x] = rawColor;
    }
  }
}

void Framebuffer::DrawRect(uint32_t x, uint32_t y, uint32_t width,
                           uint32_t height, Color color) {
  for (uint32_t i = 0; i < height; ++i) {
    for (uint32_t j = 0; j < width; ++j) {
      PutPixel(x + j, y + i, color);
    }
  }
}

} // namespace Kernel::Graphics
