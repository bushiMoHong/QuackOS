# QuackOS 原生系统调用缺失分析

## 概述

当前 AArch64 原生系统调用（SVC #1）共 17 个，覆盖微内核最基础的 IPC + 线程 + 内存操作，但与传统微内核（seL4、Fiasco.OC/L4Re、QNX Neutrino）相比存在几类重要缺失。以下按优先级排序，只列影响系统核心能力的关键缺口。

## P0：阻塞整个系统可用性的缺失

### 1. 异步通知（Notification / Signal）

**完全没有通知机制。** 当前所有 IPC 都是同步阻塞的，无法实现异步事件驱动编程。

- 缺少 `sys_notify_send` — 向目标线程发送异步信号
- 缺少 `sys_notify_wait` — 等待异步通知
- 无法将硬件 IRQ 转发为用户态通知 → **任何用户态驱动都无法处理硬件中断**

`CapType::Notification` 和 `IpcState::BlockedOnNotify` 已在内部定义（`cap/mod.rs:68`、`ipc/synchronization.rs:40`），设计上预留了但未实现 syscall。

参考：seL4 有 `Signal` + `Wait` + `Poll` 三个通知调用；QNX 有 `MsgDeliverEvent`；Minix3 有 `SYS_IRQCTL`。

### 2. IPC 超时

`sys_ipc_recv` 和 `sys_ipc_call` **没有超时参数**。如果服务端不回复，客户端线程永久阻塞，无法恢复。

- 缺少 `sys_ipc_recv_timeout` 或在现有 IPC 调用中增加 timeout 参数
- 缺少定时器 syscall 支撑超时机制

参考：QNX `MsgReceive_r` / `MsgSend_r` 都有内置超时；seL4 有 `seL4_NBSendRecv`（非阻塞变体）。

### 3. IRQ 管理

完全无法从用户态注册/处理硬件中断。

- 缺少 `sys_irq_register` — 将 IRQ 号绑定到通知对象
- 缺少 `sys_irq_ack` — 应答中断
- 当前 `sys_console_read` 硬编码 UART 地址 `0x09000000`，是**临时方案**，不是可扩展的设备驱动模型

参考：seL4 `IRQControl_Get` + `IRQHandler_SetNotification` + `IRQHandler_Ack`；Minix3 `SYS_IRQCTL`。

## P1：capability 系统缺少对应 syscall

Capability 子系统已在 `os/common/src/kernel/cap/` 实现了一套完整的内部 API：

| 内部 API | 对应 seL4 syscall | 作用 |
|----------|-------------------|------|
| `retype()` | `Untyped_Retype` | 从 Untyped 内存创建类型化内核对象 |
| `mint_cap()` | `CNode_Mint` | 创建带受限权限的派生 capability |
| `derive_cap()` | `CNode_Copy` | 复制 capability |
| `revoke()` | `CNode_Revoke` | 撤销所有派生 capability |
| — | `CNode_Move` | 移动 capability |
| — | `CNode_Delete` | 删除 capability |

**但全部缺失对应的 syscall**。`native.rs:162-165` 的注释也标注了当前 `sys_map_page` 绕过了 CSpace 检查——属于 Phase 0 妥协。

影响：用户态进程无法创建内核对象、无法管理权限传播链，capability 系统的安全隔离能力完全无法发挥。

## P1：定时器

- 缺少 `sys_timer_create` / `sys_timer_settime` — 无法设置定时器
- 缺少 `sys_nanosleep` — 无法让线程休眠指定时间
- 这也导致 P0 的 IPC 超时没有底层机制支撑

参考：QNX `TimerCreate` / `TimerSettime`；seL4 MCS 的 `SchedContext` 机制。

## P2：线程控制面不全

当前只有 `sys_create_thread` + `sys_exit_thread` + `sys_yield`，缺少：

- **优先级设置** — 创建线程时 priority 硬编码为 128，无 `sys_thread_set_priority`
- **挂起/恢复** — 无 `sys_thread_suspend` / `sys_thread_resume`
- **寄存器读写** — 无 `sys_thread_read_regs` / `sys_thread_write_regs`（GDB stub / 用户态调试器需要）
- **Notification 绑定** — 无 `sys_thread_bind_notification`

## P2：设备 I/O 与 MMIO

- 缺少 `sys_mmio_map` — 将物理 MMIO 区域映射到用户态虚存
- `sys_map_page` 当前只分配匿名页，不处理 MMIO
- 没有 IO 端口访问（aarch64 上不需要，但架构抽象应考虑）

## P3：其他辅助调用

| 缺失项 | 说明 |
|--------|------|
| `sys_vircopy` | 跨地址空间拷贝，Minix3 核心 syscall |
| `sys_kill` / `sys_tkill` | 进程间信号发送（当前走 Linux 兼容层） |
| `sys_get_info` | 内核/系统信息查询（uname, sysinfo） |
| 页表管理 | `PageTable_Map` / `PageTable_Unmap`，多级页表用户态管理（seL4 VSpace 操作） |
| 批量映射 | `sys_map_pages`，避免大段映射时反复 trap |

## 实施建议顺序

1. **Notification** — 实现 `sys_notify_send` + `sys_notify_wait`，复用已有的 `CapType::Notification`
2. **IRQ** — 实现 `sys_irq_register` + `sys_irq_ack`，依赖 Notification 机制
3. **IPC 超时** — 给 `sys_ipc_recv` / `sys_ipc_call` 增加 timeout 参数
4. **定时器** — 实现 `sys_timer_create/settime/delete`，支撑 IPC 超时
5. **CSpace syscall** — 暴露 `retype` / `mint` / `copy` / `revoke` 为 syscall，挂接现有的 cap 子系统
6. **线程控制** — 优先级设置、挂起/恢复、寄存器读写
7. **MMIO 映射** — `sys_mmio_map` 支持通用用户态驱动
