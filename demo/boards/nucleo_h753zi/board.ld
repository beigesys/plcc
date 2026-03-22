MEMORY {
    FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 2048K
    RAM   (rwx) : ORIGIN = 0x24000000, LENGTH = 512K
}
SECTIONS {
    .vectors : { KEEP(*(.vectors)) } > FLASH
    .text    : { *(.text*) *(.rodata*) } > FLASH
    .data    : { *(.data*) } > RAM AT > FLASH
    .bss     : { *(.bss*) *(COMMON) } > RAM
    .stack   : { *(.stack) } > RAM
    /DISCARD/ : { *(.ARM.*) *(.comment) }
}
