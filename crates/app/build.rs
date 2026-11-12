// Embed the icon and version info in winfx.exe (Explorer, Task Manager, SmartScreen).
fn main() {
    println!("cargo:rerun-if-changed=res/winfx.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("res/winfx.ico")
            .set("FileDescription", "WinFX")
            .set("ProductName", "WinFX")
            .set("LegalCopyright", "MIT License");
        if let Err(e) = res.compile() {
            println!("cargo:warning=couldn't embed Windows resources: {e}");
        }
    }
}
