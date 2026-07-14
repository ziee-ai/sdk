fn main() {
    // libkrun is only linked on macOS. The dylib (+ libkrunfw) is bundled in
    // the app under Contents/Frameworks for release; for dev it comes from
    // Homebrew (`brew install libkrun`) under /opt/homebrew/lib.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
        println!("cargo:rustc-link-lib=dylib=krun");
        // Find the bundled dylib at runtime relative to the executable, then
        // fall back to the Homebrew location for dev runs.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/opt/homebrew/lib");
    }
}
