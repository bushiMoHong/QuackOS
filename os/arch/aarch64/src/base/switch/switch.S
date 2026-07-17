// AArch64 context switch.
//
// Layout of TaskContext (16 × 8 = 128 bytes on the kernel stack):
//
//   Offset   Content
//   ------   -------
//   0x00     lr  (x30)       – return address
//   0x08     x19              – current-task TCB pointer
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
// Arguments:
//   x0            = current task TCB pointer (points to struct where
//                    offset 0 holds the saved kernel stack pointer).
//   x1            = next_task_kernel_stack — points to the
//                    **saved context area** of the next task on its stack.
//
// On return, x0 holds next_sp so that thread_trampoline can locate
// the TrapFrame (TrapFrame is at x0 - 288).

.section .text
.globl __switch

__switch:
    // ── 1. Save current context ───────────────────────────────────────────
    sub  sp, sp, #128

    stp  lr,  x19, [sp, #0x00]
    stp  x20, x21, [sp, #0x10]
    stp  x22, x23, [sp, #0x20]
    stp  x24, x25, [sp, #0x30]
    stp  x26, x27, [sp, #0x40]
    stp  x28, fp,  [sp, #0x50]

    mrs  x9, ttbr1_el1
    str  x9, [sp, #0x70]

    mrs  x9, ttbr0_el1
    str  x9, [sp, #0x78]

    //    Store current SP → current TCB.kernel_stack_top (offset 0)
    mov  x10, sp
    str  x10, [x0]

    // ── 2. Restore next context ───────────────────────────────────────────
    ldp  lr,  x19, [x1, #0x00]
    ldp  x20, x21, [x1, #0x10]
    ldp  x22, x23, [x1, #0x20]
    ldp  x24, x25, [x1, #0x30]
    ldp  x26, x27, [x1, #0x40]
    ldp  x28, fp,  [x1, #0x50]

    // ── 3. Switch page tables ─────────────────────────────────────────────
    tlbi vmalle1is
    dsb  ish
    isb

    //    Restore TTBR1 (kernel page table root)
    ldr  x9, [x1, #0x70]
    msr  ttbr1_el1, x9

    //    Restore TTBR0 (user / isolation page table root)
    ldr  x9, [x1, #0x78]
    msr  ttbr0_el1, x9
    isb

    // ── 4. Set up the new thread's environment ────────────────────────────
    //    x0 = next_sp  (needed by thread_trampoline)
    mov  x0, x1

    //    sp = next context + 128 = next thread's stack top
    add  sp, x1, #128

    ret
