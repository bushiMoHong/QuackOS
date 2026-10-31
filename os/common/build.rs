use std::env;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = env::var("TARGET").unwrap_or_default();

    // 根据目标架构选择对应的链接脚本。
    if target.contains("aarch64") {
        println!(
            "cargo:rustc-link-arg=-T{}/../arch/aarch64/linker.ld",
            manifest_dir
        );
        println!("cargo:rerun-if-changed=../arch/aarch64/linker.ld");
    } else if target.contains("riscv64") {
        println!(
            "cargo:rustc-link-arg=-T{}/../arch/riscv64/src/linker.ld",
            manifest_dir
        );
        println!("cargo:rerun-if-changed=../arch/riscv64/src/linker.ld");
    } else if target.contains("loongarch64") {
        println!(
            "cargo:rustc-link-arg=-T{}/../arch/loongarch64/src/linker_loongarch.ld",
            manifest_dir
        );
        println!("cargo:rerun-if-changed=../arch/loongarch64/src/linker_loongarch.ld");
    }
}
