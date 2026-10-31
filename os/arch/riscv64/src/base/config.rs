// config.rs
// RISC-V Sv39 内存布局：4KB 页，3 级页表，39 位虚拟地址
// （256 GB 用户空间 + 256 GB 内核空间）

use core::usize;

// ---- 页和地址位数 ----
pub const PAGE_SHIFT: usize = 12;
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT; // 4 KB
pub const VA_BITS: usize = 39; // Sv39

// ---- 用户空间 (User Space) ----
pub const USER_BASE: usize = 0x0000_0000_0000_0000;
pub const USER_END: usize = 0x0000_003F_FFFF_FFFF; // 256 GB
pub const USER_SIZE: usize = 256 * 1024 * 1024 * 1024; // 256 GB

// ---- 内核空间 (Kernel Space) ----
// Sv39 高半区：从 0xFFFF_FFC0_0000_0000 到 0xFFFF_FFFF_FFFF_FFFF
pub const KERNEL_BASE: usize = 0xFFFF_FFC0_0000_0000;

// ---- 内核线性映射 (Linear Mapping / Direct Mapping) ----
// 直接映射所有物理内存，virt = phys + PAGE_OFFSET。
// 本实现采用恒等映射（virt == phys），与 aarch64 保持一致。
pub const LINEAR_MAPPING_BASE: usize = 0xFFFF_FFC0_0000_0000;
pub const LINEAR_MAPPING_END: usize = 0xFFFF_FFFF_FFFF_FFFF;
pub const LINEAR_MAPPING_SIZE: usize = 256 * 1024 * 1024 * 1024; // 256 GB
// PAGE_OFFSET 通常定义为线性映射的起始地址
pub const PAGE_OFFSET: usize = LINEAR_MAPPING_BASE;
