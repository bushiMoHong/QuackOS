## 用户态 proc（进程管理器 / Process Server）

用户态的 proc 作为独立的用户级进程（Server）运行在 Ring 3，负责系统全局的进程生命周期管理、信号分发和优先级策略决策。

### 职责边界

| 职责 | 说明 |
|------|------|
| **进程生命周期** | 创建（spawn）、退出（exit）、僵尸回收（reap） |
| **信号管理** | 发送/处理 POSIX 风格信号（KILL, TERM, STOP, CONT, CHLD） |
| **优先级策略** | 根据进程角色（system/server/user）决定基础优先级 |
| **权限管理** | 父子关系、谁能创建谁、进程组 |
| **进程表维护** | 全局进程注册表，代际 ProcessId（ABA-proof） |

### 与内核 sche 的关系

```
用户进程
    │
    │ IPC: Spawn / Exit / Signal
    ▼
proc Server ─── 策略决策者（策划者）
    │
    │ 使用 usr/task 创建/销毁线程
    ▼
usr/task ─── 类型安全工具库
    │
    │ syscall (sche::create_thread)
    ▼
内核 sche ─── 机制提供者（执行者）
```

内核只看到线程。proc Server 在"线程"之上构建"进程"概念：
- **进程 = 地址空间 + 线程集合 + 能力集合 + 名字 + 优先级**

### 与其他用户态 Server 的关系

```
proc Server
    │
    ├── 通过 IPC 调用 mm-server（地址空间创建/销毁，VMA 初始化）
    ├── 通过 IPC 调用 cap 系统（权限检查，能力授予）
    └── 使用 usr/task（线程创建/销毁/优先级）
```

### 一次 Spawn 的完整流程

1. **父进程请求**：通过 IPC 向 ProcServer 发送 `ProcRequest::Spawn`
2. **分配 PID**：ProcServer 在进程表中分配新的代际 `ProcessId`
3. **创建地址空间**：ProcServer 通过 IPC 请求 mm-server 创建新地址空间
4. **初始化 VMA**：mm-server 设置代码段、数据段、栈、堆的虚拟内存区域
5. **分配内核栈**：为新进程的初始线程分配内核栈
6. **创建初始线程**：ProcServer 调用 `TaskManager::create_task()` → 内核 `sche::create_thread()`
7. **优先级决策**：ProcServer 根据进程名计算默认优先级
8. **回复父进程**：携带新 ProcessId 的 IPC 回复

### 信号处理模型

```
send_signal(target, SIGTERM)
    │
    ├── 目标状态检查（can_receive_signal?）
    │
    ├── default_action()
    │   ├── Terminate → exit(target, -signal)
    │   ├── Stop      → 设置进程状态为 Stopped
    │   ├── Continue   → 恢复为 Running
    │   └── Ignore     → 丢弃
    │
    └── (未来) 用户自定义 handler
```

### 优先级策略（"策划者"角色）

根据 `sche/README.md` 架构，ProcServer 负责决策"该进程优先级应该是多少"：

| 进程名匹配 | 优先级 | 类别 |
|-----------|--------|------|
| `init`, `mm-*`, `proc-*`, `cap-*` | 200 (SYSTEM) | 系统关键 |
| `fs-*`, `net-*`, `drv-*` | 150 (SERVER) | 系统服务 |
| 其他 | 100 (USER) | 用户进程 |

### ProcessId 设计

```
bits 31:16  →  generation (u16)  ← 每次槽位分配 +1（ABA 保护）
bits 15:0   →  index      (u16)  ← ProcessTable 数组索引
```

- `ProcessId(0)` 为 NULL，永不分配
- 代际计数器在释放槽位时**不清零**——防止悬垂引用
- 与内核 `ThreadId` 设计一致

### 模块结构

```
proc/
├── mod.rs          # 模块入口，公开接口
├── types.rs        # ProcessId, Signal, ProcError, ProcRequest
├── proc_table.rs   # ProcessInfo, ProcessTable（进程表）
├── server.rs       # ProcServer（IPC 事件循环）
└── README.md       # 本文档
```

### 升级路径

- **动态进程表**：当全局分配器就绪后，`ProcessTable` 可从固定大小数组迁移到 `BTreeMap<u16, ProcessInfo>`
- **多 worker**：`ProcServer` 可升级为多 worker 线程池（共享 `RwLock<ProcessTable>`）
- **自定义信号处理**：支持 `sigaction` 式的用户态信号处理器注册
