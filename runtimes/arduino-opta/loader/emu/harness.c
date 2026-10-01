/* SPDX-License-Identifier: MPL-2.0
 *
 * Runs a plcc program image under qemu-arm (user mode, -cpu cortex-m7) with
 * the Opta loader's own checks (../plcc_image.c) and service table
 * (../plcc_services.c, resolved against newlib and libgcc as on the board).
 * Freestanding: Linux system calls only, no C runtime start-up.
 *
 *   harness <image> <scans> <step_ms> [I bytes as hex]
 *
 * Maps the program slot and RAM window at the Opta's addresses, checks the
 * image, prepares the window, checks the descriptor, calls init, then runs
 * <scans> passes of the scan loop on a fake clock advancing <step_ms> per
 * pass, printing %Q and %M after each pass that ran a task:
 *   check ok
 *   app tasks=<n>
 *   scan <k> t=<ms> Q=<hex> M=<hex>
 *   print <message>                       (plcc_print)
 *   fault <code> <where>                  (plcc_fault; exit status 3)
 *   reject <why>                          (exit status 2)
 * Built by crates/plcc-cli/tests/image.rs; see build.sh.
 */
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "../plcc_image.h"

extern const plcc_service_table_t plcc_service_table;

/* ── Linux system calls (ARM EABI: number in r7, svc 0) ─────────────── */
static long sys3(long n, long a, long b, long c) {
  register long r0 __asm__("r0") = a;
  register long r1 __asm__("r1") = b;
  register long r2 __asm__("r2") = c;
  register long r7 __asm__("r7") = n;
  __asm__ volatile("svc 0" : "+r"(r0) : "r"(r1), "r"(r2), "r"(r7) : "memory");
  return r0;
}
static long sys6(long n, long a, long b, long c, long d, long e, long f) {
  register long r0 __asm__("r0") = a;
  register long r1 __asm__("r1") = b;
  register long r2 __asm__("r2") = c;
  register long r3 __asm__("r3") = d;
  register long r4 __asm__("r4") = e;
  register long r5 __asm__("r5") = f;
  register long r7 __asm__("r7") = n;
  __asm__ volatile("svc 0" : "+r"(r0) : "r"(r1), "r"(r2), "r"(r3), "r"(r4), "r"(r5), "r"(r7) : "memory");
  return r0;
}
#define SYS_exit 1
#define SYS_read 3
#define SYS_write 4
#define SYS_open 5
#define SYS_mmap2 192

static void out(const char *s) { sys3(SYS_write, 1, (long)s, (long)strlen(s)); }
static __attribute__((noreturn)) void quit(int code) {
  for (;;) sys3(SYS_exit, code, 0, 0);
}
static void out_u(uint64_t v) {
  char b[24];
  int n = 0;
  do b[n++] = (char)('0' + v % 10); while (v /= 10);
  char r[24];
  for (int i = 0; i < n; i++) r[i] = b[n - 1 - i];
  r[n] = 0;
  out(r);
}
static void out_hex(const uint8_t *p, uint32_t n) {
  static const char d[] = "0123456789abcdef";
  char b[3] = {0, 0, 0};
  for (uint32_t i = 0; i < n; i++) {
    b[0] = d[p[i] >> 4];
    b[1] = d[p[i] & 15];
    out(b);
  }
}
static int hexval(char c) {
  return c >= '0' && c <= '9' ? c - '0' : c >= 'a' && c <= 'f' ? c - 'a' + 10 : c >= 'A' && c <= 'F' ? c - 'A' + 10 : -1;
}

/* ── The Opta's program layout (devices/arduino-opta.toml, version 2) ── */
static const plcc_image_expect_t expect = {
    .target_id = "arduino-opta",
    .target_version_min = 2,
    .target_version_max = 2,
    .abi = 1,
    .slot_addr = 0x08180000u,
    .slot_size = 0x80000u,
    .ram_addr = 0x20010000u,
    .ram_size = 0x10000u,
    .services = 185,
    .image_i = 18,
    .image_q = 1,
    .image_m = 64,
};

/* ── Services the runtime provides itself ────────────────────────────── */
static int64_t fake_now_ns;
int64_t plcc_monotonic_ns(void) { return fake_now_ns; }
void plcc_print(const char *msg) {
  out("print ");
  out(plcc_image_in_text(&expect, (uintptr_t)msg, 1) || plcc_image_in_ram(&expect, (uintptr_t)msg, 1) ? msg : "?");
  out("\n");
}
void plcc_fault(uint32_t code, const char *where) {
  out("fault ");
  out_u(code);
  out(" ");
  out(where && plcc_image_in_text(&expect, (uintptr_t)where, 1) ? where : "?");
  out("\n");
  quit(3);
}

