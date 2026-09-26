// build.rs — embed the template catalog (templates/<id>/{template.toml,compose.yml})
//
// Generates `templates_embedded.rs` with one include_str! pair per template
// directory, so the catalog ships inside the binaries and needs no files on
// the server. Directories starting with '_' or '.' are skipped.

use std::fs;
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("../../templates");
    println!("cargo:rerun-if-changed={}", root.display());

    let mut dirs: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !path.is_dir() || name.starts_with('_') || name.starts_with('.') {
                continue;
            }
            println!("cargo:rerun-if-changed={}", path.display());
            for file in ["template.toml", "compose.yml"] {
                let f = path.join(file);
                assert!(f.is_file(), "template {name} is missing {file}");
                println!("cargo:rerun-if-changed={}", f.display());
            }
            dirs.push((name, path));
        }
    }
    dirs.sort();

    let mut out = String::from("pub static TEMPLATES: &[(&str, &str, &str)] = &[\n");
    for (name, path) in &dirs {
        let path = path.canonicalize().expect("canonical template path");
        out.push_str(&format!(
            "    ({name:?}, include_str!({:?}), include_str!({:?})),\n",
            path.join("template.toml"),
            path.join("compose.yml"),
        ));
    }
    out.push_str("];\n");
    let dest =
        PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("templates_embedded.rs");
    fs::write(dest, out).expect("write templates_embedded.rs");
}
