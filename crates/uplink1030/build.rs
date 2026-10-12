fn main() {
    // libiio-sys 0.3 only adds Homebrew's framework directory on macOS.
    // ADI's native libiio installer puts iio.framework in /Library/Frameworks.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")
        && std::env::var_os("CARGO_FEATURE_PLUTO").is_some()
    {
        println!("cargo:rustc-link-search=framework=/Library/Frameworks");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/Library/Frameworks");
    }
}
