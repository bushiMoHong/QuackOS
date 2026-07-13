# QuackOS Linux Syscall 兼容层实现计划

## 1. 背景与目标

### 1.1 当前状态

QuackOS 微内核已具备以下能力：

- **启动流程完整**：AArch64 汇编入口 → MMU 初始化 → ext4 文件系统挂载 → ELF 加载 → eret 进入用户态
- **陷阱框架就绪**：`vector.S` → `handler.rs` → `CommonTrapHandler`，ESR 解码、上下文保存/恢复均正常工作
- **Syscall 号表完整**：`arch/aarch64/src/syscall.rs` 已定义 Linux AArch64 syscall 0~292
- **用户态服务完善**：FsServer（ext4 文件操作）、ProcServer（进程管理）、MmServer（内存管理）已实现
- **IPC 子系统就绪**：Channel、Message、Capability 全部实现，`sys_ipc_send/recv/call` 已编写
- **调度器可用**：线程创建、就绪队列、上下文切换均可工作

**核心缺口**：陷阱处理中 syscall 分支仅打印 `unsupported syscall` 后挂起，所有 Linux syscall 均未实现。Bash 启动后执行第一条 `write()` 即死循环。

### 1.2 目标

以**用户态库（crate）形式**实现 Linux syscall 兼容层——`os/liblinux`。liblinux 作为纯粹的 LibOS，在用户态运行，通过微内核原生系统调用接口与内核及其他用户态服务通信，支持运行静态/动态链接的 Linux 程序（如 bash、coreutils）。

---

## 2. 整体架构

### 2.1 架构总览

```
┌──────────────────────────────────────────────────────────────────┐
│                        用户态 (EL0)                               │
│                                                                    │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │  Linux 二进制程序 (与 liblinux 共享地址空间)                  │  │
│  │  /bin/bash, /bin/ls, ...                                    │  │
│  │  │ SVC #0 (Linux syscall)                                   │  │
│  │  │         ┌──────────────────────────────────────┐         │  │
│  │  │         ▼                                       │         │  │
│  │  │  内核态陷阱 → 异常反射 → 用户态 handler          │         │  │
│  │  │         │                                       │         │  │
│  │  │         ▼                                       ▼         │  │
│  │  ┌──────────────────────────────────────────────────────┐   │  │
│  │  │  liblinux (用户态 LibOS)                              │   │  │
│  │  │  ┌──────────┐ ┌──────────┐ ┌──────────────────────┐  │   │  │
│  │  │  │ELF Loader│ │Syscall   │ │TaskStruct             │  │   │  │
│  │  │  │(方法A/B) │ │Dispatch  │ │(fd_table, sig, vma)   │  │   │  │
│  │  │  └──────────┘ └──────────┘ └──────────┬───────────┘  │   │  │
│  │  └───────────────────────────────────────┼──────────────┘   │  │
│  │                                          │                   │  │
│  │                          SVC #1 (原生微内核 syscall)          │  │
│  │                          │                                    │  │
│  │  ┌───────────────────────┼────────────────────────────────┐  │  │
│  │  │  用户态服务            │                                 │  │  │
│  │  │  ┌──────────┐  ┌──────┴───────┐  ┌──────────┐          │  │  │
│  │  │  │ FsServer │  │  ProcServer  │  │ MmServer │          │  │  │
│  │  │  │ (ext4)   │  │  (进程管理)   │  │ (内存管理) │          │  │  │
│  │  │  └────┬─────┘  └──────┬───────┘  └─────┬─────┘          │  │  │
│  │  └───────┼───────────────┼────────────────┼────────────────┘  │  │
│  │          └───────────────┼────────────────┘                   │  │
│  │                          │ IPC (通过内核中转)                  │  │
├──────────────────────────────┼──────────────────────────────────┤
│                        内核态 (EL1)                              │
│  ┌───────────────────────────┼──────────────────────────────┐   │
│  │  ┌────────────────────────┴────────────────────────┐     │   │
│  │  │  Trap Handler                                    │     │   │
│  │  │  SVC #0 → 异常反射 → 重定向到 liblinux handler    │     │   │
│  │  │  SVC #1 → 原生 syscall 分发 (IPC/Map/Thread/...) │     │   │
│  │  └─────────────────────────────────────────────────┘     │   │
│  │  ┌──────────────────────────────────────────────────┐    │   │
│  │  │  IPC Channel → 消息路由                           │    │   │
│  │  │  Scheduler, Page Table, Capability                │    │   │
│  │  └──────────────────────────────────────────────────┘    │   │
│  └──────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
```

### 2.2 设计原则

1. **liblinux 是纯粹的用户态库**：编译为用户态 ELF，与 Linux 程序共享同一地址空间。通过微内核原生 syscall 接口与外界通信，不链接任何内核 crate。

2. **异常反射（Exception Reflection）**：Linux 程序的 `SVC #0` 被内核捕获后，内核不处理 syscall 语义，而是将上下文打包并重定向到 liblinux 预先注册的用户态 handler。liblinux 在用户态完成 syscall 分发和执行后，通过原生 syscall 通知内核恢复 Linux 程序上下文。

3. **IPC 强制**：liblinux 从第一天起就通过 IPC 与 FsServer、ProcServer、MmServer 等用户态服务通信，不存在直接函数调用的捷径。

4. **内核无 Linux 语义**：内核不知道文件描述符、信号、brk 等 Linux 概念。这些状态全部由 liblinux 在用户态堆中维护。

5. **渐进式实现**：先让最简单的程序跑起来（`write(1, "hello", 5)`），再逐步覆盖复杂 syscall。

6. **跨架构兼容**：liblinux 面向多种 CPU 架构（AArch64、RISC-V、LoongArch），与架构相关的 syscall ABI 差异在 liblinux 内部消化。

---

## 3. Crate 结构设计与微内核接口依赖

### 3.1 新增 crate：`os/liblinux/`

```
os/liblinux/
├── Cargo.toml
└── src/
    ├── lib.rs              # 库入口，注册 handler、主循环
    ├── syscall_table.rs    # Linux syscall 号 → 处理函数的静态映射表
    ├── fs.rs               # 文件 I/O 相关 syscall (read/write/open/close/stat/...)
    ├── proc.rs             # 进程管理 syscall (exit/fork/execve/wait4/...)
    ├── mm.rs               # 内存管理 syscall (mmap/munmap/brk/...)
    ├── signal.rs           # 信号处理 syscall (sigaction/sigreturn/kill/...)
    ├── time.rs             # 时间相关 syscall (gettimeofday/clock_gettime/nanosleep/...)
    ├── misc.rs             # 杂项 syscall (getpid/getuid/uname/...)
    ├── loader.rs           # 用户态 ELF 加载器（方法A）
    ├── dynlink.rs          # 动态链接器支持（方法B）
    ├── task.rs             # Linux 任务控制块 (TaskStruct)，分配在用户态堆上
    ├── fd_table.rs         # 文件描述符表 (Linux fd → FsServer fd 映射)
    ├── errno.rs            # Linux errno 定义
    ├── ipc.rs              # 对微内核 IPC 原生 syscall 的封装
    └── native.rs           # 微内核原生 syscall 的 Rust 绑定
```

