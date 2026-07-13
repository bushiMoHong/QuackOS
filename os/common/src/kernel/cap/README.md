能力与权限管理器 (Capability Management)

> **来自 IPC 模块的需求：** `kernel::ipc::capability` 目前使用占位实现（始终放行）。
> 当 `kernel::cap` 模块正式实现后，需要提供以下接口供 IPC 调用，
> IPC 模块的占位函数将改为对这里的调用。

---

## IPC 对 cap 模块的接口需求

| 接口函数 | 用途 | 归属子模块 |
|----------|------|------------|
| `lookup_send_right(pid, channel_id) -> Result<(), CapError>` | 校验进程是否有权向 Channel 发送 | `cspace.rs` |
| `lookup_recv_right(pid, channel_id) -> Result<(), CapError>` | 校验进程是否有权从 Channel 接收 | `cspace.rs` |
| `lookup_call_right(pid, channel_id) -> Result<(), CapError>` | 校验进程是否有权执行 IPC Call | `cspace.rs` |
| `check_grant_chain(pid, cap) -> Result<(), CapError>` | 校验进程是否有权转让该 Capability（含派生链回溯） | `derive.rs` |
| `mint_cap(channel_id, CapType, CapRights) -> Capability` | 为新创建的 Channel 生成初始 Capability（完整权限） | `allocator.rs` |
| `derive_cap(&Capability, new_rights: CapRights) -> Option<Capability>` | 派生一个权限降级的子 Capability | `derive.rs` |

### 校验流程（send 为例）

```
ipc::sys_ipc_send(sender_pid, channel_id, msg)
  │
  ├─ cap::cspace::lookup_send_right(sender_pid, channel_id)
  │   ├─ 获取 sender_pid 的 CSpace
  │   ├─ 在 CSpace 中查找指向 channel_id 的 Capability
  │   ├─ 检查 Capability.rights 是否包含 SEND
  │   └─ 任一条件不满足 → Err(CapError)
  │
  └─ 校验通过 → 继续 IPC 数据搬运
```

### 权限位定义（与 IPC 共用）

```
READ   (1 << 0)  — 读取权限
WRITE  (1 << 1)  — 写入权限
GRANT  (1 << 2)  — 是否允许转让给其他进程
SEND   (1 << 3)  — 是否允许通过 Channel 发送
RECV   (1 << 4)  — 是否允许通过 Channel 接收
CALL   = SEND | RECV
```

### cap 模块实现后，IPC 侧的变更

`kernel::ipc::capability.rs` 当前占位：

```rust
pub fn check_send_right(sender_pid: ProcessId, channel_id: ChannelId) -> Result<(), IpcError> {
    // TODO: 对接 cap::cspace 模块，替换为：
    // cap::cspace::lookup_send_right(sender_pid, channel_id).map_err(|e| e.into())
    Ok(())
}
```

变为：

```rust
pub fn check_send_right(sender_pid: ProcessId, channel_id: ChannelId) -> Result<(), IpcError> {
    cap::cspace::lookup_send_right(sender_pid, channel_id).map_err(|e| e.into())
}
```

其他 `check_*` 函数同理，`mint_channel_cap` / `derive_cap` 改为直接调用 `cap` 模块对应函数。

---

## 原有内容

    职责： 类似 seL4 的架构，管理和校验哪个进程有权访问哪个硬件资源、内存段或 IPC 通道。

    Rust 实现： 可以完美借助 Rust 的 所有权（Ownership）和生命周期（Lifetime） 概念在内核层对系统资源的能力进行建模。

内核的 kernel/cap/ 目录下，通常会划分以下几个核心文件：

1. mod.rs —— 核心能力特征与资产定义

   负责功能： 定义系统中的能力基类（Trait）或枚举，以及统一的对外接口。

   核心逻辑： 在 Rust 中，所有具体的硬件资源/内核对象（如 Thread, Channel, PageTable）都会被包装成一种“能力”。

   核心结构体：
   Rust

   // 定义内核中所有可被授权的资源类型
   pub enum CapType {
   Untyped,     // 未分配的原始物理内存能力（一切能力的母亲）
   Endpoint,    // IPC 通信端点能力
   Thread,      // 线程控制能力
   PageTable,   // 页表控制能力
   Frame,       // 物理内存页框能力
   }

   // 每一个能力都包含：它指向哪个内核对象，以及它拥有什么权限
   pub struct Capability {
   pub obj_ptr: usize,     // 指向内核真实对象的物理/虚拟地址
   pub cap_type: CapType,  // 能力类型
   pub rights: CapRights,  // 读、写、执行、赠予等权限（位掩码）
   }

2. cspace.rs —— 能力空间与资产账本（Capability Space）

   负责功能： 每一个进程（或内核中的顶级任务）都有一个专属的“资产账本”，叫做 CSpace。这个文件负责管理这个账本的树状或表格结构。

   核心逻辑： 用户态进程在调用 Syscall 时，不能直接传内核指针，只能传一个账本索引（叫做 CPtr，类似 Linux 的文件描述符 fd）。cspace.rs 负责根据这个索引，去进程的账本里查出真正的 Capability。

   核心结构体：
   Rust

   pub struct CNode {
   // 类似多级页表，CNode 可以是多级的，存放大量的 Capability 插槽（Slots）
   pub slots: Vec<Option<Capability>>,
   pub guard: usize, // 用于路径匹配的掩码
   }

