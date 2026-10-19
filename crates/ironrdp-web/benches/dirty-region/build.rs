use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    for variable in [
        "IRONRDP_BENCH_IMAGE_SOURCE",
        "IRONRDP_BENCH_CANVAS_SOURCE",
        "IRONRDP_BENCH_VARIANT",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let selected_variant = env::var("IRONRDP_BENCH_VARIANT");
    for variant in ["simd", "fused", "candidate"] {
        println!("cargo:rustc-check-cfg=cfg(presentation_{variant})");
        if selected_variant.as_deref() == Ok(variant) {
            println!("cargo:rustc-cfg=presentation_{variant}");
        }
    }
    println!("cargo:rustc-check-cfg=cfg(presentation_direct)");
    if matches!(selected_variant.as_deref(), Ok("baseline" | "candidate")) {
        println!("cargo:rustc-cfg=presentation_direct");
    }
    let source =
        env::var_os("IRONRDP_BENCH_IMAGE_SOURCE").map_or_else(|| PathBuf::from("../../src/image.rs"), PathBuf::from);
    let canvas_source =
        env::var_os("IRONRDP_BENCH_CANVAS_SOURCE").map_or_else(|| PathBuf::from("../../src/canvas.rs"), PathBuf::from);
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", canvas_source.display());
    println!("cargo:rerun-if-changed=policy.rs");

    // Append instrumentation in the same module to call the private helpers.
    // The production source, including its current selection policy, is unchanged.
    let mut instrumented = fs::read_to_string(source).expect("read production image.rs");
    instrumented.push_str(&fs::read_to_string("policy.rs").expect("read benchmark policy wrapper"));
    let destination = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("image.rs");
    fs::write(&destination, instrumented).expect("write benchmark image.rs");
    let canvas_destination = destination.with_file_name("canvas.rs");
    fs::copy(canvas_source, &canvas_destination).expect("copy benchmark canvas.rs");
    fs::write(
        destination.with_file_name("module.rs"),
        format!(
            "#[path = {destination:?}] mod image;\n\
             #[cfg(target_arch = \"wasm32\")] #[path = {canvas_destination:?}] mod canvas;"
        ),
    )
    .expect("write benchmark module declaration");
}
