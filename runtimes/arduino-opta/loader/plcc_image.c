/* SPDX-License-Identifier: MPL-2.0
 *
 * Program image checks (plcc_image.h, docs/program-image.md). Nothing here
 * executes program code: plcc_image_check reads the header and the body from
 * flash, plcc_image_prepare writes only the RAM window, and
 * plcc_image_check_app reads the descriptor the program returned. All
 * address arithmetic is done in 64 bits so a hostile header cannot wrap it.
 */
#include "plcc_image.h"

#include <string.h>

uint32_t plcc_crc32(uint32_t crc, const void *data, size_t len) {
  /* CRC-32/ISO-HDLC (zlib's crc32): reflected 0xEDB88320, init and xorout
   * 0xFFFFFFFF. Bitwise: no table in RAM, ~10 cycles a bit is fine for a
   * check at boot. Pass crc = 0 to start. */
  const uint8_t *p = (const uint8_t *)data;
  crc = ~crc;
  while (len--) {
    crc ^= *p++;
    for (int k = 0; k < 8; k++) crc = (crc >> 1) ^ (0xEDB88320u & (0u - (crc & 1u)));
  }
  return ~crc;
}

static const plcc_image_header_t *header_of(const plcc_image_expect_t *x) {
  return (const plcc_image_header_t *)(uintptr_t)x->slot_addr;
}

static int within(uint64_t start, uint64_t len, uint64_t lo, uint64_t hi) {
  return start >= lo && start + len <= hi && start + len >= start;
}

#define FAIL(msg)  \
  do {             \
    *why = (msg);  \
    return 0;      \
  } while (0)

int plcc_image_check(const plcc_image_expect_t *x, const char **why) {
  static const char *dummy;
  if (!why) why = &dummy;
  if (x->slot_size < PLCC_IMAGE_HEADER_SIZE) FAIL("the runtime's slot is too small");
  plcc_image_header_t h;
  memcpy(&h, header_of(x), sizeof h);
  if (h.magic == 0xFFFFFFFFu) FAIL("empty slot");
  if (h.magic != PLCC_IMAGE_MAGIC) FAIL("no program image (bad magic)");
  if (h.format != PLCC_IMAGE_FORMAT) FAIL("unknown image format");
  if (h.header_size != PLCC_IMAGE_HEADER_SIZE) FAIL("unknown header size");
  if (plcc_crc32(0, &h, offsetof(plcc_image_header_t, header_crc32)) != h.header_crc32) FAIL("header CRC mismatch");
  if (memchr(h.target_id, 0, sizeof h.target_id) == NULL) FAIL("target id not terminated");
  if (strcmp(h.target_id, x->target_id) != 0) FAIL("linked for another device");
  if (h.target_version < x->target_version_min || h.target_version > x->target_version_max)
    FAIL("linked for another version of this device's manifest");
  if (h.abi != x->abi) FAIL("runtime ABI mismatch");
  if (h.services > x->services) FAIL("needs services this runtime does not have");
  if (h.slot_addr != x->slot_addr || h.slot_size != x->slot_size) FAIL("linked for another program slot");
  if (h.ram_addr != x->ram_addr || h.ram_size != x->ram_size) FAIL("linked for another RAM window");
  if (h.flags != 0) FAIL("unknown flags");

  const uint64_t slot = x->slot_addr, slot_end = slot + h.image_size;
  if (h.image_size < PLCC_IMAGE_HEADER_SIZE || h.image_size > x->slot_size) FAIL("bad image size");
  if (h.text_addr != slot + PLCC_IMAGE_HEADER_SIZE) FAIL("text does not follow the header");
  if (!within(h.text_addr, h.text_size, slot + PLCC_IMAGE_HEADER_SIZE, slot_end)) FAIL("text outside the image");
  if (!within(h.data_load, h.data_size, (uint64_t)h.text_addr + h.text_size, slot_end)) FAIL(".data image outside the image");
  const uint64_t ram = x->ram_addr, ram_end = ram + x->ram_size;
  if (h.services_slot != ram) FAIL("services slot is not the window's first word");
  if (!within(h.data_addr, h.data_size, ram + PLCC_IMAGE_RAM_RESERVED, ram_end)) FAIL(".data outside the RAM window");
  if (!within(h.bss_addr, h.bss_size, (uint64_t)h.data_addr + h.data_size, ram_end)) FAIL(".bss outside the RAM window");
  if ((h.get_app & 1u) == 0 || !within(h.get_app & ~1u, 2, h.text_addr, (uint64_t)h.text_addr + h.text_size))
    FAIL("plcc_get_app is not Thumb code inside the image");

  const uint8_t *body = (const uint8_t *)(uintptr_t)(slot + PLCC_IMAGE_HEADER_SIZE);
  if (plcc_crc32(0, body, h.image_size - PLCC_IMAGE_HEADER_SIZE) != h.body_crc32) FAIL("body CRC mismatch");
  *why = "ok";
  return 1;
}

