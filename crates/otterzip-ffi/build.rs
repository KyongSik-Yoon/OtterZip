//! Generates the C header (`include/otterzip.h`) from the Rust FFI source.
//! The generated header is committed to Git as the ABI contract artifact.

use std::{env, path::PathBuf};

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_header = PathBuf::from(&crate_dir).join("include").join("otterzip.h");

    std::fs::create_dir_all(out_header.parent().unwrap()).ok();

    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cbindgen::Config::from_file(format!("{crate_dir}/cbindgen.toml")).unwrap())
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&out_header);
        }
        Err(e) => {
            // Non-fatal during early development — header will be empty
            println!("cargo:warning=cbindgen failed (stub phase): {e}");
        }
    }

    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
