# QuackOS Makefile
#
# Targets:
#   make arm    Build the AArch64 kernel (release)
#   make run    Build and run the kernel in QEMU
#   make clean  Remove build artifacts

ARCH       ?= aarch64-unknown-none
TARGET     := target/$(ARCH)/release/common

# ---- QEMU ----
QEMU         ?= qemu-system-aarch64
QEMU_MACHINE ?= virt
QEMU_CPU     ?= cortex-a57
QEMU_RAM     ?= 512M
QEMU_FLAGS   := -M $(QEMU_MACHINE) -cpu $(QEMU_CPU) -m $(QEMU_RAM) -nographic

# ---- Default ----
.PHONY: all
all: arm

# ---- Build ----
.PHONY: arm
arm:
	cargo build --target $(ARCH) --release

# ---- Run ----
.PHONY: run
run: arm
	$(QEMU) $(QEMU_FLAGS) -kernel $(TARGET)

# ---- Clean ----
.PHONY: clean
clean:
	cargo clean
