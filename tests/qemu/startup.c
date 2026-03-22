// SPDX-License-Identifier: MPL-2.0
// Minimal Cortex-M3 startup for QEMU LM3S6965. Fully freestanding — no libc.

#include <stdint.h>

// ── Minimal libc replacements ──

void *memset(void *s, int c, unsigned long n) {
    uint8_t *p = (uint8_t *)s;
    while (n--) *p++ = (uint8_t)c;
    return s;
}

void *memcpy(void *dst, const void *src, unsigned long n) {
    uint8_t *d = (uint8_t *)dst;
    const uint8_t *s = (const uint8_t *)src;
    while (n--) *d++ = *s++;
    return dst;
}

static unsigned long my_strlen(const char *s) {
    unsigned long n = 0;
    while (*s++) n++;
    return n;
}

// ── ARM Semihosting ──

static inline void semihost_write0(const char *s) {
    register const char *r1 __asm__("r1") = s;
    register int r0 __asm__("r0") = 0x04; // SYS_WRITE0
    __asm__ volatile ("bkpt #0xAB" : : "r"(r0), "r"(r1) : "memory");
}

static inline void semihost_exit(int code) {
    // ADP_Stopped_ApplicationExit
    static uint32_t params[2];
    params[0] = 0x20026;
    params[1] = (uint32_t)code;
    register uint32_t *r1 __asm__("r1") = params;
    register int r0 __asm__("r0") = 0x18; // SYS_EXIT
    __asm__ volatile ("bkpt #0xAB" : : "r"(r0), "r"(r1) : "memory");
}

// ── Number to string ──

static void i16_to_str(int16_t val, char *buf) {
    if (val < 0) { *buf++ = '-'; val = -val; }
    char tmp[8];
    int i = 0;
    if (val == 0) tmp[i++] = '0';
    while (val > 0) { tmp[i++] = '0' + (val % 10); val /= 10; }
    while (i > 0) *buf++ = tmp[--i];
    *buf = '\0';
}

static void str_append(char *dst, const char *src) {
    while (*dst) dst++;
    while (*src) *dst++ = *src++;
    *dst = '\0';
}

// ── Compiled ST functions ──

extern void blink_init(void *state);
extern void blink_scan(void *state);

// State struct: { output : BOOL } = { uint8_t output }
struct blink_state {
    uint8_t output;
};

// ── Startup ──

static uint8_t stack[2048] __attribute__((section(".stack")));

void _start(void) __attribute__((noreturn));

__attribute__((section(".vectors")))
const void *vectors[] = {
    (void *)((uint8_t *)stack + sizeof(stack)),
    (void *)_start,
};

void _start(void) {
    struct blink_state state;
    memset(&state, 0, sizeof(state));
    blink_init(&state);

    semihost_write0("plcc QEMU test: blink.st on Cortex-M3\n");

    char buf[80];
    char nbuf[8];
    int pass = 1;

    for (int i = 0; i < 6; i++) {
        blink_scan(&state);

        buf[0] = '\0';
        str_append(buf, "scan ");
        i16_to_str((int16_t)i, nbuf);
        str_append(buf, nbuf);
        str_append(buf, ": output=");
        i16_to_str((int16_t)state.output, nbuf);
        str_append(buf, nbuf);
        str_append(buf, "\n");
        semihost_write0(buf);

        // Verify: odd scans should be 1, even scans should be 0
        uint8_t expected = (i % 2 == 0) ? 1 : 0;
        if (state.output != expected) {
            semihost_write0("FAIL: unexpected output value\n");
            pass = 0;
        }
    }

    if (pass) {
        semihost_write0("PASS: all 6 scans correct\n");
    } else {
        semihost_write0("FAIL\n");
    }
    semihost_exit(pass ? 0 : 1);
    __builtin_unreachable();
}

// Stubs for libgcc exception handling
void __exidx_start(void) {}
void __exidx_end(void) {}
void abort(void) { semihost_exit(99); __builtin_unreachable(); }
