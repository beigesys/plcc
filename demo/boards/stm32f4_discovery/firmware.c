// SPDX-License-Identifier: MPL-2.0
// plcc bare-metal PLC runtime for STM32F4
//
// Generic — works with any compiled ST program. Build with:
//   plcc compile program.st -o plc.o --target thumbv7em-unknown-none-eabihf
//   arm-none-eabi-gcc -mcpu=cortex-m4 -mthumb -mfloat-abi=hard -mfpu=fpv4-sp-d16 \
//       -ffreestanding -nostdlib -DPLC_PROGRAM=myprogram -DPLC_STATE_SIZE=4096 \
//       -I. -c plcc_runtime.c -o runtime.o
//   arm-none-eabi-gcc ... -T stm32f4.ld runtime.o plc.o -o firmware.elf -lgcc
//
// Defines:
//   PLC_PROGRAM     — name of the PROGRAM (lowercase), e.g. watertreatment
//   PLC_STATE_SIZE  — state struct size in bytes (default 4096)
//   PLC_SCAN_MS     — scan interval in ms (default 10)
//   PLC_MODBUS      — if defined, enable Modbus RTU on USART2
//   PLC_MODBUS_ADDR — Modbus slave address (default 1)

#include <stdint.h>

#ifndef PLC_STATE_SIZE
#define PLC_STATE_SIZE 4096
#endif

#ifndef PLC_SCAN_MS
#define PLC_SCAN_MS 10
#endif

#ifndef PLC_MODBUS_ADDR
#define PLC_MODBUS_ADDR 1
#endif

// ── Symbol pasting to call the right init/scan ──
#define _PASTE(a, b) a##b
#define PASTE(a, b) _PASTE(a, b)

extern void PASTE(PLC_PROGRAM, _init)(void *state);
extern void PASTE(PLC_PROGRAM, _scan)(void *state);

// ── Minimal libc ──
void *memset(void *s, int c, unsigned long n) {
    uint8_t *p = (uint8_t *)s;
    while (n--) *p++ = (uint8_t)c;
    return s;
}
void *memcpy(void *d, const void *s, unsigned long n) {
    uint8_t *dp = (uint8_t *)d;
    const uint8_t *sp = (const uint8_t *)s;
    while (n--) *dp++ = *sp++;
    return d;
}

// ── STM32F4 registers ──
#define RCC_AHB1ENR  (*(volatile uint32_t *)0x40023830)
#define RCC_APB1ENR  (*(volatile uint32_t *)0x40023840)
#define RCC_APB2ENR  (*(volatile uint32_t *)0x40023844)
#define GPIOA_MODER  (*(volatile uint32_t *)0x40020000)
#define GPIOA_AFRL   (*(volatile uint32_t *)0x40020020)
#define GPIOA_AFRH   (*(volatile uint32_t *)0x40020024)
#define GPIOD_MODER  (*(volatile uint32_t *)0x40020C00)
#define GPIOD_ODR    (*(volatile uint32_t *)0x40020C14)
#define USART1_SR    (*(volatile uint32_t *)0x40011000)
#define USART1_DR    (*(volatile uint32_t *)0x40011004)
#define USART1_BRR   (*(volatile uint32_t *)0x40011008)
#define USART1_CR1   (*(volatile uint32_t *)0x4001100C)
#define USART2_SR    (*(volatile uint32_t *)0x40004400)
#define USART2_DR    (*(volatile uint32_t *)0x40004404)
#define USART2_BRR   (*(volatile uint32_t *)0x40004408)
#define USART2_CR1   (*(volatile uint32_t *)0x4000440C)
#define SYSTICK_CTRL (*(volatile uint32_t *)0xE000E010)
#define SYSTICK_LOAD (*(volatile uint32_t *)0xE000E014)
#define SYSTICK_VAL  (*(volatile uint32_t *)0xE000E018)

static void hw_init(void) {
    RCC_AHB1ENR |= (1<<0) | (1<<3);  // GPIOA, GPIOD
    RCC_APB1ENR |= (1<<17);           // USART2
    RCC_APB2ENR |= (1<<4);            // USART1

    // PA9/PA10 = USART1 (debug), PA2/PA3 = USART2 (Modbus)
    GPIOA_MODER = (GPIOA_MODER & ~(0xF<<4) & ~(0xF<<18)) | (0xA<<4) | (0xA<<18);
    GPIOA_AFRL  = (GPIOA_AFRL & ~(0xFF<<8)) | (0x77<<8);
    GPIOA_AFRH  = (GPIOA_AFRH & ~(0xFF<<4)) | (0x77<<4);

    // PD12-15 = LEDs (output)
    GPIOD_MODER |= (1<<24) | (1<<26) | (1<<28) | (1<<30);

    USART1_BRR = 0x008B; USART1_CR1 = (1<<13)|(1<<3);           // 115200
    USART2_BRR = 0x0683; USART2_CR1 = (1<<13)|(1<<3)|(1<<2);    // 9600, TX+RX

    // SysTick: 1ms tick @ 72MHz (Renode's systickFrequency)
    SYSTICK_LOAD = 72000 - 1;
    SYSTICK_VAL = 0;
    SYSTICK_CTRL = 7;  // Enable, interrupt, processor clock
}