### 3.2 依赖关系

```
liblinux (用户态 crate)
  ├── xmas-elf (ELF 解析，纯用户态库)
  └── (无其他 crate 依赖)

依赖的唯一外部接口：微内核原生系统调用 (SVC #1)，遵循 Capability 权限模型
  ├── sys_map_page        → 消耗一个 Frame Capability，映射物理页到当前地址空间
  ├── sys_unmap_page      → 取消映射，归还 Frame Capability
  ├── sys_ipc_send        → 通过 Endpoint CPtr 发送 IPC 消息
  ├── sys_ipc_recv        → 通过 Endpoint CPtr 接收 IPC 消息
  ├── sys_ipc_call        → 通过 Endpoint CPtr 同步 IPC 调用（send + recv）
  ├── sys_create_thread   → 创建用户态线程，返回 Thread CPtr
  ├── sys_exit_thread     → 退出当前线程（通过 Thread CPtr）
  ├── sys_register_linux_handler → 为当前线程注册 Linux syscall 异常反射入口和 save_area
  ├── sys_linux_syscall_done     → 通知内核 Linux syscall 处理完毕，恢复原上下文
  └── sys_yield           → 主动让出 CPU
```

**关键变化**：liblinux **不依赖** `common`（内核 crate）、**不依赖** `aarch64`（arch crate）。它是一个独立的用户态 Rust 项目，仅通过 `SVC #1` 约定的原生 syscall 与微内核交互。

### 3.3 微内核原生系统调用接口定义（Capability 模型）

这是 liblinux 与微内核之间的 ABI 契约。所有接口遵循 **Capability 权限模型**——liblinux 只能通过自己 CSpace 中的 CPtr（槽位号）引用内核对象，无法使用全局 ID。内核收到 CPtr 后查表验证权限，再解析出实际的内核对象。

| 原生 syscall | 功能号 | 参数 | 返回值 | 说明 |
|---|---|---|---|---|
| `sys_map_page` | 1 | x0=frame_cptr, x1=vaddr, x2=prot_flags | x0=0成功/负errno | 消耗 frame_cptr 指向的 Frame Capability，将其物理页映射到当前 AS 的 vaddr |
| `sys_unmap_page` | 2 | x0=vaddr | x0=0成功/负errno (同时归还 Frame Capability 到当前 CSpace) | 取消 vaddr 处的映射，释放物理页，生成一个新的 Frame CPtr 插回当前 CSpace |
| `sys_ipc_send` | 3 | x0=endpoint_cptr, x1=msg_ptr, x2=msg_len | x0=0成功/负errno | 以 endpoint_cptr 的 SEND 权限向目标 Endpoint 发送消息 |
| `sys_ipc_recv` | 4 | x0=endpoint_cptr, x1=buf_ptr, x2=buf_len | x0=实际长度/负errno | 以 endpoint_cptr 的 RECV 权限从 Endpoint 接收消息（阻塞） |
| `sys_ipc_call` | 5 | x0=endpoint_cptr, x1=send_ptr, x2=send_len, x3=recv_buf, x4=recv_len | x0=实际长度/负errno | 以 endpoint_cptr 的 CALL 权限进行同步 IPC（send + 阻塞 recv） |
| `sys_create_thread` | 6 | x0=entry_pc, x1=stack_top, x2=arg, x3=tls_base | x0=thread_cptr/负errno | 创建新线程（共享当前 AS），在调用者 CSpace 中插入新 Thread CPtr 并返回其槽位号 |
| `sys_exit_thread` | 7 | x0=exit_code | 不返回 | 销毁当前线程，回收其 CSpace 和资源 |
| `sys_register_linux_handler` | 8 | x0=handler_pc, x1=save_area_vaddr | x0=0成功/负errno | **为当前线程**注册 liblinux 的 Linux syscall 异常反射入口和上下文保存区（per-thread） |
| `sys_linux_syscall_done` | 9 | x0=return_value | 不返回 | 从当前线程的 save_area 恢复原始 Linux 上下文，设置 x0=return_value，eret |
| `sys_yield` | 10 | 无 | 无 | 主动让出 CPU |

**接口设计要点**：

- **IPC 使用 Endpoint CPtr**：liblinux 不知道 FsServer 的全局 Channel ID。它只持有自己 CSpace 中的一个 Endpoint Capability 槽位号。内核查表时验证该 CPtr 是否具备 SEND/RECV/CALL 权限，再解析出背后的 Channel 进行消息路由。
- **内存映射使用 Frame CPtr**：liblinux 需要先通过 Untyped 内存分配接口获取 Frame Capability（消耗 Untyped 生成带 RWX 权限的 Frame），然后以 `(frame_cptr, vaddr, prot)` 调用 `sys_map_page`。没有 Frame CPtr 就无法映射内存。该 Frame CPtr 不能重复用于其他 vaddr。
- **线程创建返回 Thread CPtr**：`sys_create_thread` 不再返回裸 tid，而是在调用者 CSpace 中插入一个 Thread Capability 并返回槽位号。后续对该线程的操作（设置 TLS/TPIDR_EL0、挂起、销毁）都需要通过这个 CPtr。

#### 关于 Phase 0 的临时妥协

如果 Phase 0 时 Untyped 内存分配器尚未完成，`sys_map_page` 可暂时沿用简化版接口（`x0=vaddr, x1=prot_flags`，内核自行分配物理页），但必须在代码中明确标记 `// COMPROMISE: bypassing Capability check for frame allocation`。IPC 和 Thread 的 CPtr 机制则**不可妥协**——IPC 路径是微内核安全边界的基础。

**重要**：这些原生 syscall 在 Phase 1 启动前必须全部实现并就绪。这是 liblinux 开发的**硬前置依赖**。

---

## 4. 方法A：用户态 ELF 加载器（静态链接程序）

### 4.1 适用场景

- musl libc 静态编译的程序
- 不依赖动态链接器的独立 ELF
- **初始实现的首选目标**——复杂度最低，可快速验证 syscall 实现

### 4.2 内核与 liblinux 的职责划分

核心变化：**微内核只负责加载 liblinux 自身**。用户程序的加载完全在用户态完成。

