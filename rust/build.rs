fn main() {
    // Link with libandroid on Android for ASharedMemory support
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "android" {
        println!("cargo:rustc-link-lib=android");
    }
}
