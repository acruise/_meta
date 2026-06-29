//! A YAML surface syntax for `ValueType`, so a type can sit in a catalog
//! and be referenced by name, or be written inline.
//!
//! This is the single recursive codec for types-as-YAML. (The shallow
//! `parse_simple_type` in `udf_catalog` predates it and only handles flat
//! scalars; it should eventually fold onto this.)
//!
//! # Surface forms
//!
//! A type node is either a **string** -- a scalar keyword or the name of a
//! declared type -- or a single-key **mapping** naming a constructor:
//!
//! ```yaml
//! # scalars (bare keywords)
//! string            # ValueType::String
//! i64               # ValueType::I64   (alias: int)
//! f64               # ValueType::F64   (aliases: double, float)
//! timestamp         # millis / no-tz default; see the mapping form for params
//!
//! # parameterized scalars (mapping form)
//! { decimal:   { precision: 18, scale: 2 } }
//! { timestamp: { precision: micros, timezone: utc_offset } }
//!
//! # compounds
//! { array:  { element: string, nullable: false } }
//! { map:    { key: string, value: i64, nullable: true } }
//! { struct: [ { name: lat, type: f64 }, { name: lon, type: f64 } ] }
//! { enum:   [ red, green, blue ] }
//! { entity_ref: { target_type_id: "user", key: uuid, revision_pinnable: false } }
//!
//! # a reference to a declared type, by name
//! Address                 # bare string that is not a scalar keyword
//! { ref: Address }        # explicit form (never mistaken for a scalar)
//! ```
//!
//! A **catalog** is a `types:` mapping of name to type node; declarations may
//! reference each other (resolved in any order, cycles rejected):
//!
//! ```yaml
//! types:
//!   GeoPoint:
//!     struct:
//!       - { name: lat, type: f64 }
//!       - { name: lon, type: f64 }
//!   Place:
//!     struct:
//!       - { name: name, type: string }
//!       - { name: at,   type: GeoPoint }   # reference by name
//! ```

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};

use serde_yaml::Value as Yaml;

use crate::value::{StructField, TimestampPrecision, TimestampTimezone, ValueType};

/// A resolved set of named `ValueType` declarations.
#[derive(Debug, Clone, Default)]
pub struct TypeCatalog {
    types: BTreeMap<String, ValueType>,
}

impl TypeCatalog {
    /// A catalog with no declarations -- the right thing for parsing a purely
    /// inline type with no names to resolve.
    pub fn empty() -> Self {
        TypeCatalog::default()
    }

    pub fn get(&self, name: &str) -> Option<&ValueType> {
        self.types.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.types.keys().map(String::as_str)
    }

    /// Parse a `{ types: { name: <type> } }` document, resolving inter-type
    /// references. Errors on an unknown reference or a reference cycle.
    pub fn from_yaml(yaml: &str) -> Result<TypeCatalog, String> {
        let doc: Yaml = serde_yaml::from_str(yaml).map_err(|e| format!("invalid YAML: {e}"))?;
        let map = doc.as_mapping().ok_or("type catalog must be a mapping")?;
        let types_node = map
            .get("types")
            .ok_or("type catalog must have a `types:` key")?;
        let types_map = types_node
            .as_mapping()
            .ok_or("`types` must be a mapping of name -> type")?;

        let mut raw = BTreeMap::new();
        for (k, v) in types_map.iter() {
            let name = k.as_str().ok_or("type name must be a string")?;
            raw.insert(name.to_string(), v.clone());
        }

        let resolver = BuildResolver {
            raw: &raw,
            resolved: RefCell::new(BTreeMap::new()),
            in_progress: RefCell::new(HashSet::new()),
        };
        for name in raw.keys() {
            resolver.resolve(name)?;
        }
        Ok(TypeCatalog { types: resolver.resolved.into_inner() })
    }
}

/// Parse a single inline type node against a catalog (use [`TypeCatalog::empty`]
/// when there are no names to resolve).
pub fn parse_type(yaml: &str, catalog: &TypeCatalog) -> Result<ValueType, String> {
    let node: Yaml = serde_yaml::from_str(yaml).map_err(|e| format!("invalid YAML: {e}"))?;
    parse_value_type(&node, catalog)
}

/// Parse a pre-deserialized YAML node into a `ValueType`, resolving named
/// references against `catalog`.
pub fn parse_value_type(node: &Yaml, catalog: &TypeCatalog) -> Result<ValueType, String> {
    parse_node(node, &CatalogResolver { catalog })
}

// ---------------------------------------------------------------------------
// Name resolution
// ---------------------------------------------------------------------------