```
内核态启动流程：
┌─────────────────────┐
│ 1. 内核加载 liblinux │ → 解析 liblinux 的 ELF，映射 PT_LOAD 段
│    ELF 到用户空间    │    映射用户栈，eret 到 liblinux 入口
└────────┬────────────┘
         │
         ▼   (进入用户态，liblinux 开始运行)
┌─────────────────────┐
│ 2. liblinux 初始化   │ → 调用 sys_register_linux_handler 注册异常反射入口
│                     │    初始化内部数据结构（TaskStruct 链表等）
│                     │    通过 IPC 连接到 FsServer、ProcServer、MmServer
└────────┬────────────┘
         │
         ▼
┌─────────────────────┐
│ 3. liblinux 加载    │ → 通过 IPC 向 FsServer 请求读取 /bin/bash 的 ELF 内容
│    用户程序          │    解析 ELF header、program headers
│    (/bin/bash)      │
└────────┬────────────┘
         │
         ▼
┌─────────────────────┐
│ 4. 映射 PT_LOAD 段   │ → 遍历每个 PT_LOAD 段：
│                     │    for each vaddr range:
│                     │      sys_map_page(vaddr, prot_to_flags(p_flags))
│                     │    将 ELF 数据 memcpy 到映射后的虚拟地址
│                     │    若 p_memsz > p_filesz，memset 零填充 .bss
└────────┬────────────┘
         │
         ▼
┌─────────────────────┐
│ 5. 映射用户栈        │ → sys_map_page() 分配栈页面
│                     │    按 Linux AArch64 约定布局 argc/argv/envp/auxv
└────────┬────────────┘
         │
         ▼
┌─────────────────────┐
│ 6. 初始化 TaskStruct │ → 在 liblinux 的用户态堆上分配 TaskStruct
│                     │    fd_table 预分配 0(stdin)/1(stdout)/2(stderr)
│                     │    设置 brk_start、cwd 等
└────────┬────────────┘
         │
         ▼
┌─────────────────────┐
│ 7. 跳转到 bash 入口  │ → 直接设置用户态寄存器并模拟 eret：
│    (上下文切换技巧)   │    调用 sys_create_thread 创建新线程，
│                     │    其入口为 bash 的 entry point，
│                     │    栈为刚设置好的用户栈
│                     │    或者：直接修改当前上下文跳转（更简单）
└─────────────────────┘
```

### 4.3 关键设计：liblinux 与用户程序共享地址空间

liblinux 和它加载的 Linux 程序（如 `/bin/bash`）运行在**同一地址空间**中。这意味着：

- bash 的 ELF 段直接映射到 liblinux 所在的页表
- liblinux 的用户态堆上的 TaskStruct 对 bash 不可见（bash 不知道它的存在）
- 当 bash 执行 `SVC #0`，内核重定向到 liblinux handler 时，handler 仍在同一地址空间执行，可以直接访问 TaskStruct

**地址空间布局**：

```
0x0000_0000_0000_0000  ─────────────────────────
                        Reserved (NULL guard)
0x0000_0000_0001_0000  ─────────────────────────
                        liblinux .text/.rodata/.data/.bss
                        (链接时确定具体地址)
0x0000_0000_0040_0000  ─────────────────────────
                        Linux 用户程序 ELF 段
                        (/bin/bash text/data/bss)
                       ─────────────────────────
                        liblinux 用户态堆
                        (TaskStruct、内部数据结构)
                       ─────────────────────────
                        Linux 程序 heap (brk 区域)
                       ─────────────────────────
                        mmap 区域 (向下生长)
                       ═════════════════════════
                        (gap)
                       ═════════════════════════
0x0000_7FFF_FFF0_0000  ─────────────────────────
                        Linux 程序用户栈 (向下生长)
                       ─────────────────────────
                        liblinux 内部栈
0x0000_8000_0000_0000  ─────────────────────────  (用户空间顶部)
```

### 4.4 与现有 `init.rs` 的关系

当前 `init.rs` 在内核态完成 ELF 解析和页映射。改造后：

1. 内核态的 `init.rs` 只负责加载 **liblinux** 这一个 ELF
2. 原 `init.rs` 中的 ELF 解析逻辑移入 `liblinux/loader.rs`，变为用户态代码
3. 原 `init.rs` 中的 `page_table.map()` 调用替换为 `sys_map_page()` 原生 syscall

### 4.5 辅助向量 (Auxiliary Vector)

静态链接程序最少需要的 auxiliary vector 条目（与旧版相同，但由 liblinux 在用户态构建并写入栈顶）：

| 条目 | 含义 | 必要性 |
|------|------|--------|
| `AT_PHDR` | Program headers 地址 | **必须**（libc 用此查找 TLS 等） |
| `AT_PHENT` | Program header entry 大小 | **必须** |
| `AT_PHNUM` | Program header 数量 | **必须** |
| `AT_PAGESZ` | 页面大小 (4096) | **必须** |
| `AT_ENTRY` | 入口点地址 | **必须** |
| `AT_BASE` | 解释器基址 | 静态程序填 0 |
| `AT_UID` / `AT_EUID` / `AT_GID` / `AT_EGID` | 用户/组 ID | 可选（musl 需要） |
| `AT_RANDOM` | 16 字节随机数 | 可选（stack protector） |
| `AT_NULL` | 结束标记 | **必须** |

### 4.6 栈初始化布局

与旧版一致。Linux AArch64 要求进入 `_start` 时 sp 16 字节对齐。

```
sp →  argc (8 bytes)
      argv[0], argv[1], ..., NULL (8 bytes each)
      envp[0], envp[1], ..., NULL (8 bytes each)
      auxiliary vector entries (16 bytes each, key + value)
      (padding for 16-byte alignment)
      actual string data for argv/envp
```

---

## 5. 方法B：动态链接器链式加载（动态链接程序）

### 5.1 适用场景

- 动态链接的 Linux 程序（依赖 `libc.so.6`, `ld-linux-aarch64.so.1` 等）
- 绝大多数标准 Linux 发行版程序

### 5.2 方案 B1：宿主动态链接器（推荐起步方案）

**原理**：liblinux 的 loader 解析用户 ELF 时发现 `.interp` 段，则先加载 Linux 官方的 `ld-linux-aarch64.so.1` 作为解释器，由它负责解析和加载 `libc.so` 等动态库。

加载流程（全部在用户态完成）：

```
┌──────────────┐
│ 1. 解析 ELF   │ → liblinux 通过 IPC 从 FsServer 读取 ELF header
│ 发现 .interp  │    .interp 包含解释器路径，如 "/lib/ld-linux-aarch64.so.1"
└──────┬───────┘
       │
       ▼
┌──────────────┐
│ 2. 加载解释器 │ → 通过 IPC 从 FsServer 读取 ld-linux 的 ELF
│              │    遍历 PT_LOAD，逐段 sys_map_page + memcpy
│              │    记录解释器基址 (AT_BASE)
└──────┬───────┘
       │
       ▼
┌──────────────┐
│ 3. 加载主程序 │ → 遍历主程序 PT_LOAD，逐段 sys_map_page + memcpy
└──────┬───────┘
       │
       ▼
┌──────────────┐
│ 4. 设置栈    │ → 构建 argc/argv/envp/auxv
│              │    AT_ENTRY = 主程序入口
│              │    AT_BASE  = 解释器基址
│              │    AT_PHDR  = 主程序 phdr 地址
└──────┬───────┘
       │
       ▼
┌──────────────┐
│ 5. 跳转到    │ → 设置线程入口为解释器的 entry point
│   解释器入口  │    动态链接器初始化完成后会自动跳转到主程序入口
└──────────────┘
```

