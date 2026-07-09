// config.rs
// ARM64 内存布局：4KB 页，4 级页表，48 位虚拟地址（256 TB 用户空间 + 256 TB 内核空间）

use core::usize;

// ---- 页和地址位数 ----
pub const PAGE_SHIFT: usize = 12;
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT; // 4 KB
pub const VA_BITS: usize = 48;

// ---- 用户空间 (User Space) ----
pub const USER_BASE: usize = 0x0000_0000_0000_0000;
pub const USER_END: usize   = 0x0000_FFFF_FFFF_FFFF;
pub const USER_SIZE: usize  = 256 * 1024 * 1024 * 1024 * 1024; // 256 TB

// ---- 内核逻辑内存映射 (Linear Mapping / Direct Mapping) ----
// 直接映射所有物理内存，virt = phys + PAGE_OFFSET
pub const LINEAR_MAPPING_BASE: usize = 0xFFFF_0000_0000_0000;
pub const LINEAR_MAPPING_END: usize   = 0xFFFF_7FFF_FFFF_FFFF;
pub const LINEAR_MAPPING_SIZE: usize  = 128 * 1024 * 1024 * 1024 * 1024; // 128 TB
// PAGE_OFFSET 通常定义为线性映射的起始地址
pub const PAGE_OFFSET: usize = LINEAR_MAPPING_BASE;

// ---- KASAN Shadow Region (位于线性映射区域内) ----
pub const KASAN_SHADOW_BASE: usize = 0xFFFF_6000_0000_0000;
pub const KASAN_SHADOW_END: usize   = 0xFFFF_7FFF_FFFF_FFFF;
pub const KASAN_SHADOW_SIZE: usize  = 32 * 1024 * 1024 * 1024 * 1024; // 32 TB

// ---- 模块 (Modules) ----
pub const MODULES_BASE: usize = 0xFFFF_8000_0000_0000;
pub const MODULES_END: usize   = 0xFFFF_8000_7FFF_FFFF;
pub const MODULES_SIZE: usize  = 2 * 1024 * 1024 * 1024; // 2 GB

// ---- vmalloc 区域 ----
pub const VMALLOC_BASE: usize = 0xFFFF_8000_8000_0000;
pub const VMALLOC_END: usize   = 0xFFFF_FDFF_BF7F_FFFF;
// 大小约为 126 TB，不直接定义以避免近似误差，可通过结束 - 起始 + 1 计算
pub const VMALLOC_SIZE: usize = VMALLOC_END - VMALLOC_BASE + 1;

// ---- 保护区域 (Guard Region) ----
pub const GUARD_REGION_BASE: usize = 0xFFFF_FDFF_BF80_0000;
pub const GUARD_REGION_END: usize   = 0xFFFF_FDFF_BFFF_FFFF;
pub const GUARD_REGION_SIZE: usize  = 8 * 1024 * 1024; // 8 MB

// ---- vmemmap (struct page 映射) ----
pub const VMEMMAP_BASE: usize = 0xFFFF_FDFF_C000_0000;
pub const VMEMMAP_END: usize   = 0xFFFF_FFFF_BFFF_FFFF;
pub const VMEMMAP_SIZE: usize  = VMEMMAP_END - VMEMMAP_BASE + 1; // 约 2 TB

// ---- PCI I/O 空间 ----
pub const PCI_IO_BASE: usize = 0xFFFF_FFFF_C080_0000;
pub const PCI_IO_END: usize   = 0xFFFF_FFFF_C17F_FFFF;
pub const PCI_IO_SIZE: usize  = 16 * 1024 * 1024; // 16 MB

// ---- 固定映射 (Fixed Mappings) ----
pub const FIXED_MAPPINGS_BASE: usize = 0xFFFF_FFFF_C180_0000;
pub const FIXED_MAPPINGS_END: usize   = 0xFFFF_FFFF_FF7F_FFFF;
pub const FIXED_MAPPINGS_SIZE: usize  = FIXED_MAPPINGS_END - FIXED_MAPPINGS_BASE + 1; // 约 992 MB

// ---- 保护区域 2 (Guard Region) ----
pub const GUARD_REGION2_BASE: usize = 0xFFFF_FFFF_FF80_0000;
pub const GUARD_REGION2_END: usize   = 0xFFFF_FFFF_FFFF_FFFF;
pub const GUARD_REGION2_SIZE: usize  = 8 * 1024 * 1024; // 8 MB