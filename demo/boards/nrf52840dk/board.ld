MEMORY {
    FLASH (rx)  : ORIGIN = 0x00000000, LENGTH = 1024K
    RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 256K
}
SECTIONS {
    .vectors : { KEEP(*(.vectors)) } > FLASH
    .text    : { *(.text*) *(.rodata*) } > FLASH
    .data    : { *(.data*) } > RAM AT > FLASH
    .bss     : { *(.bss*) *(COMMON) } > RAM
    .stack   : { *(.stack) } > RAM
    /DISCARD/ : { *(.ARM.*) *(.comment) }
}