/// How a bare name that is not a scalar keyword gets turned into a type. The
/// two implementations are "look it up in a finished catalog" and "resolve it
/// against the still-building catalog" (the latter detects cycles).
trait NameResolver {
    fn resolve(&self, name: &str) -> Result<ValueType, String>;
}

struct CatalogResolver<'a> {
    catalog: &'a TypeCatalog,
}

impl NameResolver for CatalogResolver<'_> {
    fn resolve(&self, name: &str) -> Result<ValueType, String> {
        self.catalog
            .get(name)
            .cloned()
            .ok_or_else(|| format!("unknown type or reference: `{name}`"))
    }
}

struct BuildResolver<'a> {
    raw: &'a BTreeMap<String, Yaml>,
    resolved: RefCell<BTreeMap<String, ValueType>>,
    in_progress: RefCell<HashSet<String>>,
}

impl NameResolver for BuildResolver<'_> {
    fn resolve(&self, name: &str) -> Result<ValueType, String> {
        if let Some(t) = self.resolved.borrow().get(name) {
            return Ok(t.clone());
        }
        if self.in_progress.borrow().contains(name) {
            return Err(format!("cyclic type reference involving `{name}`"));
        }
        let node = self
            .raw
            .get(name)
            .ok_or_else(|| format!("unknown type reference: `{name}`"))?;
        self.in_progress.borrow_mut().insert(name.to_string());
        let t = parse_node(node, self)?;
        self.in_progress.borrow_mut().remove(name);
        self.resolved.borrow_mut().insert(name.to_string(), t.clone());
        Ok(t)
    }
}

// ---------------------------------------------------------------------------
// The recursive walker
// ---------------------------------------------------------------------------

fn parse_node(node: &Yaml, r: &dyn NameResolver) -> Result<ValueType, String> {
    match node {
        Yaml::String(s) => match scalar_keyword(s) {
            Some(vt) => Ok(vt),
            None => r.resolve(s),
        },
        Yaml::Mapping(_) => parse_compound(node, r),
        other => Err(format!("expected a type (string or mapping), found {}", yaml_kind(other))),
    }
}

