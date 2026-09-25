// Embed EarthDeskIME.manifest (UTF-8 code page) into the Windows build.
fn main() {
    println!("cargo:rerun-if-changed=EarthDeskIME.manifest");
    println!("cargo:rerun-if-changed=EarthDeskIME.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("EarthDeskIME.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
}
