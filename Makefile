# 目标镜像文件名
IMG = image.img
# 镜像大小（128MB）
SIZE_MB = 128
# Bash 编译输出目录（根据你的实际路径调整）
BASH_DIR = bin
HELLO_WORLD_DIR = bin
# 要复制的可执行文件列表
BASH_BINS = bash bashbug bashversion
HELLO_WORLD_BIN = helloworld

# 默认目标：生成并填充镜像
all: $(IMG) populate

# 创建并格式化镜像（空 ext4）
$(IMG):
	@echo "创建 $(SIZE_MB)MB 的空文件..."
	dd if=/dev/zero of=$@ bs=1M count=$(SIZE_MB) status=progress
	@echo "格式化为 ext4..."
	mkfs.ext4 -F $@
	@echo "镜像 $(IMG) 创建完成。"

# 填充镜像：挂载 -> 复制文件 -> 卸载
populate: $(IMG)
	@echo "挂载镜像到 mnt/ ..."
	@mkdir -p mnt
	sudo mount -o loop $< mnt
	@echo "创建 /bin 目录（如果不存在）..."
	sudo mkdir -p mnt/bin
	@echo "复制 Bash 可执行文件到 /bin ..."
	for f in $(BASH_BINS); do \
	sudo cp -a $(BASH_DIR)/$$f mnt/bin/; \
	done
	@echo "复制 helloworld 到 /bin ..."
	sudo cp $(HELLO_WORLD_DIR)/$(HELLO_WORLD_BIN) mnt/bin/
	@echo "设置可执行权限（确认）..."
	sudo chmod 755 mnt/bin/*
	@echo "卸载镜像..."
	sudo umount mnt
	@rmdir mnt
	@echo "镜像填充完成。"

# 手动挂载（用于调试或额外操作）
mount: $(IMG)
	@mkdir -p mnt
	sudo mount -o loop $< mnt
	@echo "已挂载到 mnt/，操作完成后请执行 make umount"

umount:
	@if mountpoint -q mnt; then sudo umount mnt; rmdir mnt; fi

# 清理：卸载并删除镜像文件
clean: umount
	@-rmdir mnt 2>/dev/null || true
	rm -f $(IMG)

.PHONY: all populate mount umount clean