fn parse_compound(node: &Yaml, r: &dyn NameResolver) -> Result<ValueType, String> {
    let m = node.as_mapping().expect("caller checked mapping");
    if m.len() != 1 {
        return Err(format!(
            "a type mapping must have exactly one constructor key, found {}",
            m.len()
        ));
    }
    let (k, v) = m.iter().next().unwrap();
    let key = k.as_str().ok_or("type constructor key must be a string")?;

    match key {
        "decimal" => {
            let dm = as_map(v, "decimal")?;
            Ok(ValueType::Decimal {
                precision: get_u32(dm, "precision", "decimal")?,
                scale: get_u32(dm, "scale", "decimal")?,
            })
        }
        "timestamp" => {
            let tm = as_map(v, "timestamp")?;
            Ok(ValueType::Timestamp {
                precision: ts_precision(req_str(tm, "precision", "timestamp")?)?,
                timezone: ts_timezone(req_str(tm, "timezone", "timestamp")?)?,
            })
        }
        "array" => {
            let am = as_map(v, "array")?;
            Ok(ValueType::Array {
                element_type: Box::new(parse_node(req_field(am, "element", "array")?, r)?),
                elements_nullable: opt_bool(am, "nullable"),
            })
        }
        "map" => {
            let mm = as_map(v, "map")?;
            Ok(ValueType::Map {
                key_type: Box::new(parse_node(req_field(mm, "key", "map")?, r)?),
                value_type: Box::new(parse_node(req_field(mm, "value", "map")?, r)?),
                values_nullable: opt_bool(mm, "nullable"),
            })
        }
        "struct" => {
            let seq = v.as_sequence().ok_or("`struct` must be a sequence of fields")?;
            let mut fields = Vec::with_capacity(seq.len());
            for (i, f) in seq.iter().enumerate() {
                let fm = as_map(f, "struct field")?;
                let name = req_str(fm, "name", "struct field")?.to_string();
                let human_name = fm
                    .get("human_name")
                    .and_then(Yaml::as_str)
                    .unwrap_or(&name)
                    .to_string();
                let value_type = parse_node(req_field(fm, "type", "struct field")?, r)
                    .map_err(|e| format!("struct field #{i} (`{name}`): {e}"))?;
                fields.push(StructField {
                    name,
                    human_name,
                    value_type,
                    nullable: opt_bool(fm, "nullable"),
                });
            }
            Ok(ValueType::Struct { fields })
        }
        "enum" => {
            let seq = v.as_sequence().ok_or("`enum` must be a sequence of strings")?;
            let values = seq
                .iter()
                .map(|x| x.as_str().map(str::to_string).ok_or_else(|| "enum values must be strings".to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ValueType::Enum { values })
        }
        "entity_ref" => {
            let em = as_map(v, "entity_ref")?;
            Ok(ValueType::EntityRef {
                target_type_id: req_str(em, "target_type_id", "entity_ref")?.to_string(),
                key_type: Box::new(parse_node(req_field(em, "key", "entity_ref")?, r)?),
                revision_pinnable: opt_bool(em, "revision_pinnable"),
            })
        }
        "ref" => {
            let name = v.as_str().ok_or("`ref` must be a string name")?;
            r.resolve(name)
        }
        other => Err(format!("unknown type constructor `{other}`")),
    }
}

/// Bare scalar keywords (and a couple of convenience defaults for the
/// parameterized scalars). `None` means "not a keyword" -- treat as a name.
fn scalar_keyword(s: &str) -> Option<ValueType> {
    Some(match s {
        "bool" | "boolean" => ValueType::Bool,
        "i8" => ValueType::I8,
        "i16" => ValueType::I16,
        "i32" => ValueType::I32,
        "i64" | "int" => ValueType::I64,
        "u8" => ValueType::U8,
        "u16" => ValueType::U16,
        "u32" => ValueType::U32,
        "u64" | "uint" => ValueType::U64,
        "f32" => ValueType::F32,
        "f64" | "double" | "float" => ValueType::F64,
        "string" => ValueType::String,
        "blob" | "bytes" => ValueType::Blob,
        "clob" => ValueType::Clob,
        "date" => ValueType::Date,
        "uuid" => ValueType::Uuid,
        "ipv4" => ValueType::Ipv4,
        "ipv6" => ValueType::Ipv6,
        "null" => ValueType::Null,
        // Convenience defaults; use the mapping form for explicit parameters.
        "timestamp" => ValueType::Timestamp {
            precision: TimestampPrecision::Millis,
            timezone: TimestampTimezone::None,
        },
        "decimal" => ValueType::Decimal { precision: 38, scale: 0 },
        _ => return None,
    })
}

fn ts_precision(s: &str) -> Result<TimestampPrecision, String> {
    Ok(match s {
        "unspecified" => TimestampPrecision::Unspecified,
        "seconds" | "s" => TimestampPrecision::Seconds,
        "millis" | "ms" => TimestampPrecision::Millis,
        "micros" | "us" => TimestampPrecision::Micros,
        "nanos" | "ns" => TimestampPrecision::Nanos,
        other => return Err(format!("unknown timestamp precision `{other}`")),
    })
}

fn ts_timezone(s: &str) -> Result<TimestampTimezone, String> {
    Ok(match s {
        "none" => TimestampTimezone::None,
        "utc_offset" => TimestampTimezone::UtcOffset,
        other => return Err(format!("unknown timestamp timezone `{other}`")),
    })
}

// --- small YAML accessors with honest errors ---

fn as_map<'a>(y: &'a Yaml, ctx: &str) -> Result<&'a serde_yaml::Mapping, String> {
    y.as_mapping().ok_or_else(|| format!("`{ctx}` must be a mapping"))
}

fn req_field<'a>(m: &'a serde_yaml::Mapping, key: &str, ctx: &str) -> Result<&'a Yaml, String> {
    m.get(key).ok_or_else(|| format!("`{ctx}` is missing required field `{key}`"))
}

fn req_str<'a>(m: &'a serde_yaml::Mapping, key: &str, ctx: &str) -> Result<&'a str, String> {
    req_field(m, key, ctx)?
        .as_str()
        .ok_or_else(|| format!("`{ctx}.{key}` must be a string"))
}

fn get_u32(m: &serde_yaml::Mapping, key: &str, ctx: &str) -> Result<u32, String> {
    req_field(m, key, ctx)?
        .as_u64()
        .map(|n| n as u32)
        .ok_or_else(|| format!("`{ctx}.{key}` must be a non-negative integer"))
}

fn opt_bool(m: &serde_yaml::Mapping, key: &str) -> bool {
    m.get(key).and_then(Yaml::as_bool).unwrap_or(false)
}

fn yaml_kind(y: &Yaml) -> &'static str {
    match y {
        Yaml::Null => "null",
        Yaml::Bool(_) => "bool",
        Yaml::Number(_) => "number",
        Yaml::String(_) => "string",
        Yaml::Sequence(_) => "sequence",
        Yaml::Mapping(_) => "mapping",
        Yaml::Tagged(_) => "tagged",
    }
}

