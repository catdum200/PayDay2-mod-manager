//! Embeds the app icon and version details into the Windows exe.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "PD2 Mod Manager")
        .set("FileDescription", "PD2 Mod Manager")
        .set("LegalCopyright", "MIT License");
    // Missing Windows SDK tools shouldn't block a build; the exe just has no icon.
    if let Err(e) = res.compile() {
        println!("cargo:warning=couldn't embed the exe icon: {e}");
    }
}
