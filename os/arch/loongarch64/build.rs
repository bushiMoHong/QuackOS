use std::env;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg=-T{}/src/linker_loongarch.ld", manifest_dir);
    println!("cargo:rerun-if-changed=src/linker_loongarch.ld");
}
