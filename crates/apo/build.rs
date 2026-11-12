// Version info for winfx_apo.dll, which loads inside the Windows audio engine.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set("FileDescription", "WinFX audio effects (APO)")
            .set("ProductName", "WinFX")
            .set("LegalCopyright", "MIT License");
        if let Err(e) = res.compile() {
            println!("cargo:warning=couldn't embed Windows resources: {e}");
        }
    }
}
