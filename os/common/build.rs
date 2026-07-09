use std::env;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    // Linker script lives in the arch/aarch64 directory.
    println!(
        "cargo:rustc-link-arg=-T{}/../arch/aarch64/linker.ld",
        manifest_dir
    );
    println!("cargo:rerun-if-changed=../arch/aarch64/linker.ld");
}
