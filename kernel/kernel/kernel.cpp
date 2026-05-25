#include <kernel/Framebuffer.hpp>
#include <limine.h>

// Link external symbol initialized inside limine.cpp
struct limine_framebuffer *get_framebuffer();

extern "C" void kernel_main() {
  struct limine_framebuffer *raw_fb = get_framebuffer();

  if (raw_fb != nullptr) {
    Kernel::Graphics::BootloaderInfo info;
    info.baseAddress = reinterpret_cast<uint32_t *>(raw_fb->address);
    info.width = raw_fb->width;
    info.height = raw_fb->height;
    info.pitch = raw_fb->pitch;
    info.bpp = raw_fb->bpp;

    // Initialize and execute the Singleton driver instance
    Kernel::Graphics::Framebuffer &fb = Kernel::Graphics::Framebuffer::Get();
    fb.Initialize(info);

    // Render Background and UI Test Bounds Box
    fb.Clear(Kernel::Graphics::Color::DeepBlue);

    uint32_t boxW = 400;
    uint32_t boxH = 200;
    fb.DrawRect((fb.GetWidth() - boxW) / 2, (fb.GetHeight() - boxH) / 2, boxW,
                boxH, Kernel::Graphics::Color::Cyan);
  }

  // Halt the CPU core safely
  while (true) {
    asm volatile("hlt");
  }
}
