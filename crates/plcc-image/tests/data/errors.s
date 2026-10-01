@ SPDX-License-Identifier: MPL-2.0
@ Objects the image linker must refuse (tests/link.rs). regen.sh assembles
@ each `.if CASE == n` block into errors-<n>.o.
        .syntax unified
        .thumb

        .text
.if CASE != 9
        .globl plcc_get_app
        .type plcc_get_app,%function
plcc_get_app:
.else           @ no plcc_get_app: not a plcc program
        .globl main
        .type main,%function
main:
.endif
        bx lr

.if CASE == 1   @ an import that is not a runtime service
        bl not_a_service
        bl printf
.endif

.if CASE == 2   @ static constructors
        .section .init_array,"aw",%init_array
        .word plcc_get_app
.endif

.if CASE == 3   @ thread-local storage
        .section .tbss,"awT",%nobits
        .space 4
.endif

.if CASE == 4   @ ARM-state code: R_ARM_CALL (written raw: a Cortex-M target has no ARM mode)
        .section .text.arm,"ax",%progbits
        .reloc ., R_ARM_CALL, plcc_get_app
        .word 0xebfffffe
.endif

.if CASE == 5   @ writable code
        .section .text.rw,"awx",%progbits
        bx lr
.endif

.if CASE == 6   @ .bss larger than the 64 KiB RAM window
        .bss
        .space 70000
.endif

.if CASE == 7   @ a 16-bit branch out of range
        .reloc ., R_ARM_THM_JUMP8, far
        .short 0xd0fe
        .section .text.far,"ax",%progbits
        .space 1000
        .globl far
        .type far,%function
far:    bx lr
.endif

.if CASE == 8   @ GOT-relative: R_ARM_GOT_PREL
        .section .rodata.got,"a",%progbits
        .reloc ., R_ARM_GOT_PREL, plcc_get_app
        .word 0
.endif

.if CASE == 10  @ hard-float calling convention (Tag_ABI_VFP_args = VFP registers)
        .eabi_attribute 28, 1
.endif
