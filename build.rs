// Embeds icon.ico (written by `tackle --write-icon`) as the exe's icon.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && std::path::Path::new("icon.ico").exists() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not embedded: {}", e);
        }
    }
    println!("cargo:rerun-if-changed=icon.ico");
}
