// SPDX-License-Identifier: MPL-2.0
// STM32F4 + Modbus RTU slave + plcc water treatment PLC
// USART2 = Modbus RTU (slave addr 1, 9600 baud)
// USART1 = debug console (115200)

#include <stdint.h>
#include "modbus_rtu.h"

void *memset(void *s, int c, unsigned long n) {
    uint8_t *p = (uint8_t *)s; while (n--) *p++ = (uint8_t)c; return s;
}
void *memcpy(void *d, const void *s, unsigned long n) {
    uint8_t *dp = (uint8_t *)d; const uint8_t *sp = (const uint8_t *)s;
    while (n--) *dp++ = *sp++; return d;
}

// STM32F4 registers
#define RCC_AHB1ENR  (*(volatile uint32_t *)0x40023830)
#define RCC_APB1ENR  (*(volatile uint32_t *)0x40023840)
#define RCC_APB2ENR  (*(volatile uint32_t *)0x40023844)
#define GPIOA_MODER  (*(volatile uint32_t *)0x40020000)
#define GPIOA_AFRL   (*(volatile uint32_t *)0x40020020)
#define GPIOA_AFRH   (*(volatile uint32_t *)0x40020024)
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

static volatile uint32_t tick_ms = 0;

static void hw_init(void) {
    RCC_AHB1ENR |= 1;       // GPIOA
    RCC_APB1ENR |= (1<<17); // USART2
    RCC_APB2ENR |= (1<<4);  // USART1
    // PA2/PA3 = USART2 AF7
    GPIOA_MODER = (GPIOA_MODER & ~(0xF << 4)) | (0xA << 4);
    GPIOA_AFRL = (GPIOA_AFRL & ~(0xFF << 8)) | (0x77 << 8);
    // PA9/PA10 = USART1 AF7
    GPIOA_MODER = (GPIOA_MODER & ~(0xF << 18)) | (0xA << 18);
    GPIOA_AFRH = (GPIOA_AFRH & ~(0xFF << 4)) | (0x77 << 4);
    USART2_BRR = 0x0683; USART2_CR1 = (1<<13)|(1<<3)|(1<<2); // 9600, TE+RE
    USART1_BRR = 0x008B; USART1_CR1 = (1<<13)|(1<<3);        // 115200, TE
    SYSTICK_LOAD = 16000 - 1; SYSTICK_VAL = 0; SYSTICK_CTRL = 7; // 1ms tick
}

static void dbg(const char *s) { while(*s) { while(!(USART1_SR&(1<<7))); USART1_DR=*s++; } }
static void dbg_i(int16_t v) {
    if(v<0){while(!(USART1_SR&(1<<7)));USART1_DR='-';v=-v;}
    char t[8];int i=0;if(!v)t[i++]='0';while(v>0){t[i++]='0'+v%10;v/=10;}
    while(i>0){while(!(USART1_SR&(1<<7)));USART1_DR=t[--i];}
}
static void mb_tx(const uint8_t *d, uint16_t n) {
    for(uint16_t i=0;i<n;i++){while(!(USART2_SR&(1<<7)));USART2_DR=d[i];}
}

// Compiled ST
extern void watertreatment_init(void *state);
extern void watertreatment_scan(void *state);

static uint8_t plc_state[4096];
static int16_t rd16(uint16_t off) { return (int16_t)(plc_state[off]|(plc_state[off+1]<<8)); }
static void wr16(uint16_t off, int16_t v) { plc_state[off]=v&0xFF; plc_state[off+1]=(v>>8)&0xFF; }

// Modbus register callbacks
static int mb_read(uint16_t a, uint16_t *v, void *c) {
    (void)c;
    if (a < 6) { *v = (uint16_t)rd16(a * 2); return 0; }
    if (a >= 100 && a <= 108) { *v = (uint16_t)rd16(2 + (a-100)*2); return 0; }
    return -1;
}
static int mb_write(uint16_t a, uint16_t v, void *c) {
    (void)c;
    if (a < 6) { wr16(a*2, (int16_t)v); return 0; }
    return -1;
}
static int mb_coil(uint16_t a, uint8_t *v, void *c) { (void)a;(void)c;*v=0;return 0; }

// Vectors
static uint8_t stack[4096] __attribute__((section(".stack")));
void _start(void) __attribute__((noreturn));
void SysTick_Handler(void);
void Default_Handler(void) { while(1); }

__attribute__((section(".vectors")))
const void *vectors[] = {
    (void*)((uint8_t*)stack+sizeof(stack)), (void*)_start,
    (void*)Default_Handler, (void*)Default_Handler, 0,0,0,0,0,0,0,
    (void*)Default_Handler, 0,0, (void*)Default_Handler, (void*)SysTick_Handler,
};

void SysTick_Handler(void) { tick_ms++; }

void _start(void) {
    hw_init();
    memset(plc_state, 0, sizeof(plc_state));
    watertreatment_init(plc_state);

    modbus_rtu_t mb;
    modbus_rtu_init(&mb, 1, mb_read, mb_write, mb_coil, 0);

    dbg("plcc PLC: water treatment + Modbus RTU\r\n");
    dbg("Slave=1 USART2@9600  Debug=USART1@115200\r\n");

    uint32_t last_scan=0, last_rx=0, scans=0;

    while (1) {
        if (USART2_SR & (1<<5)) {
            modbus_rtu_rx_byte(&mb, (uint8_t)(USART2_DR & 0xFF));
            last_rx = tick_ms;
        }
        if (mb.rx_len > 0 && (tick_ms - last_rx) > 4) {
            uint16_t n = modbus_rtu_process(&mb);
            if (n > 0) mb_tx(mb.tx_buf, n);
            modbus_rtu_rx_reset(&mb);
        }
        if ((tick_ms - last_scan) >= 10) {
            last_scan = tick_ms;
            watertreatment_scan(plc_state);
            scans++;
            if (scans % 100 == 0) {
                dbg("s="); dbg_i((int16_t)(scans&0x7FFF));
                dbg(" m="); dbg_i(rd16(0));
                dbg(" c="); dbg_i(rd16(2));
                dbg("\r\n");
            }
        }
    }
}

void __exidx_start(void) {}
void __exidx_end(void) {}
void abort(void) { while(1); }