// ── Debug UART (USART1) ──
static void dbg_putc(char c) { while(!(USART1_SR & (1<<7))); USART1_DR = c; }
static void dbg(const char *s) { while (*s) dbg_putc(*s++); }
static void dbg_i(int32_t v) {
    if (v < 0) { dbg_putc('-'); v = -v; }
    char t[12]; int i = 0;
    if (!v) t[i++] = '0';
    while (v > 0) { t[i++] = '0' + v % 10; v /= 10; }
    while (i > 0) dbg_putc(t[--i]);
}

// ── LED helpers ──
static void led_set(int n, int on) {
    if (on) GPIOD_ODR |= (1 << (12+n));
    else    GPIOD_ODR &= ~(1 << (12+n));
}

// ── Modbus RTU (optional) ──
#ifdef PLC_MODBUS
#include "modbus_rtu.h"

static modbus_rtu_t mb;

static int mb_read(uint16_t addr, uint16_t *val, void *ctx) {
    uint8_t *state = (uint8_t *)ctx;
    uint16_t off = addr * 2;
    if (off + 1 < PLC_STATE_SIZE) {
        *val = (uint16_t)(state[off] | (state[off+1] << 8));
        return 0;
    }
    return -1;
}

static int mb_write(uint16_t addr, uint16_t val, void *ctx) {
    uint8_t *state = (uint8_t *)ctx;
    uint16_t off = addr * 2;
    if (off + 1 < PLC_STATE_SIZE) {
        state[off] = val & 0xFF;
        state[off+1] = (val >> 8) & 0xFF;
        return 0;
    }
    return -1;
}

static int mb_coil(uint16_t a, uint8_t *v, void *c) { (void)a;(void)c;*v=0;return 0; }

static void mb_tx(const uint8_t *d, uint16_t n) {
    for (uint16_t i = 0; i < n; i++) { while(!(USART2_SR & (1<<7))); USART2_DR = d[i]; }
}
#endif

// ── PLC state ──
static uint8_t plc_state[PLC_STATE_SIZE];

// ── SysTick for scan timing ──
static volatile uint32_t tick_ms = 0;
static volatile uint8_t scan_due = 0;

void SysTick_Handler(void) {
    tick_ms++;
    scan_due = 1;
}

// ── Vectors ──
static uint8_t stack[4096] __attribute__((section(".stack")));
void _start(void) __attribute__((noreturn));
void Default_Handler(void) { while(1); }

__attribute__((section(".vectors")))
const void *vectors[] = {
    (void *)((uint8_t *)stack + sizeof(stack)),
    (void *)_start,
    (void *)Default_Handler,  // NMI
    (void *)Default_Handler,  // HardFault
    0, 0, 0, 0, 0, 0, 0,     // Reserved
    (void *)Default_Handler,  // SVCall
    0, 0,                     // Reserved
    (void *)Default_Handler,  // PendSV
    (void *)SysTick_Handler,  // SysTick (vector 15)
};

// ── Main ──
void _start(void) {
    hw_init();
    memset(plc_state, 0, PLC_STATE_SIZE);
    PASTE(PLC_PROGRAM, _init)(plc_state);

#ifdef PLC_MODBUS
    modbus_rtu_init(&mb, PLC_MODBUS_ADDR, mb_read, mb_write, mb_coil, plc_state);
    dbg("plcc PLC [Modbus RTU addr=");
    dbg_i(PLC_MODBUS_ADDR);
    dbg(" USART2@9600]\r\n");
#else
    dbg("plcc PLC runtime\r\n");
#endif
    dbg("Debug: USART1@115200\r\n");
    dbg("Scan interval: ");
    dbg_i(PLC_SCAN_MS);
    dbg("ms\r\n");

    uint32_t scans = 0;
    uint32_t last_scan_ms = 0;
#ifdef PLC_MODBUS
    uint32_t last_rx_ms = 0;
#endif

    while (1) {
        // Wait for interrupt (SysTick) — lets Renode advance time properly
        __asm__ volatile ("wfi");

#ifdef PLC_MODBUS
        // Poll Modbus UART
        while (USART2_SR & (1<<5)) {
            modbus_rtu_rx_byte(&mb, (uint8_t)(USART2_DR & 0xFF));
            last_rx_ms = tick_ms;
        }
        if (mb.rx_len > 0 && (tick_ms - last_rx_ms) > 4) {
            uint16_t n = modbus_rtu_process(&mb);
            if (n > 0) mb_tx(mb.tx_buf, n);
            modbus_rtu_rx_reset(&mb);
        }
#endif

        // Run PLC scan every PLC_SCAN_MS
        if ((tick_ms - last_scan_ms) >= PLC_SCAN_MS) {
            last_scan_ms = tick_ms;

            PASTE(PLC_PROGRAM, _scan)(plc_state);
            scans++;
            led_set(0, scans & 1);

            if (scans % 100 == 0) {
                dbg("scan=");
                dbg_i((int32_t)scans);
                dbg(" t=");
                dbg_i((int32_t)tick_ms);
                dbg("ms [");
                for (int i = 0; i < 3 && (i*2+1) < PLC_STATE_SIZE; i++) {
                    if (i) dbg(",");
                    int16_t v = (int16_t)(plc_state[i*2] | (plc_state[i*2+1]<<8));
                    dbg_i(v);
                }
                dbg("]\r\n");
            }
        }
    }
}

// Called by compiled ST code — PRINT('message')
void plcc_print(const char *msg) {
    dbg(msg);
    dbg("\r\n");
}

// Stubs
void __exidx_start(void) {}
void __exidx_end(void) {}
void abort(void) { while(1); }
