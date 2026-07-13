# QuackOS Makefile
#
# Targets:
#   make arm    Build the AArch64 kernel (release) + liblinux
#   make run    Build and run the kernel in QEMU
#   make clean  Remove build artifacts

ARCH       ?= aarch64-unknown-none
TARGET     := target/$(ARCH)/release/common
IMG_FILE   ?= ../image.img

# ---- liblinux user-space binary ----
LIBLINUX_DIR  := liblinux
LIBLINUX_ELF  := $(LIBLINUX_DIR)/target/$(ARCH)/release/liblinux

# ---- QEMU ----
QEMU         ?= qemu-system-aarch64
QEMU_MACHINE ?= virt
QEMU_CPU     ?= cortex-a57
QEMU_RAM     ?= 512M
QEMU_FLAGS   := -M $(QEMU_MACHINE) -cpu $(QEMU_CPU) -m $(QEMU_RAM) -nographic
QEMU_FLAGS += -drive file=$(IMG_FILE),format=raw,if=none,id=drive0
QEMU_FLAGS += -device virtio-blk-device,drive=drive0,bus=virtio-mmio-bus.0

# ---- Default ----
.PHONY: all
all: arm

# ---- Build ----
.PHONY: arm
arm: $(LIBLINUX_ELF)
	cargo build --target $(ARCH) --release

# Build liblinux as a standalone user-space binary
$(LIBLINUX_ELF):
	cd $(LIBLINUX_DIR) && cargo build --target $(ARCH) --release

# ---- Run ----
.PHONY: run
run: arm
	$(QEMU) $(QEMU_FLAGS) -kernel $(TARGET)

# ---- Clean ----
.PHONY: clean
clean:
	cd $(LIBLINUX_DIR) && cargo clean
	cargo clean