**关键差异**：
- 静态程序：线程入口 = main_elf.e_entry
- 动态程序：线程入口 = ld_linux.e_entry，动态链接器初始化完成后跳转到 main_elf.e_entry
- 动态链接器通过 `AT_BASE`、`AT_PHDR`、`AT_ENTRY` 等辅助向量获取主程序信息

### 5.3 文件系统要求

ext4 镜像中需要包含：

```
/lib/ld-linux-aarch64.so.1       # 动态链接器
/lib/libc.so.6                    # C 库
/etc/ld.so.cache                  # (可选) 库搜索路径缓存
```

**便捷方案**：使用 Alpine Linux 的 aarch64 rootfs，或 musl libc 工具链交叉编译 `ld-musl-aarch64.so.1` + `libc.so`。

### 5.4 动态链接器需要的最小 syscall 集

动态链接器运行时会调用的 syscall（按优先级排序）：

| 优先级 | Syscall | 用途 |
|--------|---------|------|
| P0 | `read`, `write` | 读取 ELF/库文件，写错误信息 |
| P0 | `openat`, `close` | 打开/关闭 .so 文件 |
| P0 | `fstat` (或 `newfstatat`) | 获取文件大小 |
| P0 | `mmap` | 将 .so 的 PT_LOAD 段映射到内存 |
| P0 | `munmap` | 卸载不需要的映射 |
| P1 | `mprotect` | 修改映射权限（如 .text 改只读） |
| P1 | `brk` | 扩展初始堆 |
| P1 | `exit_group` | 错误时退出 |

这意味着实现动态链接支持前，**至少需要先实现上述 ~8 个 syscall**。其中 `mmap`/`munmap`/`mprotect` 在 liblinux 内部通过 `sys_map_page`/`sys_unmap_page` 实现。

---

## 6. Syscall 实现分类与策略

### 6.1 实现策略矩阵

每个 syscall 根据复杂度选择实现方式。注意：所有实现均在 liblinux 用户态完成。

| 策略 | 说明 | 示例 |
|------|------|------|
| **本地计算** | 纯计算，无需 IPC，直接在 liblinux 中完成 | `getpid`, `gettid`（从 TaskStruct 读取） |
| **委托用户态服务（IPC）** | 通过 IPC 调用 FsServer/ProcServer/MmServer | `read`, `write`, `open`, `mmap` |
| **原生 syscall 组合** | 使用微内核的 sys_map_page、sys_create_thread 等组合实现 | `brk`, `clone`, `execve` |
| **存根/返回 ENOSYS** | 暂时不实现，返回 "功能未实现" | `ptrace`, `perf_event_open` |

### 6.2 文件 I/O 类 (fs.rs)

```
write(fd, buf, len)
      │
      ▼
  ┌─────────────────────────┐
  │ 1. task->fd_table[fd]   │  → Linux fd 映射到 FsServer 内部 file_id
  │    (0=stdin,1=stdout,   │
  │     2=stderr 预分配)     │
  └────────┬────────────────┘
           │
           ▼
  ┌─────────────────────────┐
  │ 2. ipc::call(           │  → 通过 SVC #1 sys_ipc_call 发送 IPC 到 FsServer
  │    FsServer_channel,    │    IpcMessage::Write { fid, buf, len }
  │    WriteRequest{...})   │    阻塞等待 FsServer 返回结果
  └────────┬────────────────┘
           │
           ▼
      返回写入字节数或负 errno
```

**需要实现的 syscall（按优先级）**：

| 优先级 | Syscall | 说明 |
|--------|---------|------|
| P0 | `read` (63) | 读文件 → IPC 到 FsServer |
| P0 | `write` (64) | 写文件 → IPC 到 FsServer |
| P0 | `openat` (56) | 打开文件 → IPC 到 FsServer |
| P0 | `close` (57) | 关闭文件 → IPC 到 FsServer |
| P1 | `lseek` (62) | 设置文件偏移 → IPC 到 FsServer |
| P1 | `fstat` / `newfstatat` (80/79) | 获取文件信息 → IPC 到 FsServer |
| P1 | `getdents64` (61) | 目录读取 → IPC 到 FsServer |
| P1 | `readlinkat` (78) | 读取符号链接 → IPC 到 FsServer |
| P2 | `ioctl` (29) | 终端控制 → liblinux 本地模拟 / IPC |
| P2 | `fcntl` (25) | 文件控制 → 部分本地，部分 IPC |

### 6.3 进程管理类 (proc.rs)

| 优先级 | Syscall | 说明 | 实现方式 |
|--------|---------|------|----------|
| P0 | `exit` (93) | 退出当前进程 | sys_exit_thread (内核) |
| P0 | `exit_group` (94) | 退出所有线程 | sys_exit_thread + 通知 ProcServer |
| P1 | `getpid` (172) | 获取进程 ID | 直接返回 TaskStruct.pid |
| P1 | `gettid` (178) | 获取线程 ID | 直接返回 TaskStruct.tid |
| P2 | `clone` (220) | 创建新线程/进程 | sys_create_thread + 用户态地址空间复制 |
| P2 | `execve` (221) | 执行新程序 | liblinux loader 替换当前地址空间 |
| P3 | `fork` (1079) | 复制当前进程 | clone 的特例 |
| P3 | `wait4` (260) | 等待子进程 | IPC 到 ProcServer |

### 6.4 内存管理类 (mm.rs)

| 优先级 | Syscall | 说明 | 实现方式 |
|--------|---------|------|----------|
| P1 | `brk` (214) | 扩展/收缩堆 | sys_map_page 逐页映射/解除 |
| P1 | `mmap` (222) | 内存映射 | sys_map_page（匿名映射）/ IPC+sys_map_page（文件映射） |
| P1 | `munmap` (215) | 取消映射 | sys_unmap_page |
| P2 | `mprotect` (226) | 修改页面权限 | 需要内核提供 sys_mprotect 或在 liblinux 中重建映射 |

**mmap 实现要点**（在 liblinux 用户态）：

```rust
fn sys_mmap(task: &mut TaskStruct, addr: usize, len: usize, prot: u32,
            flags: u32, fd: i32, offset: u64) -> isize {
    match flags & MAP_TYPE_MASK {
        MAP_ANONYMOUS => {
            for page in aligned_range(addr, len) {
                // 通过原生 syscall 请求内核映射物理页
                let ret = native::sys_map_page(page, to_native_prot(prot));
                if ret != 0 { return ret; }
            }
        }
        MAP_FILE => {
            // 1. 在 TaskStruct.vmas 中记录 VMA 元数据
            // 2. 实际页面延迟加载（缺页时通过 IPC 从 FsServer 读取）
            task.vmas.insert(addr, VmaEntry { file_fd: fd, offset, len, prot });
        }
        _ => return -EINVAL,
    }
}
```

### 6.5 信号管理类 (signal.rs)

