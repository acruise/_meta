#![allow(dead_code)]

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::hash::{Hash, Hasher};

#[derive(Debug, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub internal: Option<String>,
    pub cel: Option<String>,
    pub substrait: Option<SubstraitRef>,
    pub status: String,
    #[serde(default)]
    pub commutative: bool,
    #[serde(default)]
    pub short_circuits: bool,
    #[serde(default)]
    pub aggregate: bool,
    #[serde(default)]
    pub r#macro: bool,
    #[serde(default)]
    pub hof: bool,
    pub lambda: Option<LambdaSpec>,
    pub child_names: Option<Vec<String>>,
    pub special: Option<String>,
    pub null_semantics: Option<String>,
    pub params: Option<Vec<String>>,
    pub r#return: Option<String>,
    pub receiver: Option<String>,
    pub properties: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LambdaSpec {
    pub binding: String,
    pub r#return: String,
}

#[derive(Debug, Deserialize)]
pub struct SubstraitRef {
    pub ext: String,
    pub name: String,
}

impl CatalogEntry {
    pub fn child_fields(&self) -> Vec<(&str, &str)> {
        if let Some(names) = &self.child_names {
            return names.iter().map(|n| (n.as_str(), "ExprId")).collect();
        }

        let has_receiver = self.receiver.is_some();
        let param_count = self.params.as_ref().map_or(0, |p| p.len());
        let total = param_count + if has_receiver { 1 } else { 0 };

        match (has_receiver, param_count, total) {
            (_, _, 0) => vec![],
            (false, 1, 1) => vec![("operand", "ExprId")],
            (false, 2, 2) => vec![("lhs", "ExprId"), ("rhs", "ExprId")],
            (false, 3, 3) => {
                vec![("arg0", "ExprId"), ("arg1", "ExprId"), ("arg2", "ExprId")]
            }
            (true, 0, 1) => vec![("receiver", "ExprId")],
            (true, 1, 2) => vec![("receiver", "ExprId"), ("arg", "ExprId")],
            (true, n, _) => {
                let mut fields = vec![("receiver", "ExprId")];
                for _ in 0..n {
                    fields.push(("arg", "ExprId"));
                }
                fields
            }
            _ => {
                let mut fields = Vec::new();
                for _ in 0..total {
                    fields.push(("arg", "ExprId"));
                }
                fields
            }
        }
    }
}

pub struct ParsedCatalog {
    pub entries: Vec<CatalogEntry>,
}

impl ParsedCatalog {
    pub fn active(&self) -> Vec<&CatalogEntry> {
        self.entries
            .iter()
            .filter(|e| {
                e.internal.is_some()
                    && (e.status == "mapped" || e.status == "todo" || e.status == "partial")
            })
            .collect()
    }

    pub fn scalars(&self) -> Vec<&CatalogEntry> {
        self.active()
            .into_iter()
            .filter(|e| !e.aggregate && !e.r#macro && !e.hof && e.properties.is_none() && e.special.is_none())
            .collect()
    }

    pub fn with_properties(&self) -> Vec<&CatalogEntry> {
        self.active()
            .into_iter()
            .filter(|e| e.properties.is_some())
            .collect()
    }

    pub fn aggregates(&self) -> Vec<&CatalogEntry> {
        self.active().into_iter().filter(|e| e.aggregate).collect()
    }

    pub fn hofs(&self) -> Vec<&CatalogEntry> {
        self.active().into_iter().filter(|e| e.hof).collect()
    }
}

pub fn parse_catalog(yaml: &str) -> Result<ParsedCatalog, String> {
    let entries: Vec<CatalogEntry> =
        serde_yaml::from_str(yaml).map_err(|e| format!("Failed to parse YAML: {e}"))?;
    Ok(ParsedCatalog { entries })
}

pub fn content_hash(content: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

pub fn combined_hash(catalog_hash: u64, codegen_hash: u64) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    catalog_hash.hash(&mut hasher);
    codegen_hash.hash(&mut hasher);
    hasher.finish()
}

pub fn deduplicate_fields(fields: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (name, _) in fields {
        *counts.entry(name).or_insert(0) += 1;
    }

    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    fields
        .iter()
        .map(|(name, ty)| {
            let count = counts[name];
            if count > 1 {
                let idx = seen.entry(name).or_insert(0);
                let result = (format!("{name}{idx}"), ty.to_string());
                *idx += 1;
                result
            } else {
                (name.to_string(), ty.to_string())
            }
        })
        .collect()
}

pub fn yaml_type_to_rust(t: &str) -> &str {
    match t {
        "string" => "String",
        "int" => "i64",
        "u32" => "u32",
        "bool" => "bool",
        _ => "String",
    }
}

// --- LogExpr generation (used by _meta's build.rs) ---

pub struct CodegenResult {
    pub code: String,
    pub scalar_count: usize,
    pub hof_count: usize,
    pub property_count: usize,
}

pub fn generate(yaml: &str, codegen_source: &str) -> Result<CodegenResult, String> {
    let catalog = parse_catalog(yaml)?;
    let scalars = catalog.scalars();
    let with_properties = catalog.with_properties();
    let hofs = catalog.hofs();

    let catalog_hash = content_hash(yaml);
    let codegen_hash = content_hash(codegen_source);
    let combined = combined_hash(catalog_hash, codegen_hash);

    let mut out = String::new();

    writeln!(out, "// Generated from function-catalog.yaml — do not edit manually.").unwrap();
    writeln!(out, "// Re-generate with:").unwrap();
    writeln!(out, "//   cargo run -q -p meta-codegen -- function-catalog.yaml").unwrap();
    writeln!(out, "// CATALOG_HASH: {catalog_hash:016x}").unwrap();
    writeln!(out, "// CODEGEN_HASH: {codegen_hash:016x}").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "use crate::value::Value;").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "/// Combined hash of the catalog + codegen that produced this file.").unwrap();
    writeln!(out, "/// Consumers that pattern-match on generated types should assert").unwrap();
    writeln!(out, "/// against this constant to detect silent schema drift.").unwrap();
    writeln!(out, "pub const EXPR_GEN_HASH: u64 = 0x{combined:016x};").unwrap();
    writeln!(out).unwrap();

    emit_logical_ir(&mut out, &scalars, &with_properties, &hofs);

    Ok(CodegenResult {
        code: out,
        scalar_count: scalars.len(),
        hof_count: hofs.len(),
        property_count: with_properties.len(),
    })
}

