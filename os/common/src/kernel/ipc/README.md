内核的 src/ipc/ 目录下，为了保持 Rust 代码的模块化、高内聚和清晰的所有权管理，拆分为以下几个核心文件。它们各自负责不同的功能：
1. mod.rs —— IPC 模块的总入口

   负责功能： 统一对外暴露 IPC 的接口（如初始化函数、Syscall 的高层对接函数）。

   核心逻辑： * 接收来自 syscall.rs 转发过来的调用。

        负责根据进程/线程的 ID 或控制块，找到发送方和接收方的结构体对象。

2. channel.rs —— 通道与连接管理

   负责功能： 抽象进程之间的通信链路（Channel）。在微内核（如 seL4 或 Fuchsia）中，两个进程不能凭空通信，必须通过一个内核对象（Channel/Endpoint）相连。

   核心结构体：
   Rust

   pub struct Channel {
   // 正在等待读取该通道消息的线程队列（阻塞队列）
   pub receiver_queue: VecDeque<ThreadIdentifier>,
   // 正在等待发送消息的线程队列（在同步 IPC 中使用）
   pub sender_queue: VecDeque<ThreadIdentifier>,
   // 权限/能力控制指针
   pub capability_id: u32,
   }

   核心逻辑： 负责 Channel 的创建、销毁、以及维护连接两端的线程等待队列（Wait Queue）。

3. message.rs —— 消息数据结构与序列化

   负责功能： 定义在 IPC 过程中，数据在内核态是如何组装和表达的。

   核心结构体：
   Rust

   // 定义微内核消息的类型
   pub enum MessageType {
   ShortInfo,      // 短消息（直接通过 CPU 寄存器传递，速度极快）
   MemoryMap,      // 内存映射（传递大块数据，进行页表共享）
   GrantCapability,// 权限转让（把某个硬件访问权借给另一个进程）
   }

   pub struct MessageHeader {
   pub sender: ProcessId,
   pub msg_type: MessageType,
   pub length: u32,
   }

4. transfer.rs —— 核心数据拷贝与零拷贝引擎

   负责功能： IPC 性能的核心损耗点。 负责将数据真正从进程 A 搬运到进程 B。

   核心逻辑：

        寄存器快速路径（Fast Path）： 如果是少量的控制指令（如几个字节），直接把进程 A 的寄存器值（如 rdi, rsi）复制到进程 B 的 TrapFrame 寄存器里。执行完直接切线程，实现 0 次内存拷贝。

        共享内存与重映射路径： 如果是大数据（如文件系统读了 4KB 磁盘数据要给应用），transfer.rs 负责修改进程 B 的页表，把进程 A 存放数据的物理页框（Page Frame）直接映射到进程 B 的虚拟地址空间里，避免昂贵的 memcpy。

5. synchronization.rs (或 state.rs) —— 线程状态同步器

   负责功能： 控制发送方和接收方的阻塞与唤醒状态。

   核心逻辑：

        同步 IPC（Synchronous）： 发送方发起 ipc_send 后，如果接收方还没准备好，synchronization.rs 必须将发送方线程的状态改为 BlockedOnIPC（阻塞），并调用内核调度器切换到其他线程。

        异步 IPC（Asynchronous / Notification）： 允许发送方丢下消息就走。此文件需要管理内核中的临时消息缓冲区（Mailbox），并在接收方上线时唤醒它。

6. capability.rs (可选，但在安全微内核中必备) —— 权限校验

   负责功能： 校验进程 A 是否有权向进程 B 发送消息。

   核心逻辑： 借助 Rust 的所有权概念，检查进程 A 的控制块中是否持有指向目标 Channel 的 Capability（凭证）。如果没有，直接拒绝 IPC 请求，防止恶意程序通过 IPC 探测或攻击其他系统服务。