信号完全在用户态管理，内核不感知信号语义。

| 优先级 | Syscall | 说明 |
|--------|---------|------|
| P2 | `rt_sigaction` (134) | 设置信号处理函数 → TaskStruct.sig_handlers |
| P2 | `rt_sigprocmask` (135) | 阻塞/解除阻塞信号 → TaskStruct.sig_mask |
| P2 | `rt_sigreturn` (139) | 从信号处理函数返回 → 恢复保存的上下文 |
| P2 | `kill` (129) | 发送信号 → IPC 到 ProcServer 转发 |
| P3 | `tgkill` (131) | 向特定线程发送信号 |

**信号投递机制**：liblinux 在 Linux syscall 处理完毕、调用 `sys_linux_syscall_done` 之前，检查 TaskStruct.pending_signals。若有待处理信号，不直接恢复原 Linux 上下文，而是：
1. 在用户栈上构造 sigframe（保存原始上下文）
2. 修改上下文的 ELR 为 sig_handler，x0 为 signum
3. 再调用 `sys_linux_syscall_done`，内核恢复被修改的上下文

### 6.6 时间类 (time.rs)

| 优先级 | Syscall | 说明 |
|--------|---------|------|
| P2 | `clock_gettime` (113) | 获取当前时间 → IPC 或内核时间 syscall |
| P3 | `nanosleep` (101) | 睡眠 → 内核定时器 syscall |
| P3 | `gettimeofday` (169) | 获取时间（传统接口） |

### 6.7 杂项 (misc.rs)

| 优先级 | Syscall | 说明 |
|--------|---------|------|
| P1 | `uname` (160) | 系统信息（`uname -a` 需要） |
| P1 | `getcwd` (17) | 从 TaskStruct.cwd 返回 |
| P2 | `getuid` / `geteuid` (174/175) | 用户 ID |
| P2 | `getrandom` (278) | 随机数 |

---

## 7. TaskStruct：用户态任务控制块

### 7.1 数据结构

**TaskStruct 从内核态完全剥离**，分配在 liblinux 的用户态堆上。内核不知道文件描述符、信号、brk 等概念。

```rust
/// 每个 Linux 进程在 liblinux 的用户态堆上维护一份
pub struct TaskStruct {
    // 标识
    pub pid: u32,
    pub tid: u32,
    pub ppid: u32,

    // 文件描述符表（纯用户态数据结构）
    pub fd_table: FdTable,

    // 当前工作目录
    pub cwd: String,

    // 信号（内核不感知信号语义）
    pub sig_handlers: [SigAction; 64],
    pub sig_mask: u64,
    pub pending_signals: u64,

    // 内存管理（记录虚拟内存区域，用于 mmap/munmap/缺页处理）
    pub brk_start: usize,
    pub brk_current: usize,
    pub mmap_base: usize,
    pub vmas: BTreeMap<usize, VmaEntry>,

    // 子进程
    pub children: Vec<u32>,
    pub exit_code: Option<i32>,
}
```

### 7.2 生命周期管理

- **创建**：liblinux 的 loader 在加载 Linux 程序时，在自己的用户态堆上 `Box::new(TaskStruct { ... })`
- **访问**：liblinux 的 syscall handler 入口通过全局变量或线程局部存储获取当前 TaskStruct 指针
- **销毁**：`exit_group` 时释放 fd_table（IPC 关闭所有 fd）、清理 vmas（sys_unmap_page）、通知 ProcServer

### 7.3 FdTable 设计

**Phase 1-5 务实方案（整数 ID）**：

```rust
pub struct FdTable {
    entries: BTreeMap<i32, FdEntry>,
    next_fd: i32,
}

pub struct FdEntry {
    pub fs_file_id: u64,      // FsServer 返回的内部文件 ID
    pub flags: OpenFlags,
    pub offset: u64,
    pub path: String,         // 文件路径（用于 fstat 等）
}
```

预分配 fd 0/1/2 → 通过 IPC 打开 `/dev/tty`（或 `/dev/null`）获取对应的 fs_file_id。

**Phase 6 进阶方案（Endpoint Capability 隔离）**：

当 IPC 和 Capability 系统成熟后，FsServer 的 `open` 回复不再返回整数 ID，而是通过 IPC **传递一个动态创建的 Endpoint Capability**：

```
liblinux: open("/home/user/notes.txt", O_RDWR)
           │
           ▼ (IPC call 到 FsServer)
FsServer:  1. 在内核中创建新的 Endpoint 对象
           2. 授予该 Endpoint READ + WRITE + SEEK 权限
           3. 通过 IPC 回复将 Endpoint CPtr 传递给 liblinux
           │
           ▼
liblinux: fd_table[3] = FdEntry::Endpoint { cptr: new_endpoint_cptr }
```

此后对该文件的所有读写操作直接针对 `endpoint_cptr` 进行 IPC Call，而非通过全局 FsServer Channel 转发。优势：

- **安全隔离**：恶意进程无法在 IPC 消息中伪造 fs_file_id 访问不属于自己的文件，因为每个文件有独立的 Endpoint
- **权限细化**：以 O_RDONLY 打开的文件，其 Endpoint 仅被授予 READ 权限，内核层面不可绕过
- **并发友好**：不同文件的 I/O 操作路由到不同 Endpoint，减少单 Channel 的争用

```rust
// Phase 6 的 FdEntry
pub enum FdEntry {
    /// Phase 1-5: 整数 ID 模式
    Legacy {
        fs_file_id: u64,
        flags: OpenFlags,
        offset: u64,
        path: String,
    },
    /// Phase 6: 独立 Endpoint Capability 模式
    Endpoint {
        cptr: u32,              // 该文件的专属 Endpoint CPtr
        rights: CapRights,      // liblinux 持有的权限（READ/WRITE/SEEK）
        offset: u64,
        path: String,
    },
}

---

## 8. 异常反射机制

### 8.1 核心概念

异常反射是连接 Linux 程序与 liblinux 的关键桥梁。内核不解释 Linux syscall 语义，只负责**识别 → 打包上下文（到 per-thread save_area）→ 重定向到用户态 handler**。每个线程拥有独立的 handler 入口和 save_area，避免多线程并发 `SVC #0` 时的状态覆盖。

### 8.2 SVC 调用约定区分

| 指令 | 语义 | 处理方式 |
|------|------|----------|
| `SVC #0` | Linux syscall（标准 Linux ABI） | 异常反射到 liblinux handler |
| `SVC #1` | QuackOS 原生 syscall | 内核直接处理（IPC、Map、Thread 等） |

区分依据：AArch64 的 `ESR_EL1` 寄存器在 SVC 异常时编码了立即数值（`ISS` 字段的 bit 0-15），内核据此判断是 SVC #0 还是 SVC #1。

### 8.3 异常反射流程（per-thread save_area）

