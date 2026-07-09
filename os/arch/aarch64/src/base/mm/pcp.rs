// mm/pcp.rs

/// PCP 批量向全局 Buddy 申请/归还的页数
const PCP_BATCH: usize = 16;
const PCP_HIGH: usize = 64;

pub struct PcpCache {
    /// 单页缓存栈
    pages: [usize; PCP_HIGH],
    /// 当前缓存数量
    count: usize,
}

impl PcpCache {
    /// 分配一个单页 (Order 0)
    pub fn alloc_page(&mut self) -> Option<usize> {
        if self.count == 0 {
            // 缓存为空，加锁向全局 Buddy 批量申请 PCP_BATCH 个页填充到 pages 中
            self.refill_from_buddy();
        }

        if self.count > 0 {
            self.count -= 1;
            Some(self.pages[self.count])
        } else {
            None // 物理内存彻底耗尽
        }
    }

    /// 释放一个单页 (Order 0)
    pub fn free_page(&mut self, ppn: usize) {
        if self.count == PCP_HIGH {
            // 缓存已满，将 PCP_BATCH 个页加锁归还给全局 Buddy
            self.drain_to_buddy();
        }
        self.pages[self.count] = ppn;
        self.count += 1;
    }
}

/// 获取当前 CPU 的 PCP Cache (需要关闭中断以防被抢占)
pub fn get_local_pcp() -> &'static mut PcpCache {
    let cpu_id: usize;
    unsafe { core::arch::asm!("mrs {}, tpidr_el1", out(reg) cpu_id); }
    /// TODO
    // 根据 cpu_id 返回对应的 PcpCache 实例
    // ...
}