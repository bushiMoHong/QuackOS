# QuackOS top-level Makefile
#
# Targets:
#   make all       Build the kernel and liblinux (delegates to os/)
#   make image     Create disk image and copy bin/ contents into it
#   make run       Build and run in QEMU (delegates to os/)
#   make clean     Remove image and build artifacts
#
# 架构选择 (arm | rv | la):
#   make run ARCH=rv
#   make run ARCH=la
#   make arm / make rv / make la (仅编译)
#
# Typical workflow:
#   ./update-bin.sh && make image && make run

ARCH ?= arm

IMG     = image.img
SIZE_MB = 128
BIN_DIR = bin

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

.PHONY: all
all:
	$(MAKE) -C os all ARCH=$(ARCH)

.PHONY: arm rv la
arm rv la:
	$(MAKE) -C os $@ ARCH=$@

# ---------------------------------------------------------------------------
# Disk image
# ---------------------------------------------------------------------------

ARCH_BIN_DIR = $(BIN_DIR)/$(ARCH)/bin

.PHONY: image
image:
	@if [ ! -d "$(ARCH_BIN_DIR)" ]; then \
		echo "ERROR: $(ARCH_BIN_DIR) not found (ARCH=$(ARCH))"; \
		echo "       Available: $$(ls -1 $(BIN_DIR) 2>/dev/null || echo none)"; \
		exit 1; \
	fi
	@echo "==> Creating $(SIZE_MB)MB disk image for ARCH=$(ARCH)..."
	dd if=/dev/zero of=$(IMG) bs=1M count=$(SIZE_MB) status=progress
	mkfs.ext4 -F $(IMG)
	@echo "==> Mounting image..."
	@mkdir -p mnt
	sudo mount -o loop $(IMG) mnt
	sudo mkdir -p mnt/bin
	sudo mkdir -p mnt/tmp
	@echo "==> Copying $(ARCH_BIN_DIR)/* → $(IMG):/bin/"
	sudo cp -a $(ARCH_BIN_DIR)/* mnt/bin/
	sudo find mnt/bin -type f -exec chmod 755 {} \;
	@echo "==> Unmounting..."
	sudo umount mnt
	@rmdir mnt
	@echo "==> $(IMG) ready ($(SIZE_MB)MB, ext4, ARCH=$(ARCH))"

# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------

.PHONY: run
run:
	$(MAKE) -C os run ARCH=$(ARCH)

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