fn gen_warning(out: &mut String) {
    writeln!(out, "/// Generated — do not modify. See top of file.").unwrap();
}

fn emit_logical_ir(
    out: &mut String,
    scalars: &[&CatalogEntry],
    notochord_ops: &[&CatalogEntry],
    hofs: &[&CatalogEntry],
) {
    gen_warning(out);
    writeln!(out, "/// Logical expression tree — value-based, fully recursive.").unwrap();
    writeln!(out, "///").unwrap();
    writeln!(out, "/// Used for normalization and rewrite rules where nested").unwrap();
    writeln!(out, "/// pattern matching is the natural style.").unwrap();
    writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq, Hash)]").unwrap();
    writeln!(out, "pub enum LogExpr {{").unwrap();
    writeln!(out, "    Literal(Value),").unwrap();
    writeln!(out).unwrap();

    for e in scalars {
        let name = e.internal.as_ref().unwrap();
        let fields = e.child_fields();

        if let Some(notes) = &e.notes {
            let first_line = notes.lines().next().unwrap_or("").trim();
            if !first_line.is_empty() {
                writeln!(out, "    /// {first_line}").unwrap();
            }
        }

        if fields.is_empty() {
            writeln!(out, "    {name},").unwrap();
        } else {
            let deduped = deduplicate_fields(&fields);
            let field_str: Vec<String> = deduped
                .iter()
                .map(|(k, _)| format!("{k}: Box<LogExpr>"))
                .collect();
            writeln!(out, "    {name} {{ {} }},", field_str.join(", ")).unwrap();
        }
    }

    writeln!(out).unwrap();
    for e in notochord_ops {
        let name = e.internal.as_ref().unwrap();
        let props = e.properties.as_ref().unwrap();
        let children = e.child_fields();
        let deduped_children = deduplicate_fields(&children);

        let mut all_fields: Vec<String> = Vec::new();
        for (k, v) in props {
            let rust_type = yaml_type_to_rust(v);
            all_fields.push(format!("{k}: {rust_type}"));
        }
        for (k, _) in &deduped_children {
            all_fields.push(format!("{k}: Box<LogExpr>"));
        }
        writeln!(out, "    {name} {{ {} }},", all_fields.join(", ")).unwrap();
    }

    writeln!(out).unwrap();
    for e in hofs {
        let name = e.internal.as_ref().unwrap();
        if e.lambda.is_some() {
            writeln!(
                out,
                "    {name} {{ collection: Box<LogExpr>, binding: String, body: Box<LogExpr> }},"
            ).unwrap();
        } else {
            let fields = e.child_fields();
            let deduped = deduplicate_fields(&fields);
            let field_str: Vec<String> = deduped
                .iter()
                .map(|(k, _)| format!("{k}: Box<LogExpr>"))
                .collect();
            writeln!(out, "    {name} {{ {} }},", field_str.join(", ")).unwrap();
        }
    }

    writeln!(out).unwrap();
    writeln!(out, "    /// Multi-way conditional (desugared CASE).").unwrap();
    writeln!(out, "    Case {{ arms: Vec<(Box<LogExpr>, Box<LogExpr>)>, default: Box<LogExpr> }},").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();
}