plcc_get_app_fn plcc_image_prepare(const plcc_image_expect_t *x, const plcc_service_table_t *table) {
  const plcc_image_header_t *h = header_of(x);
  uint8_t *ram = (uint8_t *)(uintptr_t)x->ram_addr;
  memset(ram, 0, x->ram_size);
  if (h->data_size) memcpy((void *)(uintptr_t)h->data_addr, (const void *)(uintptr_t)h->data_load, h->data_size);
  *(const plcc_service_table_t *volatile *)(uintptr_t)h->services_slot = table;
  return (plcc_get_app_fn)(uintptr_t)h->get_app;
}

int plcc_image_in_text(const plcc_image_expect_t *x, uintptr_t p, size_t len) {
  const plcc_image_header_t *h = header_of(x);
  return within(p, len, h->text_addr, (uint64_t)h->text_addr + h->text_size);
}

int plcc_image_in_ram(const plcc_image_expect_t *x, uintptr_t p, size_t len) {
  return within(p, len, (uint64_t)x->ram_addr + PLCC_IMAGE_RAM_RESERVED, (uint64_t)x->ram_addr + x->ram_size);
}

static int is_code(const plcc_image_expect_t *x, const void *fn) {
  uintptr_t p = (uintptr_t)fn;
  return (p & 1u) && plcc_image_in_text(x, p & ~(uintptr_t)1, 2);
}

/* Read-only data the descriptor points at lives in text (.rodata); program
 * state and the process image live in the RAM window. */
static int is_const(const plcc_image_expect_t *x, const void *p, size_t len) {
  return plcc_image_in_text(x, (uintptr_t)p, len) || plcc_image_in_ram(x, (uintptr_t)p, len);
}

int plcc_image_check_app(const plcc_image_expect_t *x, const plcc_app_t *app, const char **why) {
  static const char *dummy;
  if (!why) why = &dummy;
  if (((uintptr_t)app & 3u) || !is_const(x, app, sizeof *app)) FAIL("plcc_get_app returned a pointer outside the image");
  if (app->abi_version != x->abi) FAIL("plcc_app.abi_version mismatch");
  if (!is_code(x, (const void *)app->init) || !is_code(x, (const void *)app->run_task)) FAIL("init/run_task outside the image");
  if (app->task_count > PLCC_MAX_TASKS) FAIL("too many tasks");
  if (app->task_count && (((uintptr_t)app->tasks & 3u) || !is_const(x, app->tasks, app->task_count * sizeof(plcc_task_t))))
    FAIL("task table outside the image");
  for (uint32_t t = 0; t < app->task_count; t++) {
    const plcc_task_t *task = &app->tasks[t];
    if (task->single && !is_code(x, (const void *)task->single)) FAIL("a SINGLE function outside the image");
    if (task->interval_ns < 0) FAIL("a negative task interval");
  }
  const plcc_process_image_t *img = app->image;
  if (((uintptr_t)img & 3u) || !is_const(x, img, sizeof *img)) FAIL("process image descriptor outside the image");
  if (img->input_size != x->image_i || img->output_size != x->image_q || img->memory_size != x->image_m)
    FAIL("process image sizes differ from the device's (compile with --device)");
  if ((img->input_size && !plcc_image_in_ram(x, (uintptr_t)img->input, img->input_size)) ||
      (img->output_size && !plcc_image_in_ram(x, (uintptr_t)img->output, img->output_size)) ||
      (img->memory_size && !plcc_image_in_ram(x, (uintptr_t)img->memory, img->memory_size)))
    FAIL("process image outside the RAM window");
  *why = "ok";
  return 1;
}
