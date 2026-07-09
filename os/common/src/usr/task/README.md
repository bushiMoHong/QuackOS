## 用户态 task（任务/线程管理工具库）

用户态的 task 模块是对内核线程（`sche::Thread`）的类型安全封装，作为工具库供其他用户态组件使用。

### 职责边界

| 职责 | 说明 |
|------|------|
| **类型安全封装** | 用 `TaskId`、`TaskPriority`、`TaskState` 包装内核原始类型 |
| **错误转换** | 将内核 `ScheError` 映射为用户友好的 `TaskError` |
| **快照查询** | `TaskInfo` 提供任务状态的一致性快照 |
| **生命周期操作** | 创建、销毁、优先级管理 |
| **调度控制** | yield、block、wake 的薄封装 |

### 与内核 sche 的关系

```
用户进程
    │
    │ TaskManager::create_task(prio, stack, ttbr0, asid)
    ▼
usr/task ─── 类型安全封装层
    │
    │ sche::create_thread(…)
    ▼
内核 sche ─── 机制提供者（ThreadTable, ReadyQueue, __switch）
```

- `task` **不维护任何自身状态**——所有状态由内核 `sche::ThreadTable` 管理
- `TaskManager` 是零大小结构体，所有方法都是对内核 `sche` 的直接委托
- 错误类型从内核 `ScheError` 映射为用户态 `TaskError`

### 模块结构

```
task/
├── mod.rs          # 模块入口，公开接口 re-export
├── types.rs        # TaskId, TaskPriority, TaskState, TaskInfo, TaskError
├── manager.rs      # TaskManager 结构体
└── README.md       # 本文档
```

### 优先级规范

| 范围   | 类别       | 说明                         |
|--------|-----------|------------------------------|
| 200–255 | System   | 内核及关键系统服务            |
| 128–199 | Server   | 用户态服务进程（mm, proc, fs）|
| 64–127  | User     | 普通用户进程                 |
| 0–63    | Background | 后台/空闲任务               |

### 与其他模块的关系

```
usr/proc (进程管理器)
    │
    │ 使用 usr/task 创建/管理线程
    ▼
usr/task (任务工具库)
    │
    │ 委托内核 sche
    ▼
kernel::sche (调度器)
```

一般用户进程应通过 `usr/proc` 间接使用 `usr/task`，而非直接调用——进程管理器是策略权威。
