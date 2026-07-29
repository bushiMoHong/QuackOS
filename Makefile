# QuackOS top-level Makefile
#
# Targets:
#   make all       Build the kernel and liblinux (delegates to os/)
#   make image     Create disk image and copy bin/ contents into it
#   make run       Build and run in QEMU (delegates to os/)
#   make clean     Remove image and build artifacts
#
# Typical workflow:
#   ./update-bin.sh && make image && make run

IMG     = image.img
SIZE_MB = 128
BIN_DIR = bin

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

.PHONY: all
all:
	$(MAKE) -C os all

# ---------------------------------------------------------------------------
# Disk image
# ---------------------------------------------------------------------------

.PHONY: image
image:
	@echo "==> Creating $(SIZE_MB)MB disk image..."
	dd if=/dev/zero of=$(IMG) bs=1M count=$(SIZE_MB) status=progress
	mkfs.ext4 -F $(IMG)
	@echo "==> Mounting image..."
	@mkdir -p mnt
	sudo mount -o loop $(IMG) mnt
	sudo mkdir -p mnt/bin
	@echo "==> Copying $(BIN_DIR)/* → $(IMG):/bin/"
	sudo cp -a $(BIN_DIR)/* mnt/bin/
	sudo chmod 755 mnt/bin/*
	@echo "==> Unmounting..."
	sudo umount mnt
	@rmdir mnt
	@echo "==> $(IMG) ready ($(SIZE_MB)MB, ext4)"

# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------

.PHONY: run
run:
	$(MAKE) -C os run

# ---------------------------------------------------------------------------
# Mount helpers (for manual inspection)
# ---------------------------------------------------------------------------

.PHONY: mount
mount: $(IMG)
	@mkdir -p mnt
	sudo mount -o loop $(IMG) mnt
	@echo "Mounted at mnt/.  Run 'make umount' when done."

.PHONY: umount
umount:
	@if mountpoint -q mnt 2>/dev/null; then \
		sudo umount mnt && rmdir mnt; \
		echo "Unmounted."; \
	else \
		echo "Not mounted."; \
	fi

# ---------------------------------------------------------------------------
# Clean
# ---------------------------------------------------------------------------

.PHONY: clean
clean:
	@if mountpoint -q mnt 2>/dev/null; then sudo umount mnt; fi
	@-rmdir mnt 2>/dev/null || true
	rm -f $(IMG)
	$(MAKE) -C os clean
