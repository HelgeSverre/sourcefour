fn main() {
    println!("cargo:rerun-if-changed=windows.rc");
    println!("cargo:rerun-if-changed=assets/icon/Sourcefour.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("windows.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
