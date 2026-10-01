/* SPDX-License-Identifier: MPL-2.0
 *
 * Fault containment for program-image code (docs/program-image.md, "Loading"):
 * every call into the program goes through plcc_guard_call, which returns
 * instead of crashing when the program
 *   - calls plcc_fault (division by zero, ...)            -> code from the program
 *   - takes a HardFault/MemManage/BusFault/UsageFault     -> PLCC_FAULT_CPU
 *   - runs longer than the watchdog                        -> PLCC_FAULT_WATCHDOG
 * so the runtime can put the outputs off and keep USB and Modbus running.
 *
 * How: a setjmp around the call; plcc_fault longjmps back. The CPU fault
 * handlers (installed into the RAM vector table) check that the fault came
 * from thread mode while a guarded call is active and the exception frame is
 * intact; then they rewrite the stacked PC to a recovery function and return
 * from the exception, and that function longjmps back. The watchdog tick
 * (an interrupt) does the same to the interrupted thread's frame, but only
 * when the interrupted PC is inside the program's code. Any other fault goes
 * to the original handler (mbed's crash report) after the outputs are put
 * off with plcc_guard_outputs_off.
 */
#ifndef PLCC_GUARD_H
#define PLCC_GUARD_H

#include <stdint.h>

#include "plcc_image.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct plcc_guard_fault {
  uint32_t code;      /* PLCC_FAULT_* */
  char where[96];     /* plcc_fault's site ("file:line:col: POU"), or a description */
  uint32_t pc, lr;    /* stacked PC/LR of a CPU fault or a watchdog stop */
  uint32_t exception; /* 3 HardFault, 4 MemManage, 5 BusFault, 6 UsageFault; 0 otherwise */
  uint32_t cfsr, hfsr, bfar, mmfar;
} plcc_guard_fault_t;

/* Install the fault handlers. Returns 1 on success; 0 if the vector table is
 * not in RAM (then no program may be run: a fault could not be contained). */
int plcc_guard_install(const plcc_image_expect_t *x);

/* Call fn(arg), which is program code or calls into it. Returns 0 if it
 * returned normally, otherwise 1 and the fault is in *plcc_guard_last(). */
int plcc_guard_call(void (*fn)(uint32_t), uint32_t arg);

/* The plcc_fault service: record and leave the guarded call. Never returns
 * when called inside a guarded call. */
void plcc_guard_fault(uint32_t code, const char *where);

/* Called every 10 ms from a timer interrupt: enforces the watchdog. */
void plcc_guard_tick(void);
#define PLCC_GUARD_TICK_MS 10u
#define PLCC_GUARD_WATCHDOG_MS 500u

const plcc_guard_fault_t *plcc_guard_last(void);

/* Board hook: drive every output off, from any context (also a fault handler). */
void plcc_guard_outputs_off(void);

#ifdef __cplusplus
}
#endif
#endif
