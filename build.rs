fn main() {
    embed_manifest::embed_manifest_file("app.manifest")
        .expect("Windows manifest must embed successfully");
    println!("cargo:rerun-if-changed=app.manifest");
}