```
Linux 程序 (某线程) 执行 SVC #0
         │
         ▼
   ┌─────────────────────────────────────────────┐
   │ 内核态 Trap Handler                          │
   │                                              │
   │ 1. 读取 ESR_EL1，EC = SVC64, imm = 0         │
   │    → 判定为 Linux syscall                     │
   │                                              │
   │ 2. 查找当前线程的 per-thread save_area       │
   │    (liblinux 已通过 sys_register_linux_handler │
   │     为每个线程单独注册了 handler_pc + save_area)│
   │    将当前用户态上下文写入该线程的 save_area    │
   │                                              │
   │ 3. 修改保存的上下文：                         │
   │    ELR_EL0 = handler_pc   (liblinux 入口)     │
   │    x0      = save_area_vaddr (上下文指针)     │
   │    SP_EL0  = liblinux 内部栈顶                │
   │    SPSR_EL0保持不变                           │
   │                                              │
   │ 4. eret → 返回用户态，进入 liblinux handler   │
   └──────────────────────────────────────────────┘
         │
         ▼   (在用户态，liblinux handler 执行)
   ┌─────────────────────────────────────────────┐
   │ liblinux 用户态 handler                       │
   │                                              │
   │ 1. 从 x0 获取当前线程的 save_area 指针        │
   │ 2. 从 save_area 读取 Linux syscall 上下文：    │
   │    nr = ctx.x8, args = [ctx.x0..ctx.x5]      │
   │ 3. dispatch(nr, args, current_task())        │
   │    → 可能涉及 IPC 调用 (SVC #1 sys_ipc_call)  │
   │ 4. 调用 sys_linux_syscall_done(ret_val)       │
   │    (SVC #1，功能号 9)                         │
   └────────┬────────────────────────────────────┘
            │
            ▼
   ┌─────────────────────────────────────────────┐
   │ 内核态，sys_linux_syscall_done 的实现          │
   │                                              │
   │ 1. 从当前线程的 per-thread save_area 恢复    │
   │    原始用户态上下文                           │
   │ 2. 设置 x0 = ret_val (Linux syscall 的返回值) │
   │ 3. 推进 ELR_EL0 到 SVC 的下一条指令           │
   │ 4. eret → 返回 Linux 程序继续执行             │
   └─────────────────────────────────────────────┘
```

### 8.4 save_area 格式

```rust
/// 内核写入 per-thread save_area 的上下文结构
/// 每个线程拥有独立的 save_area（通常分配在线程的 TLS 或栈上）,
/// 避免多线程并发 SVC #0 时的状态覆盖
#[repr(C)]
pub struct LinuxContext {
    pub x0: u64,  pub x1: u64,  pub x2: u64,  pub x3: u64,
    pub x4: u64,  pub x5: u64,  pub x6: u64,  pub x7: u64,
    pub x8: u64,  // syscall number
    pub x9: u64,  pub x10: u64, pub x11: u64, pub x12: u64,
    pub x13: u64, pub x14: u64, pub x15: u64, pub x16: u64,
    pub x17: u64, pub x18: u64, pub x19: u64, pub x20: u64,
    pub x21: u64, pub x22: u64, pub x23: u64, pub x24: u64,
    pub x25: u64, pub x26: u64, pub x27: u64, pub x28: u64,
    pub x29: u64, pub x30: u64,
    pub elr: u64,
    pub spsr: u64,
    pub sp: u64,
}
```

### 8.5 改造点

**`os/common/src/kernel/trap/mod.rs`**：

```rust
// 改造后的 handle_user_sync
fn handle_user_sync(&self, tf: &mut TrapFrame) {
    let esr = read_esr_el1();
    let ec = (esr >> 26) & 0x3F;

    match ec {
        EC_SVC64 => {
            let imm = esr & 0xFFFF;
            match imm {
                0 => {
                    // Linux syscall (SVC #0) → 异常反射
                    reflect_linux_syscall(tf);
                    // reflect_linux_syscall 修改 tf 指向 save_area，
                    // 然后直接 eret 到 liblinux handler
                }
                1 => {
                    // QuackOS 原生 syscall (SVC #1) → 内核直接处理
                    let nr = tf.general.x8;
                    native_syscall_dispatch(nr, tf);
                }
                _ => {
                    // 未知 SVC 立即数 → 发送 SIGILL
                }
            }
        }
        EC_DATA_ABORT_EL0 | EC_INST_ABORT_EL0 => {
            // 缺页：也需要通过异常反射通知 liblinux
            // liblinux 检查 VMA 决定是 SIGSEGV 还是 mmap 延迟加载
            reflect_page_fault(tf);
        }
        _ => {
            // 其他异常 → 尝试反射给 liblinux，或 SIGILL
        }
    }
}
```

### 8.6 原生 syscall 分发表

```rust
// 内核态 SVC #1 分发表
fn native_syscall_dispatch(nr: u64, tf: &mut TrapFrame) {
    match nr {
        1  => sys_map_page(tf),
        2  => sys_unmap_page(tf),
        3  => sys_ipc_send(tf),
        4  => sys_ipc_recv(tf),
        5  => sys_ipc_call(tf),
        6  => sys_create_thread(tf),
        7  => sys_exit_thread(tf),
        8  => sys_register_linux_handler(tf),
        9  => sys_linux_syscall_done(tf),
        10 => sys_yield(tf),
        _  => tf.general.x0 = -ENOSYS as u64,
    }
}
```

### 8.7 save_area 的线程绑定机制（避免并发状态损坏）

**问题**：如果所有线程共享一个全局 `save_area_vaddr`，当 bash 的多个线程同时触发 `SVC #0` 时，内核会将不同线程的上下文覆盖写入同一个地址，导致状态损坏和无法调试的崩溃。

**设计**：`save_area` 与**线程**绑定，而非与进程绑定。

```
┌─────────────────────────────────────────────────────────┐
│ 内核维护的 per-thread 数据结构                            │
│                                                          │
│  struct ThreadControlBlock {                             │
│      tid: u32,                                           │
│      asid: AddressSpaceId,                               │
│      linux_handler_pc: Option<u64>,   // liblinux 入口    │
│      linux_save_area: Option<u64>,    // ↓ per-thread!    │
│      // ...                                              │
│  }                                                       │
└─────────────────────────────────────────────────────────┘
```

**注册时机**：liblinux 在创建每个 Linux 用户线程后，为该线程单独调用 `sys_register_linux_handler`：

```rust
// liblinux 在 loader.rs 中为新线程注册反射入口
fn spawn_linux_thread(entry: u64, stack: u64, task: &TaskStruct) {
    // 1. 在该线程的 TLS 或栈上分配 per-thread save_area
    let save_area = alloc_per_thread_save_area();  // 如 &thread_tls->linux_context

    // 2. 创建线程，获得 thread_cptr
    let thread_cptr = sys_create_thread(entry, stack, arg, tls_base);

    // 3. 为新线程注册异常反射入口 + 其专属 save_area
    //    内核将该信息记录到该线程的 TCB 中
    sys_register_linux_handler(handler_pc, save_area as u64);
    // 注意：此调用作用于"当前线程"！
}
```

