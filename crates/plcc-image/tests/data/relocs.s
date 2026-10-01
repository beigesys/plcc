@ SPDX-License-Identifier: MPL-2.0
@ Every relocation type plcc-image supports, for comparison with ld.lld
@ (tests/lld.rs). Assemble: llvm-mc -triple=thumbv7em-none-eabi -filetype=obj
@ Targets live in other sections (or are global) so the assembler leaves
@ relocations instead of resolving them.
        .syntax unified
        .thumb

        .section .text.a,"ax",%progbits
        .globl plcc_get_app
        .type plcc_get_app,%function
        .p2align 2
plcc_get_app:
        bl far_func                     @ R_ARM_THM_CALL
        b.w far_func                    @ R_ARM_THM_JUMP24
        beq.w far_func                  @ R_ARM_THM_JUMP19
        @ The assembler widens 16-bit branches and loads to other sections,
        @ so these three are written as raw encodings with explicit relocations.
        .reloc ., R_ARM_THM_JUMP11, near_func
        .short 0xe7fe                   @ b.n  (addend -4)
        .reloc ., R_ARM_THM_JUMP8, near_func
        .short 0xd0fe                   @ beq.n (addend -4)
        .reloc ., R_ARM_THM_PC8, lit
        .short 0x48ff                   @ ldr.n r0, [pc, #imm] (addend -4)
        ldr.w r1, lit                   @ R_ARM_THM_PC12
        ldr.w r1, back_lit              @ R_ARM_THM_PC12, negative
        adr.w r2, lit                   @ R_ARM_THM_ALU_PREL_11_0
        adr.w r2, back_lit              @ R_ARM_THM_ALU_PREL_11_0, negative (SUB)
        movw r3, #:lower16:counter      @ R_ARM_THM_MOVW_ABS_NC
        movt r3, #:upper16:counter      @ R_ARM_THM_MOVT_ABS
        movw r4, #:lower16:(table - plcc_get_app)  @ R_ARM_THM_MOVW_PREL_NC
        movt r4, #:upper16:(table - plcc_get_app)  @ R_ARM_THM_MOVT_PREL
        bl memcpy                       @ a service: through a veneer
        bl plcc_fault                   @ a weak definition overridden by the service
        bx lr

        .section .text.b,"ax",%progbits
        .p2align 2
        .globl near_func
        .type near_func,%function
near_func:
        bx lr
        .p2align 2
        .globl lit
lit:    .word 0x12345678
        .globl back_lit

        .section .text.c,"ax",%progbits
        .globl far_func
        .type far_func,%function
far_func:
        nop
        bx lr

        .section .text.d,"ax",%progbits
        .weak plcc_fault
        .type plcc_fault,%function
plcc_fault:
        udf #254

        .section .rodata,"a",%progbits
        .p2align 2
        .globl table
table:
        .word far_func                  @ R_ARM_ABS32 to Thumb code (bit 0)
        .word counter + 4               @ R_ARM_ABS32 with an addend
        .word far_func - .              @ R_ARM_REL32
        .word memset                    @ R_ARM_ABS32 to a service
        .reloc ., R_ARM_TARGET1, table
        .word 0                         @ R_ARM_TARGET1
        .reloc ., R_ARM_PREL31, far_func
        .word 0                         @ R_ARM_PREL31

        .section .text.0,"ax",%progbits
        .p2align 2
back_lit:
        .word 0xcafef00d

        .data
        .p2align 2
        .globl ptrs
ptrs:
        .word table                     @ R_ARM_ABS32 in .data
        .word counter
        .word 42

        .bss
        .p2align 3
        .globl counter
counter:
        .space 8

        .comm shared, 12, 4
        .section .rodata.use,"a",%progbits
        .p2align 2
        .word shared                    @ COMMON symbol
