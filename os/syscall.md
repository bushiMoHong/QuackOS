| nr  | 名称                         | x0        | x1        | x2        | x3                          | 说明                           |
| --- | ---------------------------- | --------- | --------- | --------- | --------------------------- | ------------------------------ |
| 1   | `sys_map_page`               | vaddr     | prot      | –         | –                           | 映射一页物理内存               |
| 2   | `sys_unmap_page`             | vaddr     | –         | –         | –                           | 取消映射并释放                 |
| 3   | `sys_ipc_send`               | ch        | msg_ptr   | msg_len   | –                           | 发送 IPC 消息                  |
| 4   | `sys_ipc_recv`               | ch        | buf_ptr   | buf_len   | –                           | 接收 IPC 消息                  |
| 5   | `sys_ipc_call`               | ch        | send_ptr  | send_len  | recv_buf, recv_len          | 同步 IPC（send+recv）          |
| 6   | `sys_create_thread`          | entry     | stack_top | arg       | –                           | 创建用户线程                   |
| 7   | `sys_exit_thread`            | exit_code | –         | –         | –                           | 退出当前线程                   |
| 8   | `sys_register_linux_handler` | handler   | save_area | –         | –                           | 注册 Linux syscall 反射入口    |
| 9   | `sys_linux_syscall_done`     | ret_val   | –         | –         | –                           | Linux syscall 完成，恢复上下文 |
| 10  | `sys_yield`                  | –         | –         | –         | –                           | 让出 CPU                       |
| 11  | `sys_console_write`          | buf       | len       | –         | –                           | UART 输出                      |
| 12  | `sys_mprotect`               | vaddr     | prot      | –         | –                           | 修改页权限                     |
| 14  | `sys_clone`                  | flags     | child_sp  | par_tid   | child_tid, tls              | 创建进程（fork）               |
| 15  | `sys_console_read`           | buf       | len       | –         | –                           | UART 输入（非阻塞）            |
| 16  | `sys_exec`                   | elf_ptr   | elf_len   | stack_top | bootinfo                    | 替换地址空间（execve）         |
| 17  | `sys_wait4`                  | –         | –         | –         | –                           | 等待子进程退出                 |
| 18  | `sys_create_notification`    | –         | –         | –         | –                           | 创建通知对象                   |
| 19  | `sys_notify_send`            | nid       | –         | –         | –                           | 发送通知                       |
| 20  | `sys_notify_wait`            | nid       | –         | –         | –                           | 等待通知                       |
| 21  | `sys_irq_register`           | irq_num   | –         | –         | –                           | 注册 IRQ 通知                  |
| 22  | `sys_irq_ack`                | irq_num   | –         | –         | –                           | IRQ EOI 确认                   |
| 23  | `sys_ipc_recv_timeout`       | ch        | buf       | len       | timeout_ms                  | 接收 IPC（带超时）             |
| 24  | `sys_ipc_call_timeout`       | ch        | send_ptr  | send_len  | recv_buf, recv_len, timeout | 同步 IPC（带超时）             |
| 25  | `sys_cspace_mint`            | obj_id    | cap_type  | rights    | –                           | 创建能力                       |
| 26  | `sys_cspace_derive`          | src_cptr  | rights    | –         | –                           | 派生能力                       |
| 27  | `sys_cspace_revoke`          | cptr      | –         | –         | –                           | 撤销能力                       |
| 28  | `sys_cspace_move`            | src       | dest      | –         | –                           | 移动能力                       |
| 29  | `sys_cspace_delete`          | cptr      | –         | –         | –                           | 删除能力                       |
