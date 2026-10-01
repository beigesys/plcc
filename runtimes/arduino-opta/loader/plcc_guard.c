/* SPDX-License-Identifier: MPL-2.0
 *
 * Fault containment for program-image code: see plcc_guard.h.
 * Cortex-M7 (ARMv7-M) specific; written from the ARMv7-M Architecture
 * Reference Manual (exception entry/return, B1.5.6-B1.5.8; SCB fault status
 * registers, B3.2). Uses no mbed or Arduino code.
 */
#include "plcc_guard.h"

#include <setjmp.h>
#include <string.h>

/* System control block registers (ARMv7-M B3.2.2). */
#define SCB_VTOR (*(volatile uint32_t *)0xE000ED08u)
#define SCB_CFSR (*(volatile uint32_t *)0xE000ED28u)
#define SCB_HFSR (*(volatile uint32_t *)0xE000ED2Cu)
#define SCB_MMFAR (*(volatile uint32_t *)0xE000ED34u)
#define SCB_BFAR (*(volatile uint32_t *)0xE000ED38u)
#define CFSR_MMARVALID (1u << 7)
#define CFSR_BFARVALID (1u << 15)
/* Faults during exception entry or lazy FP stacking: the stacked frame is not usable. */
#define CFSR_STACKING_ERRORS ((1u << 4) | (1u << 5) | (1u << 12) | (1u << 13))

/* The vector table mbed relocates to the start of DTCM at boot (mbed_init,
 * NVIC_RAM_VECTOR_ADDRESS); the variant's linker script reserves it. */
#define RAM_VECTORS 0x20000000u
#define FIRST_FAULT 3u /* HardFault */
#define LAST_FAULT 6u  /* UsageFault */

static jmp_buf guard_jmp;
static volatile uint32_t guard_active;
static volatile uint32_t guard_ticks;
static volatile uint32_t guard_entered;
static uint32_t original_handler[LAST_FAULT + 1];
static const plcc_image_expect_t *expect;
static plcc_guard_fault_t last;

const plcc_guard_fault_t *plcc_guard_last(void) { return &last; }

__attribute__((weak)) void plcc_guard_outputs_off(void) {}

static void set_where(const char *s) {
  size_t n = 0;
  while (n + 1 < sizeof last.where && s[n]) {
    last.where[n] = s[n];
    n++;
  }
  last.where[n] = 0;
}

static __attribute__((noreturn)) void leave(void) {
  guard_active = 0;
  longjmp(guard_jmp, 1);
}

/* Where a rewritten exception frame resumes: thread mode, the faulting
 * thread's stack, back to plcc_guard_call. */
static __attribute__((noreturn, used)) void plcc_guard_recover(void) { leave(); }

static void stop_frame(uint32_t *frame, uint32_t code, uint32_t exception) {
  last.code = code;
  last.pc = frame[6];
  last.lr = frame[5];
  last.exception = exception;
  frame[6] = (uint32_t)(uintptr_t)&plcc_guard_recover & ~1u;
  /* xPSR: Thumb state, no IT block, keep bit 9 (stack realignment on entry). */
  frame[7] = (frame[7] & (1u << 9)) | (1u << 24);
}

/* Called by the fault vector stub with the exception frame. Returns 0 when
 * the fault was contained (the frame now resumes in plcc_guard_recover),
 * otherwise the handler to chain to. */
uint32_t plcc_guard_dispatch(uint32_t *frame, uint32_t exc_return, uint32_t ipsr) {
  uint32_t exception = ipsr & 0x1ffu;
  uint32_t cfsr = SCB_CFSR, hfsr = SCB_HFSR;
  int thread_mode = (exc_return & 0x8u) != 0;
  if (guard_active && thread_mode && !(cfsr & CFSR_STACKING_ERRORS) && exception >= FIRST_FAULT &&
      exception <= LAST_FAULT) {
    memset(&last, 0, sizeof last);
    stop_frame(frame, PLCC_FAULT_CPU, exception);
    last.cfsr = cfsr;
    last.hfsr = hfsr;
    last.bfar = (cfsr & CFSR_BFARVALID) ? SCB_BFAR : 0;
    last.mmfar = (cfsr & CFSR_MMARVALID) ? SCB_MMFAR : 0;
    set_where(exception == 3 ? "HardFault" : exception == 4 ? "MemManage fault" : exception == 5 ? "BusFault" : "UsageFault");
    SCB_CFSR = cfsr; /* write-one-to-clear */
    SCB_HFSR = hfsr;
    __asm__ volatile("dsb\n isb" ::: "memory");
    return 0;
  }
  /* Not the program's (or the frame is unusable): outputs off, then mbed's handler. */
  plcc_guard_outputs_off();
  return original_handler[exception <= LAST_FAULT ? exception : FIRST_FAULT];
}

