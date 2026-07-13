use xmas_elf::{ElfFile, program::{ProgramHeader, Type}};
use alloc::vec::Vec;

// 引入你的各个子系统类型 (路径根据你的实际导出情况调整)
use crate::usr::fs::server::FsServer;
use crate::usr::mm::server::MmServer;
use crate::usr::proc::proc_table::{ProcessTable, ProcessInfo, MAX_THREADS_PER_PROCESS};
use crate::usr::proc::types::{ProcessId, ProcessPriority, ProcError};
use crate::usr::task::{TaskManager, TaskPriority};
use crate::kernel::bmm::AddressSpaceId;

/// 定义进程默认的地址空间布局常量
const USER_STACK_END: usize   = 0x8000_0000;
const USER_STACK_SIZE: usize  = 0x10000; // 64 KB
const USER_STACK_START: usize = USER_STACK_END - USER_STACK_SIZE;

/// 完整的程序加载和初始化入口
pub fn spawn_process(
    fs: &FsServer,
    mm: &mut MmServer,
    proc_table: &mut ProcessTable,
    task_mgr: &TaskManager,
    parent_pid: ProcessId,
    new_pid: ProcessId,
    asid: AddressSpaceId,
    path: &str,
) -> Result<(), &'static str> {
    // ------------------------------------------------------------------------
    // 1. 文件系统：读取 ELF 字节流
    // ------------------------------------------------------------------------
    // 打开文件并获取 fd
    let fd = fs.open(parent_pid.index() as u32, path, crate::usr::fs::types::OpenFlags::O_RDONLY, 0)
        .map_err(|_| "Failed to open ELF file")?;
    
    // 获取文件大小以便读取 (这里假设你有一个获取大小的方法，或直接读取足够大的块)[cite: 5]
    let stat = fs.fstat(parent_pid.index() as u32, fd).map_err(|_| "Failed to stat file")?;
    let elf_data = fs.read(parent_pid.index() as u32, fd, stat.size as usize)
        .map_err(|_| "Failed to read ELF file")?;
    
    fs.close(parent_pid.index() as u32, fd).ok();

    // ------------------------------------------------------------------------
    // 2. ELF 解析：计算各段边界
    // ------------------------------------------------------------------------
    let elf = ElfFile::new(&elf_data).map_err(|_| "Invalid ELF format")?;
    
    let mut code_start = usize::MAX;
    let mut code_end = 0;
    let mut data_start = usize::MAX;
    let mut data_end = 0;

    for ph in elf.program_iter() {
        if ph.get_type() == Ok(Type::Load) {
            let vaddr = ph.virtual_addr() as usize;
            let mem_size = ph.mem_size() as usize;
            let is_exec = ph.flags().is_execute();

            if is_exec {
                code_start = code_start.min(vaddr);
                code_end = code_end.max(vaddr + mem_size);
            } else {
                data_start = data_start.min(vaddr);
                data_end = data_end.max(vaddr + mem_size);
            }
        }
    }

    // 按页对齐 (PAGE_MASK = 4095)
    let heap_start = (data_end + 0xFFF) & !0xFFF;

    // ------------------------------------------------------------------------
    // 3. 内存管理：初始化 VMA
    // ------------------------------------------------------------------------
    // 在 MmServer 中注册进程，它会自动建立 VmaManager
    mm.register_process(new_pid.into(), asid).map_err(|_| "Failed to register with MM")?;

    // 设置该进程的代码、数据、堆栈区域，自动插入 Guard Pages[cite: 6]
    mm.init_process_vma(
        new_pid.into(),
        if code_start == usize::MAX { 0 } else { code_start },
        code_end,
        if data_start == usize::MAX { 0 } else { data_start },
        data_end,
        USER_STACK_START,
        USER_STACK_END,
        heap_start
    ).map_err(|_| "Failed to initialize VMAs")?;

    // *注意*：微内核下，此时不需要直接把 ELF 数据拷进内存。
    // 程序启动后访问代码段会触发 Page Fault，此时 MmServer 的 handle_page_fault 会被触发。
    // 在那一刻，你再通过 pager 将 elf_data 复制到分配出的物理页中。

    // ------------------------------------------------------------------------
    // 4. 进程与任务管理：创建实体
    // ------------------------------------------------------------------------
    // 创建 ProcInfo[cite: 7]
    let proc_info = ProcessInfo::new(
        new_pid,
        path.as_bytes(),
        ProcessPriority::DEFAULT,
        asid,
        parent_pid
    );
    
    // 插入进程表[cite: 7]
    let allocated_pid = proc_table.insert(proc_info).map_err(|_| "Proc table full")?;

    // 分配内核栈 (需向内核 BMM 申请 2 页连续物理内存，这里用伪变量替代)
    let kstack_base = 0; // TODO: alloc_kstack()
    let kstack_top = 0;  // TODO: kstack_base + 8192

    // 这里的 ttbr0 应当对应 asid 的页表根地址
    let ttbr0 = 0; // TODO: get_page_table_root(asid)

    // 创建线程[cite: 8]
    let tid = task_mgr.create_task(
        TaskPriority(128), // 默认优先级
        kstack_base,
        kstack_top,
        ttbr0,
        asid.0,
        allocated_pid.into()
    ).map_err(|_| "Failed to create task")?;

    // 将线程挂载到进程上[cite: 7]
    let proc_mut = proc_table.get_mut(allocated_pid).unwrap();
    proc_mut.add_thread(tid).map_err(|_| "Thread list full")?;

    // TODO: 配置 TCB (Task Control Block) 中的上下文寄存器
    // 需要设置 ELR_EL1 = elf.header.pt2.entry_point()
    // 需要设置 SP_EL0  = USER_STACK_END
    // 需要设置 SPSR_EL1 = 0 (表示跳转到 EL0)
    
    // 把线程放入就绪队列
    task_mgr.wake_task(tid);

    Ok(())
}
