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
    pub coeffects: Vec<String>,
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
    writeln!(out, "    /// Opaque CEL sub-expression that could not be raised to a physical IR node.").unwrap();
    writeln!(out, "    /// The `source` field is the reconstructed CEL text, kept for informational/").unwrap();
    writeln!(out, "    /// debugging purposes ONLY — it must NEVER be re-parsed for execution.").unwrap();
    writeln!(out, "    /// The authoritative representation is the compiled program in the side table.").unwrap();
    writeln!(out, "    CelFallback {{ source: String, args: Vec<Box<LogExpr>> }},").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "    /// Dispatch to a registered external UDF by function_id.").unwrap();
    writeln!(out, "    ExternalCall {{ function_id: String, args: Vec<Box<LogExpr>> }},").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "    /// Multi-way conditional (desugared CASE).").unwrap();
    writeln!(out, "    Case {{ arms: Vec<(Box<LogExpr>, Box<LogExpr>)>, default: Box<LogExpr> }},").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();

    emit_intrinsic_coeffects(out, scalars, notochord_ops, hofs);
    emit_transitive_coeffects(out);
}

fn emit_intrinsic_coeffects(
    out: &mut String,
    scalars: &[&CatalogEntry],
    notochord_ops: &[&CatalogEntry],
    hofs: &[&CatalogEntry],
) {
    gen_warning(out);
    writeln!(out, "impl LogExpr {{").unwrap();
    writeln!(out, "    pub fn intrinsic_coeffects(&self) -> crate::coeffects::CoeffectSet {{").unwrap();
    writeln!(out, "        use crate::coeffects::CoeffectSet;").unwrap();
    writeln!(out, "        match self {{").unwrap();

    writeln!(out, "            Self::GetFieldByName {{ .. }} | Self::GetFieldByIndex {{ .. }} => CoeffectSet::event_data(),").unwrap();
    writeln!(out, "            Self::CelFallback {{ .. }} => CoeffectSet::all(),").unwrap();
    writeln!(out, "            Self::ExternalCall {{ .. }} => {{").unwrap();
    writeln!(out, "                let mut s = CoeffectSet::new();").unwrap();
    writeln!(out, "                s.insert(crate::coeffects::Coeffect::CallsExternalUdf(crate::coeffects::UdfLanguage::Opaque));").unwrap();
    writeln!(out, "                s").unwrap();
    writeln!(out, "            }},").unwrap();

    for e in scalars.iter().chain(notochord_ops.iter()).chain(hofs.iter()) {
        if e.coeffects.is_empty() {
            continue;
        }
        let name = e.internal.as_ref().unwrap();
        // Build a chain of union() calls from the declared coeffects
        let mut parts: Vec<String> = Vec::new();
        if e.coeffects.contains(&"reads_event_data".to_string()) {
            parts.push("CoeffectSet::event_data()".to_string());
        }
        if e.coeffects.contains(&"reads_current_time".to_string()) {
            parts.push("CoeffectSet::current_time(0)".to_string());
        }
        if e.coeffects.contains(&"reads_aggregates".to_string()) {
            parts.push("CoeffectSet::aggregates()".to_string());
        }
        if e.coeffects.contains(&"reads_enrichment".to_string()) {
            parts.push("CoeffectSet::enrichment()".to_string());
        }
        if parts.is_empty() {
            continue;
        }
        let expr = parts.into_iter().reduce(|a, b| format!("{a}.union({b})")).unwrap();
        writeln!(out, "            Self::{name} {{ .. }} => {expr},").unwrap();
    }

    writeln!(out, "            _ => CoeffectSet::default(),").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();

    // is_commutative()
    writeln!(out, "    pub fn is_commutative(&self) -> bool {{").unwrap();
    writeln!(out, "        matches!(self,").unwrap();
    let commutative: Vec<&str> = scalars.iter()
        .filter(|e| e.commutative)
        .filter_map(|e| e.internal.as_deref())
        .collect();
    for (i, name) in commutative.iter().enumerate() {
        let sep = if i + 1 < commutative.len() { " |" } else { "" };
        writeln!(out, "            Self::{name} {{ .. }}{sep}").unwrap();
    }
    writeln!(out, "        )").unwrap();
    writeln!(out, "    }}").unwrap();

    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();
}

