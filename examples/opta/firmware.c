// SPDX-License-Identifier: MPL-2.0

// Arduino Opta bare-metal shim around a plcc-compiled Structured Text program.
//
// There is no Arduino core, no mbed, and no libc here. plcc emits `<pou>_init`
// and `<pou>_scan` as freestanding functions with no .data and no .bss, so all
// this file has to provide is a vector table, a stack, the GPIO the program
// drives, and a loop that calls scan forever.

#include <stdint.h>

// ---------------------------------------------------------------------------
// STM32H747 registers (RM0399). Only what this example touches.
// ---------------------------------------------------------------------------

#define RCC_AHB4ENR (*(volatile uint32_t *)0x580244E0u)
#define GPIOI_BASE 0x58022000u
#define GPIOI_MODER (*(volatile uint32_t *)(GPIOI_BASE + 0x00u))
#define GPIOI_BSRR (*(volatile uint32_t *)(GPIOI_BASE + 0x18u))

#define SCB_VTOR (*(volatile uint32_t *)0xE000ED08u)
#define SCB_CPACR (*(volatile uint32_t *)0xE000ED88u)

// Status LEDs on the front panel, from the Arduino OPTA variant: LED_D0 is
// PI_0, LED_D1 is PI_1, LED_D2 is PI_3. The relays are also on port I
// (PI_4..PI_7) and are deliberately left untouched — they are mechanical parts
// with a finite cycle count, and a scan-rate blink would chew through them.
#define LED_D0_PIN 0u
#define LED_D1_PIN 1u
#define LED_D2_PIN 3u

// ---------------------------------------------------------------------------
// The compiled ST program.
//
// plcc emits no header, so the state struct's size and layout are established
// by reading the generated IR: blink.st lowers to `{ i8, i32 }`, which is 8
// bytes with `led0` at offset 0. The buffer is oversized on purpose so that
// editing the ST cannot silently overflow it.
// ---------------------------------------------------------------------------

extern void optablink_init(void *state);
extern void optablink_scan(void *state);

#define PLC_STATE_LED0_OFFSET 0u

static _Alignas(8) volatile uint8_t plc_state[64];

// ---------------------------------------------------------------------------

static void gpio_init(void) {
    RCC_AHB4ENR |= (1u << 8); // GPIOIEN
    (void)RCC_AHB4ENR;        // read back: the clock is not live until it lands

    // MODER is two bits per pin; 0b01 selects general-purpose output.
    uint32_t moder = GPIOI_MODER;
    moder &= ~((3u << (LED_D0_PIN * 2)) | (3u << (LED_D1_PIN * 2)) |
               (3u << (LED_D2_PIN * 2)));
    moder |= (1u << (LED_D0_PIN * 2)) | (1u << (LED_D1_PIN * 2)) |
             (1u << (LED_D2_PIN * 2));
    GPIOI_MODER = moder;
}

// BSRR sets a pin from the low half and clears it from the high half, so a
// write is atomic and needs no read-modify-write.
static void led_write(uint32_t pin, int on) {
    GPIOI_BSRR = on ? (1u << pin) : (1u << (pin + 16));
}

int main(void) {
    gpio_init();

    // Solid LED_D2 means "firmware reached main" — it distinguishes a board
    // that never booted from one whose ST program simply is not toggling.
    led_write(LED_D2_PIN, 1);

    optablink_init((void *)plc_state);

    for (;;) {
        optablink_scan((void *)plc_state);
        led_write(LED_D0_PIN, plc_state[PLC_STATE_LED0_OFFSET] != 0);
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

extern uint32_t _sidata, _sdata, _edata, _sbss, _ebss, _estack;

void Reset_Handler(void) {
    // Enable the FPU before anything else can execute a VFP instruction.
    //
    // The Cortex-M7 coprocessor is disabled out of reset. plcc now targets the
    // hardware FPU (`--cpu cortex-m7` emits vmul.f64 rather than calling
    // __muldf3), so the first float operation would take a UsageFault on a chip
    // where CP10/CP11 were never granted full access.
    SCB_CPACR |= (0xFu << 20);
    __asm volatile("dsb");
    __asm volatile("isb");

    // The bootloader's vector table is still installed; point at ours.
    SCB_VTOR = 0x08040000u;

    uint32_t *src = &_sidata;
    for (uint32_t *dst = &_sdata; dst < &_edata;) {
        *dst++ = *src++;
    }
    for (uint32_t *dst = &_sbss; dst < &_ebss;) {
        *dst++ = 0;
    }

    main();
    for (;;) {
    }
}

void Default_Handler(void) {
    for (;;) {
    }
}

// A fault lights all three LEDs solid, so a hard fault is visible on the bench
// without a debugger attached.
void HardFault_Handler(void) {
    RCC_AHB4ENR |= (1u << 8);
    GPIOI_MODER |= (1u << (LED_D0_PIN * 2)) | (1u << (LED_D1_PIN * 2)) |
                   (1u << (LED_D2_PIN * 2));
    GPIOI_BSRR = (1u << LED_D0_PIN) | (1u << LED_D1_PIN) | (1u << LED_D2_PIN);
    for (;;) {
    }
}

// The bootloader validates the first word as an initial stack pointer before
// jumping, so the table must lead with _estack.
__attribute__((section(".isr_vector"), used)) void *const vector_table[] = {
    (void *)&_estack,      // 0  initial SP
    (void *)Reset_Handler, // 1  Reset
    (void *)Default_Handler, // 2  NMI
    (void *)HardFault_Handler, // 3  HardFault
    (void *)HardFault_Handler, // 4  MemManage
    (void *)HardFault_Handler, // 5  BusFault
    (void *)HardFault_Handler, // 6  UsageFault
    0, 0, 0, 0,                // 7-10 reserved
    (void *)Default_Handler,   // 11 SVCall
    (void *)Default_Handler,   // 12 DebugMonitor
    0,                         // 13 reserved
    (void *)Default_Handler,   // 14 PendSV
    (void *)Default_Handler,   // 15 SysTick
};
