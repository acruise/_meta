use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();
    let catalog_path = args.get(1).unwrap_or_else(|| {
        eprintln!("Usage: meta-codegen <path-to-function-catalog.yaml>");
        process::exit(1);
    });

    let yaml = fs::read_to_string(catalog_path).unwrap_or_else(|e| {
        eprintln!("Failed to read {catalog_path}: {e}");
        process::exit(1);
    });

    let codegen_source = include_str!("codegen.rs");

    let result = meta_codegen::codegen::generate(&yaml, codegen_source).unwrap_or_else(|e| {
        eprintln!("{e}");
        process::exit(1);
    });

    print!("{}", result.code);

    eprintln!(
        "Generated: {} scalar, {} hof, {} notochord-specific",
        result.scalar_count,
        result.hof_count,
        result.property_count,
    );
}
