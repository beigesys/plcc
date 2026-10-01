/* SPDX-License-Identifier: MPL-2.0
 * guard_test.c on QEMU mps2-an500: code in SSRAM1 at 0, the program slot at
 * 0x00100000 (loaded by QEMU), data and stacks in SSRAM2 below the program's
 * RAM window (0x20010000), the vector table copy at 0x20000000. */
MEMORY
{
  CODE (rx) : ORIGIN = 0x00000000, LENGTH = 0x100000
  RAM (rwx) : ORIGIN = 0x20001000, LENGTH = 0xB000
}
__msp_top = 0x2000E000;
__psp_top = 0x20010000;
ENTRY(Reset_Handler)
SECTIONS
{
  .text : { KEEP(*(.vectors)) *(.text*) *(.rodata*) } > CODE
  .ARM.exidx : { __exidx_start = .; *(.ARM.exidx*) __exidx_end = .; } > CODE
  .data : { _sdata = .; *(.data*) . = ALIGN(4); _edata = .; } > RAM AT > CODE
  _sidata = LOADADDR(.data);
  .bss (NOLOAD) : { _sbss = .; *(.bss*) *(COMMON) . = ALIGN(4); _ebss = .; } > RAM
  end = _ebss; /* newlib's sbrk (linked, never called) */
}