// ---------------------------------------------------------------------------
// Encoding (ValueType -> YAML, inline only)
// ---------------------------------------------------------------------------

/// Encode a `ValueType` to a YAML node in this surface syntax. Always inline:
/// a `ValueType` does not remember which named declaration it came from, so
/// names are not recovered.
pub fn to_yaml(vt: &ValueType) -> Yaml {
    match vt {
        ValueType::Null => s("null"),
        ValueType::Bool => s("bool"),
        ValueType::I8 => s("i8"),
        ValueType::I16 => s("i16"),
        ValueType::I32 => s("i32"),
        ValueType::I64 => s("i64"),
        ValueType::U8 => s("u8"),
        ValueType::U16 => s("u16"),
        ValueType::U32 => s("u32"),
        ValueType::U64 => s("u64"),
        ValueType::F32 => s("f32"),
        ValueType::F64 => s("f64"),
        ValueType::String => s("string"),
        ValueType::Blob => s("blob"),
        ValueType::Clob => s("clob"),
        ValueType::Date => s("date"),
        ValueType::Uuid => s("uuid"),
        ValueType::Ipv4 => s("ipv4"),
        ValueType::Ipv6 => s("ipv6"),
        ValueType::Decimal { precision, scale } => {
            single("decimal", map_of([("precision", num(*precision as u64)), ("scale", num(*scale as u64))]))
        }
        ValueType::Timestamp { precision, timezone } => single(
            "timestamp",
            map_of([
                ("precision", s(ts_precision_str(*precision))),
                ("timezone", s(ts_timezone_str(*timezone))),
            ]),
        ),
        ValueType::Array { element_type, elements_nullable } => single(
            "array",
            map_of([("element", to_yaml(element_type)), ("nullable", Yaml::Bool(*elements_nullable))]),
        ),
        ValueType::Map { key_type, value_type, values_nullable } => single(
            "map",
            map_of([
                ("key", to_yaml(key_type)),
                ("value", to_yaml(value_type)),
                ("nullable", Yaml::Bool(*values_nullable)),
            ]),
        ),
        ValueType::Struct { fields } => {
            let items = fields
                .iter()
                .map(|f| {
                    map_of([
                        ("name", s(&f.name)),
                        ("type", to_yaml(&f.value_type)),
                        ("nullable", Yaml::Bool(f.nullable)),
                    ])
                })
                .collect();
            single("struct", Yaml::Sequence(items))
        }
        ValueType::Enum { values } => {
            single("enum", Yaml::Sequence(values.iter().map(|v| s(v)).collect()))
        }
        ValueType::EntityRef { target_type_id, key_type, revision_pinnable } => single(
            "entity_ref",
            map_of([
                ("target_type_id", s(target_type_id)),
                ("key", to_yaml(key_type)),
                ("revision_pinnable", Yaml::Bool(*revision_pinnable)),
            ]),
        ),
    }
}

fn ts_precision_str(p: TimestampPrecision) -> &'static str {
    match p {
        TimestampPrecision::Unspecified => "unspecified",
        TimestampPrecision::Seconds => "seconds",
        TimestampPrecision::Millis => "millis",
        TimestampPrecision::Micros => "micros",
        TimestampPrecision::Nanos => "nanos",
    }
}

fn ts_timezone_str(t: TimestampTimezone) -> &'static str {
    match t {
        TimestampTimezone::None => "none",
        TimestampTimezone::UtcOffset => "utc_offset",
    }
}

fn s(v: &str) -> Yaml {
    Yaml::String(v.to_string())
}

fn num(n: u64) -> Yaml {
    Yaml::Number(n.into())
}

fn single(key: &str, value: Yaml) -> Yaml {
    let mut m = serde_yaml::Mapping::new();
    m.insert(s(key), value);
    Yaml::Mapping(m)
}

