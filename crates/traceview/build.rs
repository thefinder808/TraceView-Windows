//! Embeds the application manifest, which sets per-monitor DPI, long paths, and
//! the console policy that lets one EXE act as both a CLI and a GUI.

fn main() {
    println!("cargo:rerun-if-changed=traceview.manifest");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest::embed_manifest_file("traceview.manifest").expect("manifest should embed");
    }
}
