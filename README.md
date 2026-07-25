# QuackOS

一个微内核实验操作系统，通过 liblinux（用户态 Linux 兼容层）运行linux程序。

## 架构

```
┌──────────────┐
│   bash/echo  │  ← musl 静态链接的 Linux 程序
├──────────────┤
│   liblinux   │  ← 用户态 Linux syscall 兼容层
├──────────────┤
│   微内核     │  ← IPC / 线程调度 / 页表 / 异常处理
├──────────────┤
│   AArch64    │  ← QEMU virt, GICv3, PL011 UART, VirtIO
└──────────────┘
```

## 目录

```
QuackOS/
├── os/                    # 内核 + liblinux
│   ├── arch/aarch64/      # 架构相关（向量表、页表、MMU）
│   ├── common/src/kernel/ # 内核核心（调度、IPC、异常、内存管理）
│   ├── common/src/usr/    # 用户态服务（FS server、进程管理、ELF 加载）
│   └── liblinux/          # Linux syscall 兼容层
├── user/                  # 用户程序
│   ├── helloworld/        # musl hello world
│   └── bash-5.2/          # bash 5.2
└── Makefile               # 构建磁盘镜像
```

## 构建 & 运行

```bash
# 1. 编译内核 + liblinux
cd os && make arm

# 2. 运行（需要 QEMU aarch64）
make run

# 3. 构建磁盘镜像（可选，用于 ext4 根文件系统）
cd .. && make
```

## 当前状态

- **已支持**：静态链接 musl 程序（bash、helloworld、echo）
- **已支持**：fork / exec / wait4 进程模型
- **已支持**：独立地址空间、VirtIO 块设备、ext4 文件系统
- **进行中**：更多 Linux syscall、信号、管道

## 工具链

- Rust (nightly) — 内核 + liblinux
- aarch64-linux-musl-gcc — 用户程序
- QEMU (qemu-system-aarch64) — 模拟器