static void *map_fixed(uint32_t addr, uint32_t size) {
  /* PROT_READ|PROT_WRITE|PROT_EXEC, MAP_PRIVATE|MAP_FIXED|MAP_ANONYMOUS */
  long r = sys6(SYS_mmap2, (long)addr, (long)size, 7, 0x02 | 0x10 | 0x20, -1, 0);
  if (r != (long)addr) {
    out("reject cannot map memory\n");
    quit(4);
  }
  return (void *)(uintptr_t)addr;
}

static uint32_t parse_u(const char *s) {
  uint32_t v = 0;
  while (*s >= '0' && *s <= '9') v = v * 10 + (uint32_t)(*s++ - '0');
  return v;
}

int harness_main(int argc, char **argv) {
  if (argc < 4) {
    out("usage: harness <image> <scans> <step_ms> [I hex]\n");
    return 1;
  }
  uint8_t *slot = map_fixed(expect.slot_addr, expect.slot_size);
  map_fixed(expect.ram_addr, expect.ram_size);
  memset(slot, 0xff, expect.slot_size); /* erased flash */
  long fd = sys3(SYS_open, (long)argv[1], 0, 0);
  if (fd < 0) {
    out("reject cannot open the image\n");
    return 4;
  }
  uint32_t got = 0;
  for (;;) {
    long n = sys3(SYS_read, fd, (long)(slot + got), (long)(expect.slot_size - got));
    if (n <= 0) break;
    got += (uint32_t)n;
    if (got == expect.slot_size) break;
  }
  const char *why = "";
  if (!plcc_image_check(&expect, &why)) {
    out("reject ");
    out(why);
    out("\n");
    return 2;
  }
  out("check ok\n");
  plcc_get_app_fn get_app = plcc_image_prepare(&expect, &plcc_service_table);
  const plcc_app_t *app = get_app();
  if (!plcc_image_check_app(&expect, app, &why)) {
    out("reject ");
    out(why);
    out("\n");
    return 2;
  }
  out("app tasks=");
  out_u(app->task_count);
  out("\n");
  const plcc_process_image_t *img = app->image;
  if (argc > 4) {
    const char *h = argv[4];
    for (uint32_t i = 0; h[0] && h[1] && i < img->input_size; i++, h += 2)
      img->input[i] = (uint8_t)(hexval(h[0]) << 4 | hexval(h[1]));
  }
  app->init();

  uint32_t scans = parse_u(argv[2]), step_ms = parse_u(argv[3]);
  int64_t next_due[PLCC_MAX_TASKS] = {0};
  uint8_t prev_single[PLCC_MAX_TASKS] = {0};
  for (uint32_t k = 0; k < scans; k++) {
    fake_now_ns = (int64_t)k * step_ms * 1000000;
    int64_t now = fake_now_ns;
    uint8_t due[PLCC_MAX_TASKS] = {0};
    int any = 0;
    for (uint32_t t = 0; t < app->task_count; t++) { /* the runtime's scheduling */
      const plcc_task_t *task = &app->tasks[t];
      uint8_t s = task->single ? task->single() : 0;
      if (task->single && s && !prev_single[t]) due[t] = 1;
      prev_single[t] = s;
      if (task->interval_ns > 0 && !s && now >= next_due[t]) {
        due[t] = 1;
        next_due[t] = next_due[t] + task->interval_ns > now ? next_due[t] + task->interval_ns : now + task->interval_ns;
      }
      if (task->interval_ns == 0 && !task->single) due[t] = 1;
    }
    for (;;) {
      uint32_t best = app->task_count;
      for (uint32_t t = 0; t < app->task_count; t++)
        if (due[t] && (best == app->task_count || app->tasks[t].priority < app->tasks[best].priority)) best = t;
      if (best == app->task_count) break;
      due[best] = 0;
      app->run_task(best);
      any = 1;
    }
    if (!any) continue;
    out("scan ");
    out_u(k);
    out(" t=");
    out_u((uint64_t)k * step_ms);
    out(" Q=");
    out_hex(img->output, img->output_size);
    out(" M=");
    out_hex(img->memory, img->memory_size);
    out("\n");
  }
  return 0;
}

/* Entry: the kernel leaves argc at [sp], argv after it. */
__attribute__((naked, noreturn)) void _start(void) {
  __asm__ volatile(
      "ldr r0, [sp]\n"
      "add r1, sp, #4\n"
      "bl harness_start\n");
}

__attribute__((noreturn, used)) void harness_start(int argc, char **argv) { quit(harness_main(argc, argv)); }
