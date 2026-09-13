# ext4 metadata_csum 修复任务

## 背景

镜像启用了 `metadata_csum` feature flag (`feature_ro_compat & 0x400`)。这要求 ext4 驱动对所有元数据结构计算并维护 crc32c 校验和。当前 QuackOS 的 ext4 驱动在修改元数据时没有同步更新校验和，导致 host 内核挂载时报 "corrupted filesystem" (EBADMSG, code 74)。

## 受影响的元数据类型

### 1. Inode 校验和 ✅ 已修复

- **文件**: `os/common/src/usr/fs/ext4/inode.rs`
- **修复内容**:
  - `write_inode_on_disk()` — 移除 `if false &&`，启用校验和计算
  - `write_inode()` — 新增校验和计算（创建新 inode 时调用）
  - 校验和存储位置：低 16 位→偏移 0x7C(124)，高 16 位→偏移 0x82(130)
  - `Ext4InodeDisk` 结构：`obso_faddr` / `size_hi` 字段顺序修正为内核布局
- **校验和公式**:
  - `i_csum_seed = crc32c(crc32c(s_checksum_seed, inum_le), generation_le)`
  - `csum = crc32c(i_csum_seed, raw_inode)`（其中 0x7C 和 0x82 两处校验和字段清零）

### 2. Inode 位图校验和 ✅ 已修复

- **位置**: `Ext4GroupDescDisk.inode_bitmap_csum_lo` / `inode_bitmap_csum_hi`
- **公式**: `crc32c(seed, bitmap[0..inodes_per_group/8])`（**不含组号**）

### 3. Block 位图校验和 ✅ 已修复

- **位置**: `Ext4GroupDescDisk.block_bitmap_csum_lo` / `block_bitmap_csum_hi`
- **公式**: `crc32c(seed, bitmap[0..blocks_per_group/8])`（**不含组号**）

### 4. 组描述符自身校验和 ✅ 已修复

- **位置**: `Ext4GroupDescDisk.checksum`
- **公式**: `crc32c(seed, grp_num_le) 逐段累加 descriptor 字节，取低 16 位`
- **附带修复**: `itable_unused` 计数器在 alloc/dealloc inode 时正确增减并写回磁盘

### 5. 超级块校验和 ✅ 已修复

- **位置**: `Ext4SuperBlockDisk.checksum` (offset 0x3FC)
- **公式**: `crc32c(~0, sb_data[0..0x3FC])`

### 6. 目录块尾校验和 (dirent tail) ✅ 已修复

- **位置**: 目录数据块末尾 12 字节的 `ext4_dir_entry_tail.det_checksum`
- **公式**: `crc32c(i_csum_seed, dirent[0..tail_offset])`，`tail_offset = block_size - 12`
- **修复**: 新增 `set_dirblock_csum()`，在 `add_entry`/`set_entry`/`delete_entry` 写回前调用

### 7. 其他附带修复 ✅

- **文件类型常量** (`dentry.rs`): `EXT4_DT_*` 之前误用了 mode 位 (`0x8`=REG 等)，改为正确的 dirent 类型 (`0x1`=REG, `0x2`=DIR, `0x7`=LNK)
- **i_blocks** (`inode.rs`): `set_size` 之前用 `ceil(size/512)`，改为 `ceil(size/fs_block_size) * (fs_block_size/512)`
- **extra_isize** (`inode.rs`): 新建 inode 时 `extra_isize` 之前为 0，改为 32（使 `i_checksum_hi` 参与校验和）

### 8. 遗留问题（未处理）

- extent tree 内部节点校验和（当前测试用例小文件不涉及）
- 新 inode 的 `generation` 字段恒为 0（内核用随机值，NFS 场景才需要）
- `mkdir` 未初始化 `.` / `..` 目录项（TODO）

## 代码位置映射

| 文件 | 相关函数/结构 |
|---|---|
| `os/common/src/usr/fs/ext4/inode.rs` | `write_inode()`, `write_inode_on_disk()`, `crc32c()`, `Ext4InodeDisk` |
| `os/common/src/usr/fs/ext4/block_group.rs` | `GroupDesc::alloc_inode()`, `alloc_one_block()`, `alloc_block()`, `dealloc_*()`, `Ext4GroupDescDisk` |
| `os/common/src/usr/fs/ext4/fs.rs` | `Ext4FileSystem::alloc_inode()`, `alloc_one_block()`, `alloc_block()`, `dealloc_*()` |
| `os/common/src/usr/fs/ext4/super_block.rs` | `Ext4SuperBlock`, `Ext4SuperBlockDisk` |

## 验证方法

```bash
# 在 host 上查看内核具体报哪部分校验失败
sudo dmesg | grep -i ext4 | tail -20
sudo fsck.ext4 -fn image.img
```

## 修复优先级建议

1. **组描述符写回** — 将内存中的 `GroupDescInner` 变更写回到磁盘上的 `Ext4GroupDescDisk`，同时更新其 `checksum` (crc16) 和位图校验和
2. **超级块写回** — 将 `SuperBlockInner` 的 `free_*_count` 写回磁盘并更新超级块 `checksum`
3. **验证 inode 校验和** — 确认当前的 inode 校验和修复是否正确
