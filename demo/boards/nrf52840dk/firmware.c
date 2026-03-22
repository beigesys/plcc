// SPDX-License-Identifier: MPL-2.0
// plcc PLC firmware for nRF52840 DK (Cortex-M4F)
// UARTE0 = debug console

#include <stdint.h>

#ifndef PLC_STATE_SIZE
#define PLC_STATE_SIZE 4096
#endif

#define _PASTE(a, b) a##b
#define PASTE(a, b) _PASTE(a, b)

extern void PASTE(PLC_PROGRAM, _init)(void *state);
extern void PASTE(PLC_PROGRAM, _scan)(void *state);

void *memset(void *s, int c, unsigned long n) {
    uint8_t *p = (uint8_t *)s; while (n--) *p++ = (uint8_t)c; return s;
}
void *memcpy(void *d, const void *s, unsigned long n) {
    uint8_t *dp = (uint8_t *)d; const uint8_t *sp = (const uint8_t *)s;
    while (n--) *dp++ = *sp++; return d;
}

// NRF52840 UARTE0
#define UARTE0_BASE      0x40002000
#define UARTE0_STARTTX   (*(volatile uint32_t *)(UARTE0_BASE + 0x008))
#define UARTE0_TXDRDY    (*(volatile uint32_t *)(UARTE0_BASE + 0x11C))
#define UARTE0_ENABLE     (*(volatile uint32_t *)(UARTE0_BASE + 0x500))
#define UARTE0_PSELTXD   (*(volatile uint32_t *)(UARTE0_BASE + 0x50C))
#define UARTE0_PSELRXD   (*(volatile uint32_t *)(UARTE0_BASE + 0x514))
#define UARTE0_BAUDRATE  (*(volatile uint32_t *)(UARTE0_BASE + 0x524))
#define UARTE0_TXD_PTR   (*(volatile uint32_t *)(UARTE0_BASE + 0x544))
#define UARTE0_TXD_MAXCNT (*(volatile uint32_t *)(UARTE0_BASE + 0x548))

// NRF52840 GPIO
#define GPIO_BASE        0x50000000
#define GPIO_DIRSET      (*(volatile uint32_t *)(GPIO_BASE + 0x518))
#define GPIO_OUTSET      (*(volatile uint32_t *)(GPIO_BASE + 0x508))
#define GPIO_OUTCLR      (*(volatile uint32_t *)(GPIO_BASE + 0x50C))

// LED1 = P0.13
#define LED1_PIN 13

static char tx_buf[128];

static void hw_init(void) {
    // LED
    GPIO_DIRSET = (1 << LED1_PIN);
    GPIO_OUTSET = (1 << LED1_PIN); // LEDs are active-low on nRF DK

    // UARTE0: P0.6=TX, P0.8=RX, 115200
    UARTE0_PSELTXD = 6;
    UARTE0_PSELRXD = 8;
    UARTE0_BAUDRATE = 0x01D7E000; // 115200
    UARTE0_ENABLE = 8; // Enable UARTE
}

static void dbg(const char *s) {
    int len = 0;
    while (s[len] && len < 127) { tx_buf[len] = s[len]; len++; }
    UARTE0_TXD_PTR = (uint32_t)tx_buf;
    UARTE0_TXD_MAXCNT = len;
    UARTE0_STARTTX = 1;
    volatile int d = 5000; while (d-- > 0); // Wait for TX
}

static void dbg_i(int32_t v) {
    char t[12]; int i = 0;
    if (v < 0) { tx_buf[0] = '-'; v = -v; /* simplified */ }
    if (!v) t[i++] = '0';
    while (v > 0) { t[i++] = '0' + v % 10; v /= 10; }
    int pos = 0;
    while (i > 0) tx_buf[pos++] = t[--i];
    UARTE0_TXD_PTR = (uint32_t)tx_buf;
    UARTE0_TXD_MAXCNT = pos;
    UARTE0_STARTTX = 1;
    volatile int d = 5000; while (d-- > 0);
}

static uint8_t plc_state[PLC_STATE_SIZE];
static uint8_t stack[4096] __attribute__((section(".stack")));
void _start(void) __attribute__((noreturn));
void Default_Handler(void) { while(1); }

__attribute__((section(".vectors")))
const void *vectors[] = {
    (void *)((uint8_t *)stack + sizeof(stack)), (void *)_start,
    (void *)Default_Handler, (void *)Default_Handler,
};

void _start(void) {
    hw_init();
    memset(plc_state, 0, PLC_STATE_SIZE);
    PASTE(PLC_PROGRAM, _init)(plc_state);

    dbg("plcc PLC on nRF52840 DK (Cortex-M4F)\r\n");

    uint32_t scans = 0;
    while (1) {
        PASTE(PLC_PROGRAM, _scan)(plc_state);
        scans++;
        if (scans & 1) GPIO_OUTCLR = (1 << LED1_PIN);
        else           GPIO_OUTSET = (1 << LED1_PIN);
        if (scans % 500 == 0) { dbg("scan="); dbg_i(scans); dbg("\r\n"); }
    }
}

// Called by compiled ST code — PRINT('message')
void plcc_print(const char *msg) {
    dbg(msg);
    dbg("\r\n");
}

void __exidx_start(void) {}
void __exidx_end(void) {}
void abort(void) { while(1); }
