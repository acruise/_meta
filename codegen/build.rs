use std::path::PathBuf;

#[path = "src/codegen.rs"]
mod codegen;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let proto_root = PathBuf::from("../proto");
    let vendor_root = proto_root.join("vendor");

    // --- CEL protobuf (syntax.proto, checked.proto) ---

    let mut config = prost_build::Config::new();
    config.disable_comments(&["."]);

    config.compile_protos(
        &[
            vendor_root.join("cel/expr/syntax.proto"),
            vendor_root.join("cel/expr/checked.proto"),
        ],
        &[&vendor_root],
    )?;

    // --- LogExpr codegen ---

    let yaml = std::fs::read_to_string("../function-catalog.yaml")?;
    let codegen_source = include_str!("src/codegen.rs");

    let result = codegen::generate(&yaml, codegen_source)
        .map_err(|e| format!("LogExpr codegen failed: {e}"))?;

    std::fs::write(out_dir.join("expr_gen.rs"), &result.code)?;

    println!("cargo::rerun-if-changed=../function-catalog.yaml");
    println!("cargo::rerun-if-changed=src/codegen.rs");

    Ok(())
}