fn emit_transitive_coeffects(out: &mut String) {
    gen_warning(out);
    writeln!(out, "pub fn transitive_coeffects(expr: &LogExpr) -> crate::coeffects::CoeffectSet {{").unwrap();
    writeln!(out, "    let mut result = expr.intrinsic_coeffects();").unwrap();
    writeln!(out, "    match expr {{").unwrap();
    // Recursively union children. We enumerate the structural shapes.
    writeln!(out, "        LogExpr::Literal(_) => {{}},").unwrap();
    writeln!(out, "        LogExpr::GetFieldByName {{ .. }} | LogExpr::GetFieldByIndex {{ .. }} | LogExpr::CurrentTimestamp => {{}},").unwrap();

    // Unary: one child named `operand`
    writeln!(out, "        LogExpr::LogicalNot {{ operand }} | LogExpr::Negate {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::IsNull {{ operand }} | LogExpr::IsNotNull {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::IsNan {{ operand }} | LogExpr::IsFinite {{ operand }} | LogExpr::IsInfinite {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::Abs {{ operand }} | LogExpr::Sqrt {{ operand }} | LogExpr::Exp {{ operand }} | LogExpr::Sign {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::Size {{ operand }} | LogExpr::Lower {{ operand }} | LogExpr::Upper {{ operand }} | LogExpr::Trim {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::TimestampExtract {{ operand }} | LogExpr::RoundTemporal {{ operand }} | LogExpr::RoundCalendar {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::CastBool {{ operand }} | LogExpr::CastInt {{ operand }} | LogExpr::CastUint {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::CastDouble {{ operand }} | LogExpr::CastString {{ operand }} | LogExpr::CastBytes {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::CastDuration {{ operand }} | LogExpr::CastTimestamp {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::TypeOf {{ operand }} | LogExpr::Dyn {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::Ln {{ operand }} | LogExpr::Log10 {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::Ceil {{ operand }} | LogExpr::Floor {{ operand }} | LogExpr::Round {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::JsonParse {{ operand }} | LogExpr::JsonParseStruct {{ operand }} | LogExpr::JsonStringify {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::IpToInt {{ operand }} | LogExpr::IntToIp {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::Has {{ operand }}").unwrap();
    writeln!(out, "        | LogExpr::RaiseError {{ operand }}").unwrap();
    writeln!(out, "        => {{ result = result.union(transitive_coeffects(operand)); }},").unwrap();

    // Binary: lhs/rhs
    writeln!(out, "        LogExpr::LogicalOr {{ lhs, rhs }} | LogExpr::LogicalAnd {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::Equal {{ lhs, rhs }} | LogExpr::NotEqual {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::LessThan {{ lhs, rhs }} | LogExpr::LessOrEqual {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::GreaterThan {{ lhs, rhs }} | LogExpr::GreaterOrEqual {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::NullSafeEqual {{ lhs, rhs }} | LogExpr::NullSafeNotEqual {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::Coalesce {{ lhs, rhs }} | LogExpr::TryOrElse {{ lhs, rhs }} | LogExpr::Least {{ lhs, rhs }} | LogExpr::Greatest {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::Add {{ lhs, rhs }} | LogExpr::Subtract {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::Multiply {{ lhs, rhs }} | LogExpr::Divide {{ lhs, rhs }} | LogExpr::Modulus {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::Power {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::StringSplit {{ lhs, rhs }} | LogExpr::StringPosition {{ lhs, rhs }} | LogExpr::Concat {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::Index {{ lhs, rhs }} | LogExpr::In {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::JsonExtract {{ lhs, rhs }} | LogExpr::JsonExtractString {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::CidrContains {{ lhs, rhs }} | LogExpr::CidrMatch {{ lhs, rhs }}").unwrap();
    writeln!(out, "        | LogExpr::RegexExtract {{ lhs, rhs }}").unwrap();
    writeln!(out, "        => {{").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(lhs));").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(rhs));").unwrap();
    writeln!(out, "        }},").unwrap();

    // Receiver + arg
    writeln!(out, "        LogExpr::Contains {{ receiver, arg }} | LogExpr::StartsWith {{ receiver, arg }}").unwrap();
    writeln!(out, "        | LogExpr::EndsWith {{ receiver, arg }} | LogExpr::RegexMatch {{ receiver, arg }}").unwrap();
    writeln!(out, "        => {{").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(receiver));").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(arg));").unwrap();
    writeln!(out, "        }},").unwrap();

    // Ternary
    writeln!(out, "        LogExpr::Between {{ arg0, arg1, arg2 }} | LogExpr::Substring {{ arg0, arg1, arg2 }}").unwrap();
    writeln!(out, "        | LogExpr::Replace {{ arg0, arg1, arg2 }} | LogExpr::RegexReplace {{ arg0, arg1, arg2 }}").unwrap();
    writeln!(out, "        => {{").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(arg0));").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(arg1));").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(arg2));").unwrap();
    writeln!(out, "        }},").unwrap();

    writeln!(out, "        LogExpr::Conditional {{ condition, then_expr, else_expr }} => {{").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(condition));").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(then_expr));").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(else_expr));").unwrap();
    writeln!(out, "        }},").unwrap();

    // GetChildByName has operand
    writeln!(out, "        LogExpr::GetChildByName {{ operand, .. }} => {{").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(operand));").unwrap();
    writeln!(out, "        }},").unwrap();
    writeln!(out, "        LogExpr::GetChildByIndex {{ lhs, rhs, .. }} => {{").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(lhs));").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(rhs));").unwrap();
    writeln!(out, "        }},").unwrap();

    // HOFs: collection + body
    writeln!(out, "        LogExpr::All {{ collection, body, .. }} | LogExpr::Exists {{ collection, body, .. }}").unwrap();
    writeln!(out, "        | LogExpr::ExistsOne {{ collection, body, .. }} | LogExpr::Filter {{ collection, body, .. }}").unwrap();
    writeln!(out, "        | LogExpr::MapTransform {{ collection, body, .. }}").unwrap();
    writeln!(out, "        => {{").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(collection));").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(body));").unwrap();
    writeln!(out, "        }},").unwrap();

    // CelFallback / ExternalCall: args
    writeln!(out, "        LogExpr::CelFallback {{ args, .. }} | LogExpr::ExternalCall {{ args, .. }} => {{").unwrap();
    writeln!(out, "            for arg in args {{").unwrap();
    writeln!(out, "                result = result.union(transitive_coeffects(arg));").unwrap();
    writeln!(out, "            }}").unwrap();
    writeln!(out, "        }},").unwrap();

    // Case: arms + default
    writeln!(out, "        LogExpr::Case {{ arms, default }} => {{").unwrap();
    writeln!(out, "            for (cond, body) in arms {{").unwrap();
    writeln!(out, "                result = result.union(transitive_coeffects(cond));").unwrap();
    writeln!(out, "                result = result.union(transitive_coeffects(body));").unwrap();
    writeln!(out, "            }}").unwrap();
    writeln!(out, "            result = result.union(transitive_coeffects(default));").unwrap();
    writeln!(out, "        }},").unwrap();

    writeln!(out, "    }}").unwrap();
    writeln!(out, "    result").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();

    emit_is_foldable(out);
}

fn emit_is_foldable(out: &mut String) {
    gen_warning(out);
    writeln!(out, "impl LogExpr {{").unwrap();
    writeln!(out, "    /// True if this expression tree can be evaluated at compile time --").unwrap();
    writeln!(out, "    /// no event data, no session state, no aggregates, no enrichment.").unwrap();
    writeln!(out, "    /// Useful for predicate pushdown: e.g. `IN (x, y, z)` can only be").unwrap();
    writeln!(out, "    /// rewritten to a hash lookup if all elements are foldable.").unwrap();
    writeln!(out, "    pub fn is_foldable(&self) -> bool {{").unwrap();
    writeln!(out, "        transitive_coeffects(self).is_pure()").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();
}
