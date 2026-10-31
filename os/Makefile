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
#   make rv     Build the RISC-V kernel + liblinux
#   make la     Build the LoongArch kernel + liblinux
#   make run    Build and run the selected architecture in QEMU
#   make clean  Remove build artifacts

ARCH ?= arm

# ---- liblinux user-space binary ----
LIBLINUX_DIR  := liblinux
IMG_FILE      ?= ../image.img

# ---- Per-architecture configuration ----
ifeq ($(ARCH),arm)
  TARGET_TRIPLE := aarch64-unknown-none
  QEMU          := qemu-system-aarch64
  QEMU_ARGS     := -M virt -cpu cortex-a57 -m 512M -nographic \
                   -drive file=$(IMG_FILE),format=raw,if=none,id=drive0 \
                   -device virtio-blk-device,drive=drive0,bus=virtio-mmio-bus.0
else ifeq ($(ARCH),rv)
  TARGET_TRIPLE := riscv64gc-unknown-none-elf
  QEMU          := qemu-system-riscv64
  QEMU_ARGS     := -M virt -m 512M -nographic
else ifeq ($(ARCH),la)
  TARGET_TRIPLE := loongarch64-unknown-none
  QEMU          := qemu-system-loongarch64
  QEMU_ARGS     := -M virt -m 512M -nographic
else
  $(error Unknown ARCH=$(ARCH): use arm, rv or la)
endif

LIBLINUX_ELF := $(LIBLINUX_DIR)/target/$(TARGET_TRIPLE)/release/liblinux
KERNEL_BIN   := target/$(TARGET_TRIPLE)/release/common

# ---- Default ----
.PHONY: all
all: $(ARCH)

# ---- Build: 编译 liblinux（用户态 ELF） ----
.PHONY: _build_liblinux
_build_liblinux:
	cd $(LIBLINUX_DIR) && cargo build --target $(TARGET_TRIPLE) --release

$(LIBLINUX_ELF): _build_liblinux

# ---- Build: 编译完整内核（common） ----
# 每个 target 通过递归 make 把自身名字作为 ARCH 传入，保证 TARGET_TRIPLE 正确。
.PHONY: arm rv la _build
arm:
	@$(MAKE) --no-print-directory _build ARCH=arm
rv:
	@$(MAKE) --no-print-directory _build ARCH=rv
la:
	@$(MAKE) --no-print-directory _build ARCH=la

_build: $(LIBLINUX_ELF)
	cargo build -p common --target $(TARGET_TRIPLE) --release

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