3. derive.rs —— 能力派生与权限降级（Derivation & Minting）

   负责功能： 负责能力的“繁殖”与控制。微内核的核心机制是：父进程可以把自己的资源分给子进程，但权限只能等于或小于父进程。

   核心逻辑： * 派生（Derive/Mint）： 复制一个能力，但降低其权限。例如，把一个拥有“读写”权限的内存能力，派生成一个只拥有“只读”权限的能力送给子进程。

        剥夺（Revoke）： 父进程有权收回它曾经派生出去的所有子能力。这个文件需要维护一个“能力派生树（Dependency Tree）”，确保顺着树根能把所有分支干掉。

4. allocator.rs —— 原始内存能力逆天改命（Untyped Allocator）

   负责功能： 动态内核对象的生命周期管理。

   核心逻辑： 微内核（如 seL4）为了防止内核态发生内存耗尽而崩溃，内核自身是不运行 malloc 的。

        系统刚启动时，除了内核占用的内存，剩下的所有物理内存都被打包成几个巨大的 Untyped（未定义类型）能力交给 Init 进程。

        当用户态想要创建一个新线程时，它必须调用 allocator.rs 提供的系统调用，把一块 Untyped 能力拆碎，“重塑（Retype）”成一个 Thread 能力。

        这样，内核里对象的生死和数量，完全由用户态消耗自己的物理内存资产来决定，内核只做账目记录。

## 不足

   ### 一、 当前 cap 模块如何解决 TaskStruct 存放在用户空间的难题？

      1.解决“容易被应用程序篡改（安全性挑战）”

         - 机制：LibOS 的伪造是无效的，因为微内核只认 CSpace。根据你的 cspace.rs 设计，sys_ipc_send 会调用 cap::cspace::lookup_send_right(sender_pid, channel_id)。  
         - 效果：无论 bash 怎么篡改它内存里的 TaskStruct，当它尝试发 IPC 消息给 FsServer 时，微内核只会在该进程的 CNode 表里查找是否有对应 ChannelId 的 Capability 且包含 SEND 权限。用户态无法伪造内核里的 CSpace 表。  

      2.解决“全局状态同步极其困难（如 fork 共享 fd）”

         - 机制：利用 derive.rs 中的能力派生链。
         - 效果：当 LibOS 执行 fork 时，它可以调用 derive_cap 将父进程 CSpace 中的 Endpoint 能力派生给子进程。父子进程通过各自的 CPtr 访问同一个底层的 Channel，FsServer 端收到的消息都指向同一个内部文件对象，从而天然实现了文件偏移量（offset）的同步。  

      3.解决“ProcServer 的设计压力倍增”

         - 机制：利用 derive.rs 中的 revoke 机制。
         - 效果：当父进程想要强制杀死子进程，或者回收资源时，只需要调用 revoke。由于 DERIVE_TABLE 维护了派生树，所有子进程通过该能力派生出的通信通道或内存访问权都会被瞬间切断，极大简化了 ProcServer 的回收逻辑。  

   ### 二、 为了跑通 LibOS，还需要增加哪些额外功能？

      虽然基础非常扎实，但目前的 cap 模块还有一些为了初期快速开发而留的“占位符 (placeholder)”，你需要补齐以下几块拼图：

      1. 突破单层 CNode 的容量限制（极其关键）

         - 现状：在 cspace.rs 中，目前采用的是单层平铺的 CNode，且 CSLOT_COUNT = 64。  
         - 问题：一个真实的 Linux 进程（如 Bash）不仅需要打开三个标准输入输出，还会打开几十个甚至上百个文件、动态库映射、网络 Socket 等。64 个槽位 会被瞬间耗尽。  
         - 额外需求：就像你的注释里提到的，必须升级为 多级 CSpace (multi-level CSpace)。或者，在早期阶段，至少把 CSLOT_COUNT 扩大到 1024 左右，暂时使用动态数组（alloc::vec::Vec）而非固定大小的栈上数组，以防内核栈溢出。  

      2. IPC 传递 Capability（能力授予）
         - 现状：你在 CapRights 中定义了 GRANT 权限（允许转让给其他进程）。  
         - 问题：目前没有看到 IPC 模块和 cap 模块联动实现“在发送 IPC 消息时，附带转移/复制一个 CPtr”的代码。
         - 额外需求：在 FsServer 执行 open("/etc/passwd") 成功后，FsServer 需要通过 IPC 把一个新的 Endpoint（代表这个打开的文件）传给 LibOS。微内核必须能够拦截这种特殊的 IPC 消息，在接收方进程的 CSpace 里 insert_cap，并把新的 CPtr 告诉接收方。  

      3. 实现 Untyped 内存的 Retype (动态创建内核对象)
         - 现状：你的 allocator.rs 中，retype 函数目前只是一个返回 Err(RetypeError::NotUntyped) 的占位符。  
         - 问题：在纯微内核中，LibOS 想要创建一个新线程或新的 IPC Channel 时，必须消耗自己的 Untyped 内存。目前由于 retype 没有实现，LibOS 无法动态向内核申请创建新资源。
         - 额外需求：需要补齐物理内存管理（bmm），让 retype 真正能够切分 Untyped 内存并生成 CapType::Thread 或 CapType::Endpoint。  

      4. 僵尸对象与引用计数（死亡通知）
         - 现状：当进程退出调用 destroy_cspace 时，内核只是删除了这个进程的账本。  
         - 问题：FsServer 怎么知道某个 LibOS 已经崩溃或退出了？如果不通知，FsServer 里维护的底层文件状态就会内存泄漏。
         - 额外需求：需要增加一种机制——当某个 Channel 或内核对象的所有 Capability 引用都被释放时，微内核能够自动给该对象的拥有者（如 FsServer）发送一条特殊的 IPC 死亡通知（Death Notification）。你的枚举里已经预留了 CapType::Notification，后续可以用它来实现异步事件通知。  