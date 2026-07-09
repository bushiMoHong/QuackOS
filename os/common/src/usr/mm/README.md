## 用户态 mm（策略决策者）

用户态的 mm 作为独立的用户级进程（Server）运行在 Ring 3，负责系统全局的内存分配逻辑、记账和高级内存特性。

### 职责边界

| 职责 | 说明 |
|------|------|
| **虚拟内存区域管理 (VMA)** | 记录每个用户进程的虚拟地址空间布局（代码段、数据段、堆、栈、mmap 区域） |
| **物理内存分配策略** | 维护空闲物理内存链表（伙伴系统 + per-CPU 页面缓存） |
| **缺页异常处理** | 接收内核 BMM 发来的 IPC 缺页消息，根据 VMA 判断合法性并分配物理页 |
| **页面置换与 Swap** | 决定不常用页何时换出/换入（预留钩子） |
| **内存共享** | 管理多进程间的共享内存权限 |
| **CoW（写时复制）** | 处理 CoW 页的写缺页，分配新页并拷贝内容 |

### 与内核 BMM 的交互协议

```
  用户进程缺页
       │
       │ (MMU 硬件异常)
       ▼
  内核 BMM ─── IpcPageFault ──→ 用户态 mm
                                     │
                          ┌──────────┼──────────┐
                          │ 查 VMA  │ 分配物理页 │ 权限检查
                          └──────────┼──────────┘
                                     │
  内核 BMM ←── MapRequest / KillProcess ──┘
       │
       │ (修改页表)
       ▼
  用户进程恢复执行
```

### 模块结构

```
mm/
├── mod.rs          # 模块入口，公开接口
├── types.rs        # 核心类型：VmaEntry, VmPerms, MmError, MmRequest
├── vma.rs          # VmaManager：每进程虚拟地址空间管理
├── allocator.rs    # BuddyAllocator + PcpCache：物理页分配
├── page_fault.rs   # 缺页解析：VMA 查找 → 分配 → 构建映射请求
├── server.rs       # MmServer：进程表、mmap/munmap、缺页事件循环
└── README.md       # 本文档
```

### 一次缺页异常的完整处理流程

1. **触发异常**：用户进程 A 访问未映射的虚拟地址，MMU 触发硬件缺页
2. **内核 BMM 介入**：捕获异常，读取 fault_vaddr 和 cause，暂停进程 A
3. **IPC 向上**：BMM 生成 `IpcPageFault { addr_space_id, fault_vaddr, cause }`，通过 IPC channel 发送给 mm 服务器
4. **mm 决策**：
   - `VmaManager::find(vaddr)` → 找到对应 VMA
   - 权限检查 → `entry.permits(needed_perms)`
   - Guard page？→ SIGSEGV
   - CoW？→ 分配新页，拷贝内容
   - `BuddyAllocator::alloc_page()` → 获取物理页
5. **IPC 向下**：mm 构建 `MapRequest { addr_space_id, vaddr, paddr, flags }`，通过 IPC 发回内核
6. **内核 BMM 执行**：调用 `bmm::map()` 修改页表，刷新 TLB，唤醒进程 A
7. **恢复执行**：进程 A 继续执行

### 设计决策

#### VMA 数据结构

当前使用固定大小数组（`MAX_VMA_ENTRIES = 64`），按 `start_vaddr` 排序，二分查找 O(log N)。

**升级路径**：当全局分配器就绪后，迁移到 `BTreeMap<usize, VmaEntry>`（key = start_vaddr），所有操作 O(log N)，无固定上限。

#### 物理页分配

```
alloc_page()
    │
    ▼
PcpCache (per-CPU, 无锁) ─── 缓存命中，直接返回
    │ 缓存未命中
    ▼
BuddyAllocator (全局, spinlock) ─── 伙伴系统分裂/合并
    │ OOM
    ▼
try_reclaim() ─── LRU 扫描 / swap (预留)
    │ 回收失败
    ▼
OOM Kill ─── 发送 KillProcess IPC
```

#### SMP 多核

- VMA 查询使用读写锁（缺页处理是 reader，可以真正并行）
- 物理页分配通过 per-CPU `PcpCache` 减少锁竞争
- 当 SMP 就绪后，`MmServer` 可升级为多 worker 线程池

#### 预取 (Prefault)

当 `prefault_enabled = true` 时，每次缺页额外映射相邻 4 页（同 VMA 内），减少 IPC 往返次数。
