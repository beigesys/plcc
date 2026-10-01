/* SPDX-License-Identifier: MPL-2.0
 *
 * Bare-metal test of the loader's fault guard (../plcc_guard.c) on QEMU's
 * mps2-an500 board (Cortex-M7), the way the Opta runs it: vector table copied
 * to 0x20000000 (as mbed does), thread mode on the PSP (as RTX threads run),
 * a 10 ms SysTick driving plcc_guard_tick (the Opta uses an mbed Ticker).
 * Output through semihosting. Runs the image loaded at 0x00100000 (the
 * guard-test device, crates/plcc-cli/tests/data/image/guard-test.toml) built
 * from guard_prog.c, and prints one line per step:
 *   run <task> ok m0=<n>
 *   run <task> fault code=<n> exc=<n> pc_in_text=<0|1> off=<calls> where=<text>
 *   chained off=<0|1>            (a fault outside the program reached the
 *                                 original handler; off=1: outputs off first)
 * Built and run by crates/plcc-cli/tests/image.rs; see build-guard.sh.
 */
#include <stdint.h>
#include <string.h>

#include "../plcc_guard.h"
#include "../plcc_image.h"

extern const plcc_service_table_t plcc_service_table;

/* ── Semihosting ────────────────────────────────────────────────────── */
static int semi(int op, const void *arg) {
  register int r0 __asm__("r0") = op;
  register const void *r1 __asm__("r1") = arg;
  __asm__ volatile("bkpt 0xab" : "+r"(r0) : "r"(r1) : "memory");
  return r0;
}
static void out(const char *s) { semi(0x04, s); }
static __attribute__((noreturn)) void finish(int code) {
  const uint32_t block[2] = {0x20026u, (uint32_t)code}; /* ADP_Stopped_ApplicationExit */
  for (;;) semi(0x20, block);
}
static void out_u(uint32_t v) {
  char b[12];
  int n = 11;
  b[n] = 0;
  do b[--n] = (char)('0' + v % 10); while (v /= 10);
  out(b + n);
}

/* ── The device ────────────────────────────────────────────────────── */
static const plcc_image_expect_t expect = {
    "guard-test", 1, 1, 1, 0x00100000u, 0x80000u, 0x20010000u, 0x10000u, 185, 18, 1, 64,
};

static volatile uint32_t outputs_off_calls;
void plcc_guard_outputs_off(void) { outputs_off_calls++; }
int64_t plcc_monotonic_ns(void) { return 0; }
void plcc_print(const char *msg) { (void)msg; }
void plcc_fault(uint32_t code, const char *where) { plcc_guard_fault(code, where); }

/* ── The program ───────────────────────────────────────────────────── */
static plcc_get_app_fn get_app;
static const plcc_app_t *app;
static void call_get_app(uint32_t x) { (void)x; app = get_app(); }
static void call_init(uint32_t x) { (void)x; app->init(); }
static void call_task(uint32_t t) { app->run_task(t); }

static void step(uint32_t task) {
  uint32_t before = outputs_off_calls;
  out("run ");
  out_u(task);
  if (!plcc_guard_call(call_task, task)) {
    out(" ok m0=");
    out_u(app->image->memory[0]);
    out("\n");
    return;
  }
  const plcc_guard_fault_t *f = plcc_guard_last();
  out(" fault code=");
  out_u(f->code);
  out(" exc=");
  out_u(f->exception);
  out(" pc_in_text=");
  out_u((uint32_t)plcc_image_in_text(&expect, f->pc & ~1u, 2));
  out(" off=");
  out_u(outputs_off_calls - before);
  out(" where=");
  out(f->where);
  out("\n");
}

static void test_main(void) {
  const char *why = "";
  if (!plcc_guard_install(&expect)) {
    out("guard not installed\n");
    finish(2);
  }
  if (!plcc_image_check(&expect, &why)) {
    out("reject ");
    out(why);
    out("\n");
    finish(2);
  }
  get_app = plcc_image_prepare(&expect, &plcc_service_table);
  if (plcc_guard_call(call_get_app, 0) || !plcc_image_check_app(&expect, app, &why) || plcc_guard_call(call_init, 0)) {
    out("start failed\n");
    finish(2);
  }
  out("started\n");
  step(0);
  step(1); /* udf */
  step(0);
  step(2); /* loops: the watchdog */
  step(0);
  step(3); /* unmapped read */
  step(0);
  step(4); /* plcc_fault */
  step(0);
  /* A fault outside any guarded call goes to the original handler, after
   * the outputs are put off. */
  outputs_off_calls = 0;
  __asm__ volatile("udf #9");
  out("not reached\n");
  finish(3);
}

/* ── Start-up ──────────────────────────────────────────────────────── */
extern uint32_t _sidata, _sdata, _edata, _sbss, _ebss, __msp_top, __psp_top;
#define SCB_VTOR (*(volatile uint32_t *)0xE000ED08u)
#define SCB_CPACR (*(volatile uint32_t *)0xE000ED88u)
#define SYST_CSR (*(volatile uint32_t *)0xE000E010u)
#define SYST_RVR (*(volatile uint32_t *)0xE000E014u)
#define SYST_CVR (*(volatile uint32_t *)0xE000E018u)

void Reset_Handler(void);
static void original_fault(void) {
  out(outputs_off_calls ? "chained off=1\n" : "chained off=0\n");
  finish(0);
}
static void SysTick_Handler(void) { plcc_guard_tick(); }
static void Unexpected_Handler(void) {
  out("unexpected exception\n");
  finish(4);
}

__attribute__((section(".vectors"), used)) static const void *const vectors[16] = {
    &__msp_top,         Reset_Handler,      Unexpected_Handler, original_fault,     original_fault, original_fault,
    original_fault,     0,                  0,                  0,                  0,              Unexpected_Handler,
    Unexpected_Handler, 0,                  Unexpected_Handler, SysTick_Handler,
};

void Reset_Handler(void) {
  for (uint32_t *s = &_sidata, *d = &_sdata; d < &_edata;) *d++ = *s++;
  for (uint32_t *d = &_sbss; d < &_ebss;) *d++ = 0;
  SCB_CPACR |= 0xFu << 20; /* FPU (program images use it) */
  __asm__ volatile("dsb\n isb");
  /* Vector table to the start of RAM, as mbed_init does on the Opta. */
  memcpy((void *)0x20000000u, vectors, sizeof vectors);
  SCB_VTOR = 0x20000000u;
  __asm__ volatile("dsb\n isb");
  /* Thread mode on the PSP, as an RTX thread. */
  __asm__ volatile(
      "msr psp, %0\n"
      "mov r0, #2\n"
      "msr control, r0\n"
      "isb\n" ::"r"(&__psp_top)
      : "r0", "memory");
  SYST_RVR = 25000u * 10u - 1u; /* 10 ms at the board's 25 MHz */
  SYST_CVR = 0;
  SYST_CSR = 7;
  test_main();
  finish(5);
}
