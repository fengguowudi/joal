use std::{env, fs, path::Path};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = Path::new(&manifest_dir)
        .parent()
        .expect("crates/")
        .parent()
        .expect("workspace root");
    let src = workspace_root.join("resources");

    let out_dir = env::var("OUT_DIR").unwrap();
    let profile_dir = Path::new(&out_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let dst = profile_dir.join("resources");

    copy_dir(&src, &dst).unwrap_or_else(|e| {
        panic!(
            "Failed to copy resources from {} to {}: {e}",
            src.display(),
            dst.display()
        );
    });

    let config_json = dst.join("config.json");
    let config_example = dst.join("config.example.json");
    if !config_json.exists() && config_example.exists() {
        fs::copy(&config_example, &config_json).unwrap_or_else(|e| {
            panic!("Failed to create config.json from example: {e}");
        });
        println!("cargo:warning=Created config.json from config.example.json");
    }

    // JOAL_CLIENT env var overrides the client in config.json at build time
    // Usage: JOAL_CLIENT=qbittorrent-5.2.1.client cargo run
    if let Ok(client) = env::var("JOAL_CLIENT")
        && !client.is_empty()
        && config_json.exists()
    {
        let content = fs::read_to_string(&config_json).unwrap();
        if let Ok(mut json) = content.parse::<serde_json::Value>() {
            json["client"] = serde_json::Value::String(client.clone());
            fs::write(&config_json, serde_json::to_string_pretty(&json).unwrap()).unwrap();
            println!("cargo:warning=JOAL_CLIENT overridden to: {client}");
        }
    }

    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-env-changed=JOAL_CLIENT");
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let name = entry.file_name();
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        if ty.is_dir() {
            copy_dir(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