/* The fault vector: find the frame (MSP or PSP per EXC_RETURN bit 2), ask
 * plcc_guard_dispatch, then return from the exception or chain. r4 is
 * pushed only to keep the stack 8-byte aligned for the C call. */
__attribute__((naked)) static void plcc_guard_vector(void) {
  __asm__ volatile(
      "tst lr, #4\n"
      "ite eq\n"
      "mrseq r0, msp\n"
      "mrsne r0, psp\n"
      "mov r1, lr\n"
      "mrs r2, ipsr\n"
      "push {r4, lr}\n"
      "bl plcc_guard_dispatch\n"
      "pop {r4, lr}\n"
      "cbz r0, 1f\n"
      "bx r0\n"
      "1:\n"
      "bx lr\n");
}

int plcc_guard_install(const plcc_image_expect_t *x) {
  expect = x;
  if (SCB_VTOR != RAM_VECTORS) return 0;
  volatile uint32_t *vectors = (volatile uint32_t *)(uintptr_t)RAM_VECTORS;
  for (uint32_t e = FIRST_FAULT; e <= LAST_FAULT; e++) {
    original_handler[e] = vectors[e];
    vectors[e] = (uint32_t)(uintptr_t)&plcc_guard_vector;
  }
  __asm__ volatile("dsb\n isb" ::: "memory");
  for (uint32_t e = FIRST_FAULT; e <= LAST_FAULT; e++)
    if (vectors[e] != ((uint32_t)(uintptr_t)&plcc_guard_vector)) return 0;
  return 1;
}

int plcc_guard_call(void (*fn)(uint32_t), uint32_t arg) {
  if (guard_active) return 1; /* no nesting */
  if (setjmp(guard_jmp) != 0) {
    guard_active = 0;
    plcc_guard_outputs_off();
    return 1;
  }
  guard_entered = guard_ticks;
  guard_active = 1;
  __asm__ volatile("" ::: "memory");
  fn(arg);
  __asm__ volatile("" ::: "memory");
  guard_active = 0;
  return 0;
}

void plcc_guard_fault(uint32_t code, const char *where) {
  if (!guard_active) return; /* not from program code: nothing to leave */
  memset(&last, 0, sizeof last);
  last.code = code;
  /* `where` is a string in the program's read-only data; copy it (bounded)
   * only if it really lies inside the image. */
  if (where && expect && plcc_image_in_text(expect, (uintptr_t)where, 1)) {
    size_t n = 0;
    while (n + 1 < sizeof last.where && plcc_image_in_text(expect, (uintptr_t)(where + n), 1) && where[n]) {
      last.where[n] = where[n];
      n++;
    }
    last.where[n] = 0;
  } else {
    set_where("?");
  }
  leave();
}

void plcc_guard_tick(void) {
  guard_ticks++;
  if (!guard_active || (guard_ticks - guard_entered) * PLCC_GUARD_TICK_MS < PLCC_GUARD_WATCHDOG_MS) return;
  /* An interrupt from thread mode stacked the interrupted thread's frame on
   * its PSP. Stop it only if that thread is executing program code: then it
   * is the thread inside plcc_guard_call. Otherwise (another thread, or the
   * program inside a library service) try again on the next tick. */
  uint32_t psp;
  __asm__ volatile("mrs %0, psp" : "=r"(psp));
  /* The frame must lie in RAM the runtime's threads use (AXI SRAM or DTCM). */
  int in_sram = psp >= 0x24000000u && psp + 32u <= 0x24080000u;
  int in_dtcm = psp >= 0x20000000u && psp + 32u <= 0x20020000u;
  if ((psp & 3u) || !(in_sram || in_dtcm) || !expect) return;
  uint32_t *frame = (uint32_t *)(uintptr_t)psp;
  if (!plcc_image_in_text(expect, frame[6] & ~1u, 2)) return;
  memset(&last, 0, sizeof last);
  stop_frame(frame, PLCC_FAULT_WATCHDOG, 0);
  set_where("watchdog: a scan ran longer than 500 ms");
}
