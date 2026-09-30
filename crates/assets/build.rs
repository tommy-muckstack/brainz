fn main() {
    // Rust's include_bytes tracking misses newly added files in embedded directories.
    println!("cargo:rerun-if-changed=../../assets");
}
