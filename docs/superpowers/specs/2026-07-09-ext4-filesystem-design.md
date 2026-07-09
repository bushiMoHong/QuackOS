# QuackOS ext4 文件系统支持 — 设计文档

## 概述

为 QuackOS 微内核添加完整的 VFS（虚拟文件系统）层和 ext4 文件系统支持，目标平台 aarch64。

参考实现：`example/ext4/`（完整 ext4 参考代码，约 300KB）

## 架构

```
用户态 (Ring 3)
┌──────────────────────────────────────────────────┐
│  FsServer (IPC 事件循环 + 工作线程池)              │
│  ├─ 文件描述符表 (per-process)                      │
│  ├─ Dentry Cache (全局目录项缓存)                   │
│  ├─ Inode Cache (全局 inode 缓存)                   │
│  └─ Mount Table (挂载点管理)                        │
│                                                    │
│  VFS 层                                           │
│  ├─ InodeOp trait  ←── Ext4Inode impl              │
│  ├─ File / Dentry / Kstat                          │
│  └─ PageCache (AddressSpace)                       │
│                                                    │
│  Block 层                                          │
│  ├─ BlockDevice trait  ←── VirtioBlock impl        │
│  └─ BlockCache (块缓存)                             │
│                                                    │
│  Device 层                                         │
│  ├─ /dev/null, /dev/tty, /dev/urandom              │
│  └─ LoopDevice                                     │
└──────────────────────────────────────────────────┘
         │ IPC (open/read/write/stat/...)
         ▼
内核 (Ring 0)
┌──────────────────────────────────────────────────┐
│  IPC Channel │ bmm │ cap │ sche │ trap            │
└──────────────────────────────────────────────────┘
```

## FsServer 并发模型

单线程事件循环 + 工作线程池：

- **主线程**：IPC 收发 + VFS 元数据操作（路径解析、Dentry 查找、权限检查），无阻塞
- **工作线程池**（初期 4 线程）：处理块设备 I/O（PageCache 未命中时），完成后通知主线程
- 发起 I/O 的请求进程在 IPC call 上阻塞等待结果，其他进程不受影响

## IPC 数据传递策略

| 场景 | 策略 |
|------|------|
| 小数据 read/write (< 4KB) | IPC message payload 直接携带 |
| 大数据 read/write (≥ 4KB) | IPC 消息携带 (page_frame_number, count)，通过共享页传递 |
| mmap 文件映射 | IPC 返回 extent 物理块号，mm 服务直接映射零拷贝 |
| PageCache 操作 | 页归 FsServer 管理，数据通过共享页让用户端访问 |

## Dentry 锁策略

- **第一阶段**：一把全局 `RwLock` 保护 dentry 树，路径解析读锁，创建/删除写锁
- **后续优化**：逐节点细粒度锁，类似 Linux path_walk RCU 模式

## VFS 核心接口

```rust
pub trait InodeOp: Send + Sync {
    fn read(&self, offset: usize, buf: &mut [u8]) -> usize;
    fn write(&self, offset: usize, buf: &[u8]) -> usize;
    fn lookup(&self, name: &str, parent: Arc<Dentry>) -> Arc<Dentry>;
    fn create(&self, dentry: Arc<Dentry>, mode: u16);
    fn truncate(&self, size: usize) -> SyscallRet;
    fn get_page(&self, page_index: usize) -> Option<Arc<Page>>;
    fn get_pages(&self, page_index: usize, count: usize) -> Vec<Arc<Page>>;
    fn get_stat(&self) -> Kstat;
    fn as_any(&self) -> &dyn Any;
}
```

## 模块清单

| 模块 | 说明 | 来源 |
|------|------|------|
| `fs/types.rs` | Kstat, OpenFlags, FileDesc, Errno | 新写 |
| `fs/inode.rs` | InodeOp trait, InodeCache | 新写 |
| `fs/dentry.rs` | Dentry, DentryCache | 新写 |
| `fs/file.rs` | File, 文件描述符表 | 新写 |
| `fs/page_cache.rs` | Page, AddressSpace 页缓存 | 新写 |
| `fs/dev/block_dev.rs` | BlockDevice trait, BlockCache | 新写 + 参考 example |
| `fs/dev/null.rs` | /dev/null | 新写 |
| `fs/dev/tty.rs` | /dev/tty | 新写 |
| `fs/dev/urandom.rs` | /dev/urandom | 新写 |
| `fs/dev/loop_device.rs` | 回环设备 | 新写 |
| `fs/dev/rtc.rs` | 实时时钟设备 | 新写 |
| `fs/ext4/mod.rs` | InodeOp impl for Ext4Inode | 从 example 移植适配 |
| `fs/ext4/super_block.rs` | 超级块解析 | 从 example 移植 |
| `fs/ext4/block_group.rs` | 块组描述符/bitmap | 从 example 移植 |
| `fs/ext4/inode.rs` | Ext4InodeDisk + Ext4Inode | 从 example 适配 |
| `fs/ext4/dentry.rs` | Ext4DirEntry | 从 example 移植 |
| `fs/ext4/extent_tree.rs` | Extent 树结构 | 从 example 移植 |
| `fs/ext4/block_op.rs` | 目录内容/块操作 | 从 example 移植 |
| `fs/ext4/fs.rs` | Ext4FileSystem | 从 example 适配 |
| `fs/server.rs` | FsServer IPC 事件循环 + 线程池 | 新写 |

## ext4 适配要点

- 6 个文件直接移植（super_block, block_group, extent_tree, dentry, block_op）：几乎不改逻辑
- 2 个文件重点适配（fs.rs, inode.rs）：替换 crate path、BlockDevice trait、AddressSpace
- 1 个文件重写（mod.rs）：保留 InodeOp impl，设备 inode 创建逻辑移入 dev/
- 移除 `la2000` feature flag 和 `inode_la2000.rs`（aarch64 不需要）
- 新增依赖：`hashbrown = "0.14"`

## 实施顺序

1. `types.rs` — Kstat, OpenFlags, Errno, SyscallRet
2. `inode.rs` + `dentry.rs` — VFS 核心 trait + 目录缓存
3. `page_cache.rs` + `dev/block_dev.rs` — 页缓存 + 块设备抽象
4. `ext4/*` — 7 个 ext4 文件移植
5. `file.rs` + `dev/*` — 文件描述符 + 设备文件
6. `server.rs` — FsServer IPC 事件循环 + 工作线程池
7. `usr/mod.rs` — 注册 fs 模块，init 启动

每步完成后编译验证。
