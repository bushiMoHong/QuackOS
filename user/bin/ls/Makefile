TARGET = ls
TARGET_JSON = aarch64-unknown-quackos.json
TARGET_DIR = aarch64-unknown-quackos

all: $(TARGET)

$(TARGET): src/main.rs Cargo.toml $(TARGET_JSON)
	RUSTFLAGS="-Zunstable-options -C link-arg=-nostartfiles -C link-arg=-nostdlib" cargo build -Z build-std=core,compiler_builtins --target $(TARGET_JSON) --release
	cp target/$(TARGET_DIR)/release/$(TARGET) $(TARGET)
	aarch64-linux-gnu-strip $(TARGET)

clean:
	cargo clean
	rm -f $(TARGET)
