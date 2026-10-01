/* SPDX-License-Identifier: MPL-2.0
 *
 * Program-image test (tests/image.rs): a hand-written "program" in C that
 * implements the runtime contract (ABI 1) the way plcc output does, compiled
 * by GCC instead of LLVM. plcc never emits initialized .data or COMMON
 * symbols, so this is what checks that the loader copies .data, zeroes .bss
 * and COMMON, and that the linker relocates pointers inside .data.
 */
#include <stdint.h>

typedef struct {
  uint8_t *input;
  uint32_t input_size;
  uint8_t *output;
  uint32_t output_size;
  uint8_t *memory;
  uint32_t memory_size;
} image_t;

typedef struct {
  const char *name;
  int64_t interval_ns;
  uint32_t priority;
  uint32_t program_count;
  uint8_t (*single)(void);
  const void *programs;
} task_t;

typedef struct {
  uint32_t abi_version;
  uint32_t task_count;
  const task_t *tasks;
  const image_t *image;
  void (*init)(void);
  void (*run_task)(uint32_t task);
  const void *retain;
  uint32_t retain_count;
  uint32_t retain_signature;
} app_t;

extern void plcc_print(const char *msg);
extern void *memcpy(void *dst, const void *src, unsigned n);

uint8_t img_i[18], img_q[1], img_m[64]; /* .bss */
int counter = 7;                         /* .data */
const char *greeting = "hello from .data"; /* .data, a pointer into .rodata */
int shared;                              /* COMMON (-fcommon) */

static const image_t image = {img_i, 18, img_q, 1, img_m, 64};

static void init(void) { plcc_print(greeting); }

static void run(uint32_t task) {
  (void)task;
  counter++;
  shared += 2;
  memcpy(img_m, &counter, 4);
  memcpy(img_m + 4, &shared, 4);
  img_q[0] = (uint8_t)(counter & 1);
}

static const task_t tasks[1] = {{"Main", 10000000, 1, 0, 0, 0}};
static const app_t app = {1, 1, tasks, &image, init, run, 0, 0, 0};

const app_t *plcc_get_app(void) { return &app; }
