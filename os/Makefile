# QuackOS Makefile
#
# 架构选择 (arm | rv | la)，通过 ARCH 变量或对应 target:
#   make run ARCH=arm    # AArch64 (默认)
#   make run ARCH=rv     # RISC-V
#   make run ARCH=la     # LoongArch
#   等价地: make arm / make rv / make la (仅编译)
#
# Targets:
#   make arm    Build the AArch64 kernel (release) + liblinux
#   make rv     Build the RISC-V kernel
#   make la     Build the LoongArch kernel
#   make run    Build and run the selected architecture in QEMU
#   make clean  Remove build artifacts

ARCH ?= arm

# ---- liblinux user-space binary (AArch64 only) ----
LIBLINUX_DIR  := liblinux
LIBLINUX_ELF  := $(LIBLINUX_DIR)/target/aarch64-unknown-none/release/liblinux
AARCH64_BIN   := target/aarch64-unknown-none/release/common
IMG_FILE      ?= ../image.img

# ---- Per-architecture configuration ----
ifeq ($(ARCH),arm)
  KERNEL_BIN := $(AARCH64_BIN)
  QEMU       := qemu-system-aarch64
  QEMU_ARGS  := -M virt -cpu cortex-a57 -m 512M -nographic \
                -drive file=$(IMG_FILE),format=raw,if=none,id=drive0 \
                -device virtio-blk-device,drive=drive0,bus=virtio-mmio-bus.0
else ifeq ($(ARCH),rv)
  KERNEL_BIN := arch/riscv64/target/riscv64gc-unknown-none-elf/release/riscv64
  QEMU       := qemu-system-riscv64
  QEMU_ARGS  := -M virt -m 512M -nographic
else ifeq ($(ARCH),la)
  KERNEL_BIN := arch/loongarch64/target/loongarch64-unknown-none/release/loongarch64
  QEMU       := qemu-system-loongarch64
  QEMU_ARGS  := -M virt -m 512M -nographic
else
  $(error Unknown ARCH=$(ARCH): use arm, rv or la)
endif

# ---- Default ----
.PHONY: all
all: $(ARCH)

# ---- Build: AArch64 (kernel + liblinux) ----
.PHONY: arm
arm: $(LIBLINUX_ELF)
	cargo build --target aarch64-unknown-none --release

# liblinux is embedded into the kernel, so always rebuild it.
.PHONY: _build_liblinux
$(LIBLINUX_ELF): _build_liblinux
	cd $(LIBLINUX_DIR) && cargo build --target aarch64-unknown-none --release

# ---- Build: RISC-V (standalone) ----
.PHONY: rv
rv:
	cd arch/riscv64 && cargo build --release

# ---- Build: LoongArch (standalone) ----
.PHONY: la
la:
	cd arch/loongarch64 && cargo build --release

# ---- Run ----
.PHONY: run
run: $(ARCH)
	$(QEMU) $(QEMU_ARGS) -kernel $(KERNEL_BIN)

# ---- Clean ----
.PHONY: clean
clean:
	cd $(LIBLINUX_DIR) && cargo clean
	cargo clean
	cd arch/riscv64 && cargo clean
	cd arch/loongarch64 && cargo clean
