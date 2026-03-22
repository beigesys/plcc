// SPDX-License-Identifier: MPL-2.0
// STM32F4 startup for Renode. Uses USART2 for output.
// Targets: STM32F4 Discovery board (Cortex-M4)

#include <stdint.h>

// ── Minimal libc ──

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

// ── STM32F4 USART2 registers ──

#define RCC_BASE        0x40023800
#define RCC_APB1ENR     (*(volatile uint32_t *)(RCC_BASE + 0x40))
#define RCC_AHB1ENR     (*(volatile uint32_t *)(RCC_BASE + 0x30))

#define GPIOA_BASE      0x40020000
#define GPIOA_MODER     (*(volatile uint32_t *)(GPIOA_BASE + 0x00))
#define GPIOA_AFRL      (*(volatile uint32_t *)(GPIOA_BASE + 0x20))

#define USART2_BASE     0x40004400
#define USART2_SR       (*(volatile uint32_t *)(USART2_BASE + 0x00))
#define USART2_DR       (*(volatile uint32_t *)(USART2_BASE + 0x04))
#define USART2_BRR      (*(volatile uint32_t *)(USART2_BASE + 0x08))
#define USART2_CR1      (*(volatile uint32_t *)(USART2_BASE + 0x0C))

#define GPIOD_BASE      0x40020C00
#define GPIOD_MODER     (*(volatile uint32_t *)(GPIOD_BASE + 0x00))
#define GPIOD_ODR       (*(volatile uint32_t *)(GPIOD_BASE + 0x14))

static void uart_init(void) {
    // Enable GPIOA and USART2 clocks
    RCC_AHB1ENR |= (1 << 0);   // GPIOA
    RCC_AHB1ENR |= (1 << 3);   // GPIOD (LEDs)
    RCC_APB1ENR |= (1 << 17);  // USART2

    // PA2 = USART2_TX (AF7)
    GPIOA_MODER &= ~(3 << (2 * 2));
    GPIOA_MODER |= (2 << (2 * 2));  // Alternate function
    GPIOA_AFRL  &= ~(0xF << (2 * 4));
    GPIOA_AFRL  |= (7 << (2 * 4));  // AF7 = USART2

    // GPIOD12-15 as output (LEDs)
    GPIOD_MODER |= (1 << (12 * 2)) | (1 << (13 * 2)) | (1 << (14 * 2)) | (1 << (15 * 2));

    // USART2 config: 115200 baud @ 16MHz
    USART2_BRR = 0x008B;  // ~115200 @ 16MHz
    USART2_CR1 = (1 << 13) | (1 << 3);  // UE + TE
}

static void uart_putc(char c) {
    while (!(USART2_SR & (1 << 7)));  // Wait for TXE
    USART2_DR = c;
}

static void uart_puts(const char *s) {
    while (*s) uart_putc(*s++);
}

static void led_set(int led, int on) {
    // LEDs on PD12-PD15
    if (on)
        GPIOD_ODR |= (1 << (12 + led));
    else
        GPIOD_ODR &= ~(1 << (12 + led));
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

// ── Compiled ST functions ──

extern void blink_init(void *state);
extern void blink_scan(void *state);

struct blink_state {
    uint8_t output;
};

// ── Stack and vectors ──

static uint8_t stack[4096] __attribute__((section(".stack")));

void _start(void) __attribute__((noreturn));
void Default_Handler(void);

__attribute__((section(".vectors")))
const void *vectors[] = {
    (void *)((uint8_t *)stack + sizeof(stack)),  // SP
    (void *)_start,                               // Reset
    (void *)Default_Handler,                       // NMI
    (void *)Default_Handler,                       // HardFault
};

void Default_Handler(void) {
    while (1);
}

void _start(void) {
    uart_init();

    struct blink_state state;
    memset(&state, 0, sizeof(state));
    blink_init(&state);

    uart_puts("plcc Renode test: blink.st on STM32F4 (Cortex-M4)\r\n");

    char nbuf[8];
    int pass = 1;

    for (int i = 0; i < 10; i++) {
        blink_scan(&state);

        // Drive LED from ST output
        led_set(0, state.output);

        // Print result
        uart_puts("scan ");
        i16_to_str((int16_t)i, nbuf);
        uart_puts(nbuf);
        uart_puts(": output=");
        i16_to_str((int16_t)state.output, nbuf);
        uart_puts(nbuf);
        uart_puts("\r\n");

        // Verify
        uint8_t expected = (i % 2 == 0) ? 1 : 0;
        if (state.output != expected) {
            uart_puts("FAIL: unexpected value\r\n");
            pass = 0;
        }
    }

    if (pass) {
        uart_puts("PASS: all 10 scans correct on STM32F4\r\n");
    } else {
        uart_puts("FAIL\r\n");
    }

    // Halt
    while (1) { __asm__ volatile ("wfi"); }
}

// Stubs
void __exidx_start(void) {}
void __exidx_end(void) {}
void abort(void) { while (1); }
