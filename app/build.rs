//! Windows: embed Kaseer's icon in kaseer.exe (Explorer, the taskbar, and
//! GPUI's window icon read resource 1). Nothing to do on other targets.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = "../packaging/windows/kaseer.rc";
    println!("cargo:rerun-if-changed={rc}");
    println!("cargo:rerun-if-changed=../packaging/windows/kaseer.ico");
    embed_resource::compile(rc, embed_resource::NONE)
        .manifest_optional()
        .expect("embed the Windows icon");
}
