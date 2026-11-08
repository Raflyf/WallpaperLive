fn main() {
    embed_manifest::embed_manifest_file("app.manifest")
        .expect("Windows manifest must embed successfully");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=.env");

    let mut key1 = std::env::var("PIXABAY_KEY_1")
        .or_else(|_| std::env::var("PIXABAY_API_KEY_1"))
        .unwrap_or_default();
    let mut key2 = std::env::var("PIXABAY_KEY_2")
        .or_else(|_| std::env::var("PIXABAY_API_KEY_2"))
        .unwrap_or_default();

    if (key1.is_empty() || key2.is_empty())
        && let Ok(content) = std::fs::read_to_string(".env")
    {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(val) = trimmed
                .strip_prefix("PIXABAY_KEY_1=")
                .or_else(|| trimmed.strip_prefix("PIXABAY_API_KEY_1="))
                && key1.is_empty()
            {
                key1 = val.trim().to_string();
            } else if let Some(val) = trimmed
                .strip_prefix("PIXABAY_KEY_2=")
                .or_else(|| trimmed.strip_prefix("PIXABAY_API_KEY_2="))
                && key2.is_empty()
            {
                key2 = val.trim().to_string();
            }
        }
    }

    println!("cargo:rustc-env=PIXABAY_KEY_1={key1}");
    println!("cargo:rustc-env=PIXABAY_KEY_2={key2}");
}
