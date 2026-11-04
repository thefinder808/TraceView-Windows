//! Embeds the EXE's resources: the application manifest, which sets per-monitor
//! DPI, long paths, and the console policy that lets one EXE act as both a CLI
//! and a GUI; the icon; and the version info Explorer shows. Only the
//! `traceview` binary gets them, so integration tests keep a normal console.

fn main() {
    println!("cargo:rerun-if-changed=traceview.manifest");
    println!("cargo:rerun-if-changed=traceview.ico");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    let here = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest folder");
    let mut resources = winresource::WindowsResource::new();
    resources
        .set_manifest_file(&format!(r"{here}\traceview.manifest"))
        .set_icon(&format!(r"{here}\traceview.ico"))
        .set_language(0x0409)
        .set("ProductName", "TraceView")
        .set("FileDescription", "TraceView")
        .set("CompanyName", "Nathaniel Graham")
        .set("LegalCopyright", "Copyright © 2026 Nathaniel Graham")
        .set("OriginalFilename", "traceview.exe")
        .set("InternalName", "traceview");
    resources
        .compile_for("traceview")
        .expect("resources should compile, which needs the Windows SDK's rc.exe");
}
