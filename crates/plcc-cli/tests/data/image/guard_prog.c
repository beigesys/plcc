/* SPDX-License-Identifier: MPL-2.0
 *
 * Program-image test (tests/image.rs, emulated_guard_contains_faults): a
 * "program" whose tasks misbehave on purpose, for the runtime's fault guard
 * (runtimes/arduino-opta/loader/plcc_guard.c) on an emulated Cortex-M7.
 *   task 0  counts (works)
 *   task 1  executes an undefined instruction (UsageFault -> HardFault)
 *   task 2  loops forever (the watchdog)
 *   task 3  reads unmapped memory (BusFault -> HardFault)
 *   task 4  calls plcc_fault (as plcc code does on a division by zero)
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

extern void plcc_fault(uint32_t code, const char *where);

uint8_t img_i[18], img_q[1], img_m[64];
static uint32_t counter;

static const image_t image = {img_i, 18, img_q, 1, img_m, 64};

static void init(void) { counter = 100; }

static void run(uint32_t task) {
  img_q[0] = 0x0f; /* outputs on while it runs: the guard must turn them off */
  switch (task) {
    case 0:
      counter++;
      img_m[0] = (uint8_t)counter;
      break;
    case 1:
      __asm__ volatile("udf #7");
      break;
    case 2:
      for (;;) __asm__ volatile("" ::: "memory");
    case 3:
      counter += *(volatile uint32_t *)0x90000000u;
      break;
    case 4:
      plcc_fault(1, "guard_prog.c:61: task 4");
      break;
  }
}

static const task_t tasks[5] = {
    {"T0", 0, 1, 0, 0, 0}, {"T1", 0, 1, 0, 0, 0}, {"T2", 0, 1, 0, 0, 0}, {"T3", 0, 1, 0, 0, 0}, {"T4", 0, 1, 0, 0, 0},
};
static const app_t app = {1, 5, tasks, &image, init, run, 0, 0, 0};

const app_t *plcc_get_app(void) { return &app; }