**内核侧实现要点**：

```rust
fn sys_register_linux_handler(tf: &mut TrapFrame) {
    let handler_pc = tf.general.x0;
    let save_area = tf.general.x1;

    // 获取当前正在执行的线程的 TCB
    let current_thread = get_current_thread();
    current_thread.linux_handler_pc = Some(handler_pc);
    current_thread.linux_save_area = Some(save_area);

    tf.general.x0 = 0; // 成功
}

fn reflect_linux_syscall(tf: &mut TrapFrame) {
    let current_thread = get_current_thread();

    // 从当前线程的 TCB 中获取 per-thread save_area
    let save_area = current_thread.linux_save_area
        .expect("Linux syscall from thread without registered handler");
    let handler_pc = current_thread.linux_handler_pc.unwrap();

    // 将上下文写入该线程专属的 save_area
    write_linux_context(save_area, tf);

    // 重定向到 liblinux handler
    tf.elr = handler_pc;
    tf.general.x0 = save_area as u64;
    // ...
}
```

**多线程安全**：每个线程有独立的 TCB → 独立的 `linux_save_area` → 不同线程的上下文写入不同地址。即使线程 A 和线程 B 同时触发 `SVC #0` 并被内核序列化处理时，各自的上下文写入各自专属的 save_area，互不干扰。

**liblinux handler 的并发考虑**：因为 save_area 是 per-thread 的，liblinux handler 本身可以是**单线程**的（一次只处理一个线程的 syscall）或**多线程**的（为每个 Linux 线程维护一个 liblinux worker 线程）。初始阶段建议单线程模型——handler 处理完当前 syscall 并调用 `sys_linux_syscall_done` 后才接受下一个。内核侧在 TCB 中增加 `linux_syscall_in_progress: bool` 标志，拒绝重入。

---

## 9. IPC 集成：强制从 Phase 1 开始

### 9.1 设计原则

liblinux 与 FsServer、ProcServer、MmServer **位于不同的用户态地址空间**，所有通信必须通过微内核 IPC Channel。从 Phase 1 起即强制此约束——不存在直接函数调用的捷径。

### 9.2 liblinux 的 IPC 封装层 (`ipc.rs`)

```rust
/// liblinux 内部的 IPC 封装
pub fn fs_write(fid: u64, buf: &[u8], offset: u64) -> Result<usize, Errno> {
    let msg = IpcMessage::Write { fid, offset, data: buf };
    let reply = native::sys_ipc_call(FS_SERVER_CHANNEL, &msg)?;
    match reply {
        IpcReply::WriteResult(n) => Ok(n as usize),
        IpcReply::Error(e) => Err(e),
        _ => Err(Errno::EIO),
    }
}

pub fn fs_open(path: &str, flags: OpenFlags) -> Result<u64, Errno> {
    let msg = IpcMessage::Open { path, flags };
    let reply = native::sys_ipc_call(FS_SERVER_CHANNEL, &msg)?;
    match reply {
        IpcReply::OpenResult(fid) => Ok(fid),
        IpcReply::Error(e) => Err(e),
        _ => Err(Errno::EIO),
    }
}
```

### 9.3 Channel 连接建立

liblinux 启动时需要获取各服务器 Channel 的 capability：

1. **方式一（推荐）**：liblinux 启动时，内核将预配置的 Channel capability 通过寄存器或初始栈传递给 liblinux（类似 seL4 的 bootinfo）
2. **方式二**：liblinux 通过一个众所周知的 "名字服务" Channel 查询各服务器的 Channel ID

### 9.4 IPC 就绪前置条件

Phase 1 启动前，以下 IPC 路径必须已打通：

- [x] 内核 IPC Channel 创建/销毁
- [x] `sys_ipc_send` / `sys_ipc_recv` / `sys_ipc_call` 原生 syscall 实现
- [x] FsServer 启动并绑定到 Channel，等待 IPC 请求
- [x] liblinux 获取 FsServer Channel 的 capability
- [ ] IpcMessage/IpcReply 的序列化格式确定（需与 FsServer 协商一致）

---

## 10. 分阶段实施路线图

### Phase 0：微内核原生 Syscall 就绪（前置阶段）

**目标**：实现 liblinux 依赖的全部微内核原生 syscall（见 §3.3），打通 IPC 路径。

**工作内容**：
- 内核 `SVC #1` 分发表实现（10 个原生 syscall）
- IPC Channel 端点创建与路由就绪
- FsServer 通过 IPC Channel 接收请求并返回结果
- `sys_register_linux_handler` + `sys_linux_syscall_done` 的上下文打包/恢复逻辑
- 内核态的 `init.rs` 改为加载 liblinux ELF

**文件**：
- 改造 `os/common/src/kernel/trap/mod.rs`（SVC 立即数判断 + 原生 syscall 分发表）
- 改造 `os/common/src/usr/init.rs`（只加载 liblinux）
- 新增 `os/common/src/kernel/trap/reflect.rs`（异常反射逻辑）
- FsServer 增加 IPC 事件循环（从被动函数调用改为主动监听 Channel）

### Phase 1：最小可运行程序 (目标：打印 "hello world")

**目标**：liblinux 加载一个 musl 静态编译的 C 程序，输出 "hello world\n" 后退出。

**需实现的 Linux syscall**（在 liblinux 用户态）：

| Syscall | 编号 | 实现方式 |
|---------|------|----------|
| `write` | 64 | IPC → FsServer |
| `exit_group` | 94 | sys_exit_thread + 通知 ProcServer |
| `brk` | 214 | sys_map_page（扩展初始堆） |

**文件**：
- 新建 `os/liblinux/` crate（完整结构见 §3.1）
- liblinux 的 `loader.rs` 实现用户态 ELF 加载
- liblinux 的 `syscall_table.rs` 实现分发表
- liblinux 的 `task.rs` 实现用户态 TaskStruct
- liblinux 的 `ipc.rs` + `native.rs` 封装原生 syscall

### Phase 2：Bash 交互式运行 (目标：bash 命令行可用)

**目标**：Bash 启动后能显示提示符、读取用户输入、执行内建命令。

**需新增的 Linux syscall**：

| Syscall | 用途 |
|---------|------|
| `read` (63) | 从 stdin 读取 → IPC 到 FsServer |
| `openat` (56) | 打开文件 → IPC 到 FsServer |
| `close` (57) | 关闭 fd → IPC 到 FsServer |
| `fstat` (80) | bash 检查 /dev/tty |
| `ioctl` (29) | 终端 ioctl → liblinux 本地模拟（TIOCGPGRP 等返回 ENOTTY） |
| `rt_sigaction` (134) | 信号处理注册 → TaskStruct（初期存根） |
| `rt_sigprocmask` (135) | 信号掩码 → TaskStruct（初期存根） |
| `getpid` (172) | 从 TaskStruct 返回 |
| `getcwd` (17) | 从 TaskStruct 返回 |
| `uname` (160) | liblinux 本地构造 |

