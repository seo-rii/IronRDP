use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let source = PathBuf::from("../../src/image.rs");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed=policy.rs");

    // Append instrumentation in the same module to call the private helpers.
    // The production source, including its current selection policy, is unchanged.
    let mut instrumented = fs::read_to_string(source).expect("read production image.rs");
    instrumented.push_str(&fs::read_to_string("policy.rs").expect("read benchmark policy wrapper"));
    let destination = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("image.rs");
    fs::write(&destination, instrumented).expect("write benchmark image.rs");
    fs::write(
        destination.with_file_name("module.rs"),
        format!("#[path = {destination:?}] mod image;"),
    )
    .expect("write benchmark module declaration");
}
