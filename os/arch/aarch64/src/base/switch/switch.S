// AArch64 context switch.
//
// Layout of TaskContext (16 × 8 = 128 bytes on the kernel stack):
//
//   Offset   Content
//   ------   -------
//   0x00     lr  (x30)       – return address
//   0x08     x19              – current-task TCB pointer (== RISC-V "tp")
//   0x10     x20              – callee-saved
//   0x18     x21
//   0x20     x22
//   0x28     x23
//   0x30     x24
//   0x38     x25
//   0x40     x26
//   0x48     x27
//   0x50     x28
//   0x58     fp  (x29)       – frame pointer
//   0x60     (reserved)
//   0x68     (reserved)
//   0x70     ttbr1_el1        – kernel page-table root
//   0x78     ttbr0_el1        – user page-table root
//
// Convention:
//   x19           = current-task TCB pointer (points to struct where
//                    offset 0 holds the saved kernel stack pointer).
//   x0            = argument: `next_task_kernel_stack` — points to the
//                    **saved context area** of the next task on its stack.
//
// After the switch x19 points to the **next** task's TCB; `ret` jumps
// to the lr value that was stored in its context (so the next task
// resumes right after its own last call to __switch).

.section .text
.globl __switch

__switch:
    // 1.  Allocate the context area on the current kernel stack
    sub  sp, sp, #128

    // 2.  Save callee-saved registers (x19–x30) using store-pair
    stp  lr,  x19, [sp, #0x00]
    stp  x20, x21, [sp, #0x10]
    stp  x22, x23, [sp, #0x20]
    stp  x24, x25, [sp, #0x30]
    stp  x26, x27, [sp, #0x40]
    stp  x28, fp,  [sp, #0x50]

    //    Save kernel page-table root at slot 14 (0x70)
    mrs  x9, ttbr1_el1
    str  x9, [sp, #0x70]

    //    Save user page-table root at slot 15 (0x78)
    mrs  x9, ttbr0_el1
    str  x9, [sp, #0x78]

    // 3.  Write current SP into TCB (x19 points to the TCB; TCB[0] = sp)
    //     A64 does not allow SP as a source register for STR; bounce
    //     through a temporary general-purpose register.
    mov  x10, sp
    str  x10, [x19]

    // ----  Now x0 points to the next task's saved context  ----

    // 4.  Restore callee-saved registers from next context
    ldp  lr,  x19, [x0, #0x00]
    ldp  x20, x21, [x0, #0x10]
    ldp  x22, x23, [x0, #0x20]
    ldp  x24, x25, [x0, #0x30]
    ldp  x26, x27, [x0, #0x40]
    ldp  x28, fp,  [x0, #0x50]

    //    Restore TTBR1_EL1
    ldr  x9, [x0, #0x70]
    msr  ttbr1_el1, x9

    //    Restore TTBR0_EL1
    ldr  x9, [x0, #0x78]
    msr  ttbr0_el1, x9
    //    Barrier required after TTBR writes
    dsb  ish
    isb
    //    Invalidate entire TLB for the new ASID/VMID context
    tlbi vmalle1is
    dsb  ish
    isb

    // 5.  Set SP to the top of the next task's context area on its stack
    //     (x0 + 128 = next-task SP after its context was saved)
    add  sp, x0, #128

    // 6.  Return — lr now holds next task's saved return address
    ret
