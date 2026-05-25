#include <limine.h>
#include <stddef.h>

extern "C" {
// Submit Framebuffer Request to Limine
__attribute__((used)) static volatile struct limine_framebuffer_request
    framebuffer_request = {
        .id = LIMINE_FRAMEBUFFER_REQUEST, .revision = 0, .response = NULL};

// Assert compatibility with modern Limine baseline specifications
__attribute__((used)) static volatile LIMINE_BASE_REVISION(2);
}

struct limine_framebuffer *get_framebuffer() {
  if (framebuffer_request.response == NULL ||
      framebuffer_request.response->framebuffer_count < 1) {
    return NULL;
  }
  return framebuffer_request.response->framebuffers[0];
}
