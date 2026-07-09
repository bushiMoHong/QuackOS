use crate::config::{PAGE_SHIFT, PAGE_SIZE};

/// 物理页元数据 (类似于 Linux 的 struct page)
#[repr(C)]
pub struct PageMeta {
    pub flags: u32, // 标记位：是否空闲、是否是伙伴的头部等
    pub order: u32, // 当前页块的阶数 (0 ~ MAX_ORDER)
    // 侵入式双向链表节点，用于挂载到 Buddy 的 free_area 中
    pub prev: *mut PageMeta,
    pub next: *mut PageMeta,
}

pub const MAX_ORDER: usize = 10; // 最大支持 2^10 = 1024 页 (4MB) 连续分配

/// 伙伴系统全局状态
pub struct BuddyAllocator {
    // MAX_ORDER + 1 个双向链表头
    free_area: [*mut PageMeta; MAX_ORDER + 1],
}

impl BuddyAllocator {
    /// 计算伙伴的物理页号 (异或操作)
    /// buddy_ppn = ppn ^ (1 << order)
    #[inline]
    fn buddy_of(ppn: usize, order: u32) -> usize {
        ppn ^ (1 << order)
    }

    /// 分配 2^order 个连续物理页
    pub fn alloc_pages(&mut self, target_order: usize) -> Option<usize> {
        // 1. 从 target_order 向上查找第一个非空的链表
        for current_order in target_order..=MAX_ORDER {
            let head = self.free_area[current_order];
            if !head.is_null() {
                // 2. 摘取节点
                // 3. 如果 current_order > target_order，则需要向下分裂 (Split)
                //    将多余的块放入对应阶数的 free_area 中
                // 4. 返回物理页号
                return Some(/* 分配的 ppn */);
            }
        }
        None
    }

    /// 释放并尝试合并伙伴
    pub fn free_pages(&mut self, ppn: usize, mut order: usize) {
        // 1. 不断查找 buddy_ppn = ppn ^ (1 << order)
        // 2. 检查伙伴是否空闲且 order 相同
        // 3. 如果是，将其从 free_area 中摘除，合并成 (order + 1) 的大块，继续向上循环
        // 4. 如果不能合并，将当前块挂入 free_area[order] 的链表中
    }
}