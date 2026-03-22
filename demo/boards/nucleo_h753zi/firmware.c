// SPDX-License-Identifier: MPL-2.0
// plcc PLC firmware for Nucleo H753ZI (Cortex-M7)
// USART3 = debug (ST-Link VCP on Nucleo), USART2 = Modbus RTU

#include <stdint.h>

#ifndef PLC_STATE_SIZE
#define PLC_STATE_SIZE 4096
#endif
#ifndef PLC_MODBUS_ADDR
#define PLC_MODBUS_ADDR 1
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

// STM32H7 registers
#define RCC_AHB4ENR  (*(volatile uint32_t *)0x580244E0)
#define RCC_APB1LENR (*(volatile uint32_t *)0x580244E8)

#define GPIOB_MODER  (*(volatile uint32_t *)0x58020400)
#define GPIOB_ODR    (*(volatile uint32_t *)0x58020414)

// USART3 = debug (PD8/PD9 on Nucleo ST-Link VCP)
#define USART3_BASE  0x40004800
#define USART3_ISR   (*(volatile uint32_t *)(USART3_BASE + 0x1C))
#define USART3_TDR   (*(volatile uint32_t *)(USART3_BASE + 0x28))
#define USART3_BRR   (*(volatile uint32_t *)(USART3_BASE + 0x0C))
#define USART3_CR1   (*(volatile uint32_t *)(USART3_BASE + 0x00))

// USART2 = Modbus RTU
#define USART2_BASE  0x40004400
#define USART2_ISR   (*(volatile uint32_t *)(USART2_BASE + 0x1C))
#define USART2_RDR   (*(volatile uint32_t *)(USART2_BASE + 0x24))
#define USART2_TDR   (*(volatile uint32_t *)(USART2_BASE + 0x28))
#define USART2_BRR   (*(volatile uint32_t *)(USART2_BASE + 0x0C))
#define USART2_CR1   (*(volatile uint32_t *)(USART2_BASE + 0x00))

static void hw_init(void) {
    RCC_AHB4ENR |= (1<<1) | (1<<3);   // GPIOB, GPIOD
    RCC_APB1LENR |= (1<<17) | (1<<18); // USART2, USART3

    // LED: PB0 (green), PB14 (red)
    GPIOB_MODER = (GPIOB_MODER & ~(3<<0) & ~(3<<28)) | (1<<0) | (1<<28);

    // USART3 debug — simplified, Renode handles UART model
    USART3_BRR = 0x008B;
    USART3_CR1 = (1<<0) | (1<<3); // UE + TE

    // USART2 Modbus
    USART2_BRR = 0x0683;
    USART2_CR1 = (1<<0) | (1<<3) | (1<<2); // UE + TE + RE
}

static void dbg(const char *s) { while (*s) { USART3_TDR = *s++; volatile int d=50; while(d-->0); } }
static void dbg_i(int32_t v) {
    if (v < 0) { USART3_TDR = '-'; v = -v; }
    char t[12]; int i = 0;
    if (!v) t[i++] = '0';
    while (v > 0) { t[i++] = '0' + v % 10; v /= 10; }
    while (i > 0) { USART3_TDR = t[--i]; volatile int d=50; while(d-->0); }
}

#ifdef PLC_MODBUS
#include "modbus_rtu.h"
static modbus_rtu_t mb;
static int mb_read(uint16_t a, uint16_t *v, void *c) {
    uint8_t *s = (uint8_t *)c; uint16_t o = a*2;
    if (o+1 < PLC_STATE_SIZE) { *v = s[o]|(s[o+1]<<8); return 0; } return -1;
}
static int mb_write(uint16_t a, uint16_t v, void *c) {
    uint8_t *s = (uint8_t *)c; uint16_t o = a*2;
    if (o+1 < PLC_STATE_SIZE) { s[o]=v&0xFF; s[o+1]=(v>>8)&0xFF; return 0; } return -1;
}
static int mb_coil(uint16_t a, uint8_t *v, void *c) { (void)a;(void)c;*v=0;return 0; }
#endif

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

    dbg("plcc PLC on Nucleo H753ZI (Cortex-M7)\r\n");
#ifdef PLC_MODBUS
    modbus_rtu_init(&mb, PLC_MODBUS_ADDR, mb_read, mb_write, mb_coil, plc_state);
    dbg("Modbus RTU addr=1 USART2@9600\r\n");
#endif

    uint32_t scans = 0;
    while (1) {
#ifdef PLC_MODBUS
        if (USART2_ISR & (1<<5)) { modbus_rtu_rx_byte(&mb, (uint8_t)(USART2_RDR & 0xFF)); }
#endif
        PASTE(PLC_PROGRAM, _scan)(plc_state);
        scans++;
        GPIOB_ODR ^= 1; // Toggle green LED
        if (scans % 500 == 0) { dbg("scan="); dbg_i(scans); dbg("\r\n"); }
    }
}

void __exidx_start(void) {}
void __exidx_end(void) {}
void abort(void) { while(1); }
