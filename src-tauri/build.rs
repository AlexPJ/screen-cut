fn main() {
    // whisper.cpp usa `@available` en su código de Metal, que llama a
    // `__isPlatformVersionAtLeast` del runtime de clang; rustc no lo enlaza.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let dir = std::process::Command::new("clang")
            .arg("--print-resource-dir")
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|dir| !dir.is_empty())
            .expect("no se encontró clang (instala las Command Line Tools de Xcode)");
        println!("cargo:rustc-link-search=native={dir}/lib/darwin");
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
    tauri_build::build()
}
