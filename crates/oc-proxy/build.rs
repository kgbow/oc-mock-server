use std::fs;
use std::path::Path;

fn main() {
    let dist = Path::new("../../web/dist");
    if !dist.exists() {
        let _ = fs::create_dir_all(dist);
    }
    println!("cargo:rerun-if-changed=../../web/dist");
}
