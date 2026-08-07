/* SPDX-License-Identifier: MPL-2.0 */

/* Arduino Opta (STM32H747XI, Cortex-M7 core).
 *
 * FLASH starts at 0x08040000, not at 0x08000000: the factory DFU bootloader
 * owns the low sectors and jumps here. Staying above it is what keeps USB
 * recovery working — and the bootloader's own sector is marked read-only in
 * the DFU descriptor, so it cannot be erased through that interface anyway.
 *
 * Stack and data live in DTCM, which needs no PLL, no MPU and no cache
 * maintenance to be usable straight out of reset.
 */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08040000, LENGTH = 768K
  DTCM  (rwx) : ORIGIN = 0x20000000, LENGTH = 128K
}

ENTRY(Reset_Handler)

SECTIONS
{
  .isr_vector : { KEEP(*(.isr_vector)) } > FLASH

  .text :
  {
    *(.text .text.*)
    *(.rodata .rodata.*)
    . = ALIGN(4);
  } > FLASH

  .data :
  {
    . = ALIGN(4);
    _sdata = .;
    *(.data .data.*)
    . = ALIGN(4);
    _edata = .;
  } > DTCM AT> FLASH
  _sidata = LOADADDR(.data);

  .bss :
  {
    . = ALIGN(4);
    _sbss = .;
    *(.bss .bss.*)
    *(COMMON)
    . = ALIGN(4);
    _ebss = .;
  } > DTCM

  /* Structured Text cannot throw and this firmware has no unwinder. plcc marks
   * its own functions nounwind; this drops the tables GCC emits for the shim. */
  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.attributes) }

  _estack = ORIGIN(DTCM) + LENGTH(DTCM);
}