### Phase 3：基础文件操作 (目标：ls、cat、cd 可用)

**需新增的 Linux syscall**：

| Syscall | 用途 |
|---------|------|
| `getdents64` (61) | 目录枚举 → IPC |
| `lseek` (62) | 文件偏移 → IPC |
| `newfstatat` (79) | 路径 stat → IPC |
| `readlinkat` (78) | 符号链接 → IPC |
| `fcntl` (25) | dup/cloexec → 本地 TaskStruct.fd_table |
| `dup` / `dup3` (23/24) | 重定向 → 本地 fd_table 操作 |
| `chdir` (49) | 切换目录 → 更新 TaskStruct.cwd |

### Phase 4：进程管理 (目标：脚本执行、管道)

**需新增的 Linux syscall**：

| Syscall | 用途 |
|---------|------|
| `clone` (220) | 创建新进程/线程 → sys_create_thread + 用户态 AS 复制 |
| `execve` (221) | 执行新程序 → liblinux loader 替换当前 AS 内容 |
| `wait4` (260) | 等待子进程 → IPC 到 ProcServer |
| `pipe2` (59) | 创建管道 → liblinux 本地 pipe 实现 + fd_table |
| `exit` (93) | 单线程退出 → sys_exit_thread |

**重点难点**：`clone` 需要复制地址空间。在用户态实现 COW：
1. 遍历 TaskStruct.vmas，对每个私有映射在新线程中重建
2. 共享映射直接复用（同一 AS）

### Phase 5：动态链接支持 (目标：运行动态链接程序)

**需新增的 Linux syscall**：

| Syscall | 用途 |
|---------|------|
| `mmap` (222) | 动态链接器映射 .so → sys_map_page + VMA 记录 |
| `munmap` (215) | 取消映射 → sys_unmap_page + VMA 更新 |
| `mprotect` (226) | 修改映射权限 |

**额外工作**：
- loader.rs 增加 PT_INTERP 解析
- ext4 镜像包含 `ld-linux-aarch64.so.1` + `libc.so.6`

### Phase 6：完善与优化 (目标：接近原生 Linux 体验)

- 信号处理完善（SIGCHLD、SIGPIPE、SIGINT 实际投递）
- `poll` / `ppoll`（bash `read -t`）
- errno 线程安全（通过 TLS 实现 `__errno_location`）
- 性能优化（减少 IPC 往返次数、批量操作）

---

## 11. 关键技术细节

### 11.1 AArch64 Syscall ABI

```
Linux syscall (SVC #0):
  nr:    x8
  args:  x0, x1, x2, x3, x4, x5
  ret:   x0 (正数=成功, 负数= -errno)

QuackOS 原生 syscall (SVC #1):
  nr:    x8 (功能号)
  args:  x0-x5 (取决于具体功能)
  ret:   x0
```

### 11.2 liblinux handler 的入口约定

内核通过异常反射进入 liblinux handler 时：

```
x0 = save_area 的虚拟地址（指向 LinuxContext 结构体）
SP  = liblinux 内部栈（从 sys_register_linux_handler 时记录的栈顶恢复）
其他寄存器 = 未定义（handler 应从 save_area 读取所需全部信息）
```

### 11.3 用户态内存管理

liblinux 负责管理 Linux 程序的全部内存布局。内存管理 syscall 的实现路径：

- `brk(addr)` → liblinux 遍历 [brk_current, addr) 范围，逐页 `sys_map_page(vaddr, PROT_RW)` 或 `sys_unmap_page(vaddr)`
- `mmap(MAP_ANONYMOUS)` → 同上，批量 `sys_map_page`
- `mmap(MAP_FILE)` → 记录 VMA，延迟到缺页时通过 IPC 从 FsServer 读取文件数据再 `sys_map_page`

### 11.4 TLS 支持

静态链接 musl 程序需要 TPIDR_EL0 指向 TLS 区域。liblinux 从 PT_TLS program header 获取 TLS 模板：
1. 通过 sys_map_page 分配 TLS 区域
2. 将 TLS 初始化数据 memcpy 到 TLS 区域
3. 设置 TPIDR_EL0（需要内核提供原生 syscall 或在线程创建时传递此值）

### 11.5 缺页处理

缺页发生时，内核通过异常反射将上下文传递给 liblinux。liblinux 检查 TaskStruct.vmas：
- **VMA 命中且为延迟分配**：`sys_map_page()` 分配物理页，若是文件映射则先通过 IPC 从 FsServer 读取数据
- **VMA 未命中**：发送 SIGSEGV 给 Linux 程序（修改 save_area 中的上下文，使下次 `sys_linux_syscall_done` 时跳转到信号处理函数或终止进程）

---

## 12. 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| 异常反射的性能开销（每次 Linux syscall 需两次 SVC 往返） | 吞吐量下降 | 热路径 syscall (read/write) 可考虑未来在内核中做 fast-path 缓存；当前阶段可接受 |
| IPC 路径首次就绪的工作量大 | Phase 0 阻塞 | 优先实现最小 IPC 子集（send/recv/call 到 FsServer），暂不要求完整的 seL4 式 capability 检查 |
| liblinux 与 Linux 程序共享地址空间，可能被恶意程序破坏 | 安全性 | 当前阶段关注功能正确性；未来可通过 ARM PAC/Domains 加固 |
| `fork/clone` 的用户态 COW 实现复杂 | Phase 4 停滞 | 考虑用 `vfork` 语义简化（bash 的 fork+exec 天然适合 vfork）；先不实现完全 COW |
| 动态链接器版本兼容性 | 加载失败 | 固定使用 Alpine musl 工具链的特定版本 |
| 页表操作从用户态控制的安全风险 | 内核页表被破坏 | sys_map_page 在内核中严格校验 vaddr 在用户空间范围内且不覆盖 liblinux 保护区 |

---

## 13. 总结

本计划以**用户态库 `os/liblinux`** 的形式构建 Linux syscall 兼容层，核心架构决策包括：

1. **异常反射**：内核通过 SVC 立即数（#0 vs #1）区分 Linux syscall 和原生 syscall，将 Linux syscall 的上下文打包后重定向到 liblinux 的用户态 handler
2. **状态下放**：TaskStruct、fd_table、信号处理等全部 Linux 语义由 liblinux 在用户态维护，内核完全不感知
3. **IPC 强制**：liblinux 从 Phase 1 起通过 IPC 与用户态服务通信，不存在直接函数调用
4. **共享地址空间**：liblinux 与 Linux 程序在同一 AS 中，liblinux 的 loader 通过 `sys_map_page` 等原生 syscall 加载用户程序

**方法A（静态链接程序）** 是 Phase 1-4 的主攻方向，**方法B（动态链接）** 在 Phase 5 启动。两者共用 liblinux 的 syscall 实现，唯一差异在 loader 的入口选择。

**前置硬依赖（Phase 0）**：微内核必须实现 10 个原生 syscall（见 §3.3），并打通 FsServer 的 IPC 事件循环。这是所有后续阶段的基础。