fn map_of<const N: usize>(pairs: [(&str, Yaml); N]) -> Yaml {
    let mut m = serde_yaml::Mapping::new();
    for (k, v) in pairs {
        m.insert(s(k), v);
    }
    Yaml::Mapping(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inline(yaml: &str) -> Result<ValueType, String> {
        parse_type(yaml, &TypeCatalog::empty())
    }

    #[test]
    fn scalars_and_aliases() {
        assert_eq!(inline("string").unwrap(), ValueType::String);
        assert_eq!(inline("int").unwrap(), ValueType::I64);
        assert_eq!(inline("double").unwrap(), ValueType::F64);
        assert_eq!(inline("uuid").unwrap(), ValueType::Uuid);
    }

    #[test]
    fn parameterized_scalars() {
        assert_eq!(
            inline("{ decimal: { precision: 18, scale: 2 } }").unwrap(),
            ValueType::Decimal { precision: 18, scale: 2 }
        );
        assert_eq!(
            inline("{ timestamp: { precision: micros, timezone: utc_offset } }").unwrap(),
            ValueType::Timestamp {
                precision: TimestampPrecision::Micros,
                timezone: TimestampTimezone::UtcOffset,
            }
        );
        // bare keyword defaults
        assert_eq!(
            inline("timestamp").unwrap(),
            ValueType::Timestamp { precision: TimestampPrecision::Millis, timezone: TimestampTimezone::None }
        );
    }

    #[test]
    fn nested_compound() {
        let vt = inline("{ array: { element: { map: { key: string, value: i64, nullable: true } } } }").unwrap();
        assert_eq!(
            vt,
            ValueType::Array {
                element_type: Box::new(ValueType::Map {
                    key_type: Box::new(ValueType::String),
                    value_type: Box::new(ValueType::I64),
                    values_nullable: true,
                }),
                elements_nullable: false,
            }
        );
    }

    #[test]
    fn struct_with_fields() {
        let yaml = "
struct:
  - { name: lat, type: f64 }
  - { name: lon, type: f64, nullable: true }
";
        let vt = inline(yaml).unwrap();
        match vt {
            ValueType::Struct { fields } => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "lat");
                assert_eq!(fields[0].value_type, ValueType::F64);
                assert!(!fields[0].nullable);
                assert!(fields[1].nullable);
            }
            _ => panic!("expected struct"),
        }
    }

    #[test]
    fn enum_values() {
        assert_eq!(
            inline("{ enum: [red, green, blue] }").unwrap(),
            ValueType::Enum { values: vec!["red".into(), "green".into(), "blue".into()] }
        );
    }

    #[test]
    fn catalog_named_reference() {
        let cat = TypeCatalog::from_yaml(
            "
types:
  GeoPoint:
    struct:
      - { name: lat, type: f64 }
      - { name: lon, type: f64 }
  Place:
    struct:
      - { name: name, type: string }
      - { name: at, type: GeoPoint }
",
        )
        .unwrap();

        let place = cat.get("Place").unwrap();
        match place {
            ValueType::Struct { fields } => {
                assert_eq!(fields[1].name, "at");
                // The reference resolved to the full GeoPoint struct.
                assert_eq!(fields[1].value_type, cat.get("GeoPoint").unwrap().clone());
            }
            _ => panic!("expected struct"),
        }

        // Inline parse can also resolve names against the catalog.
        assert_eq!(parse_type("{ array: { element: GeoPoint } }", &cat).unwrap(), ValueType::Array {
            element_type: Box::new(cat.get("GeoPoint").unwrap().clone()),
            elements_nullable: false,
        });
        assert_eq!(parse_type("{ ref: GeoPoint }", &cat).unwrap(), cat.get("GeoPoint").unwrap().clone());
    }

    #[test]
    fn unknown_reference_errors() {
        let err = inline("Nope").unwrap_err();
        assert!(err.contains("unknown type or reference"), "{err}");
    }

    #[test]
    fn cyclic_reference_errors() {
        let err = TypeCatalog::from_yaml(
            "
types:
  A: { struct: [ { name: b, type: B } ] }
  B: { struct: [ { name: a, type: A } ] }
",
        )
        .unwrap_err();
        assert!(err.contains("cyclic"), "{err}");
    }

    #[test]
    fn round_trip_inline() {
        let vt = ValueType::Struct {
            fields: vec![
                StructField { name: "id".into(), human_name: "id".into(), value_type: ValueType::Uuid, nullable: false },
                StructField {
                    name: "scores".into(),
                    human_name: "scores".into(),
                    value_type: ValueType::Map {
                        key_type: Box::new(ValueType::String),
                        value_type: Box::new(ValueType::Decimal { precision: 9, scale: 2 }),
                        values_nullable: true,
                    },
                    nullable: false,
                },
                StructField {
                    name: "when".into(),
                    human_name: "when".into(),
                    value_type: ValueType::Timestamp {
                        precision: TimestampPrecision::Nanos,
                        timezone: TimestampTimezone::UtcOffset,
                    },
                    nullable: true,
                },
            ],
        };
        let encoded = to_yaml(&vt);
        let back = parse_value_type(&encoded, &TypeCatalog::empty()).unwrap();
        assert_eq!(back, vt);
    }
}
