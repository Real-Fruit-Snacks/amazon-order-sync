// Windows: embed the icon, the manifest (DPI awareness, modern dialogs) and the
// version into the exe. A missing resource compiler only costs the icon, so it
// warns instead of failing the build.
fn main() {
    println!("cargo:rerun-if-changed=tray/app.rc");
    println!("cargo:rerun-if-changed=tray/app.ico");
    println!("cargo:rerun-if-changed=tray/app.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let v = env!("CARGO_PKG_VERSION");
    let parts: Vec<&str> = v.split(['.', '-']).take(3).collect();
    let commas = format!("{},{},{},0", parts[0], parts[1], parts[2]);
    let defs = [format!("AOS_VERSION_COMMAS={}", commas), format!("AOS_VERSION_STR=\\\"{}\\\"", v)];
    if let Err(e) = embed_resource::compile("tray/app.rc", &defs).manifest_optional() {
        println!("cargo:warning=no Windows resources embedded (icon, manifest): {}", e);
    }
}
