//! Generate a compile-time static asset table for the fully self-contained web binary.
//! Headless and default Docker builds do not require Node, pnpm or web assets.

use std::{
    env, fs,
    fmt::Write as _,
    path::{Path, PathBuf},
};

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_EMBEDDED_WEB");
    if env::var_os("CARGO_FEATURE_EMBEDDED_WEB").is_none() {
        return;
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("crate directory"))
        .join("../../web/dist");
    assert!(
        root.join("index.html").is_file(),
        "embedded-web requires web/dist; build the frontend with pnpm build first"
    );
    println!("cargo:rerun-if-changed={}", root.display());
    let mut paths = Vec::new();
    collect_files(&root, &root, &mut paths);
    paths.sort();

    let mut generated = String::from(
        "pub(super) fn embedded_asset(path: &str) -> Option<(&'static [u8], &'static str)> {\n    match path {\n",
    );
    for path in paths {
        let relative = path.strip_prefix(&root).expect("asset must be inside dist");
        let route = format!("/{}", relative.to_string_lossy().replace('\\', "/"));
        let absolute = fs::canonicalize(&path).expect("asset must be readable");
        let mime = match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
            "html" => "text/html; charset=utf-8",
            "js" => "text/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "json" => "application/json; charset=utf-8",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "ico" => "image/x-icon",
            "woff2" => "font/woff2",
            "woff" => "font/woff",
            "txt" => "text/plain; charset=utf-8",
            _ => "application/octet-stream",
        };
        writeln!(
            &mut generated,
            "        {route:?} => Some((&include_bytes!({path:?})[..], {mime:?})),",
            path = absolute.to_string_lossy(),
        )
        .expect("write asset descriptor");
    }
    generated.push_str("        _ => None,\n    }\n}\n");
    let dest = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("embedded_assets.rs");
    fs::write(dest, generated).expect("write static asset table");
}

fn collect_files(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read web/dist") {
        let entry = entry.expect("read web asset");
        let kind = entry.file_type().expect("inspect web asset");
        let path = entry.path();
        if kind.is_dir() {
            collect_files(root, &path, output);
        } else if kind.is_file() && path.starts_with(root) {
            output.push(path);
        }
    }
}
