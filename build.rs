extern crate bindgen;

use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-link-lib=fuse3");
    println!("cargo:rerun-if-changed=wrapper.h");
    bindgen::Builder::default()
        .header("wrapper.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .generate()
        .expect("bindgen fail")
        .write_to_file("src/fuse3_sys.rs")
        .expect("failed to write bindings");
}