//! Parse a native UDF catalog from YAML into `UdfCatalogEntry` values.
//!
//! The YAML format follows the same conventions as the function catalog:
//!
//! ```yaml
//! - id: ua_parse
//!   cel: ua_parse
//!   description: Parse a User-Agent string into structured fields
//!   params:
//!     - name: user_agent
//!       type: string
//!   return_type:
//!     struct:
//!       - { name: browser, type: string }
//!       - { name: os, type: string }
//!   notes: Backed by the woothee crate.
//! ```

use serde::Deserialize;
use crate::native_fn::{UdfCatalogEntry, UdfParam};
use crate::value::{ValueType, StructField};

#[derive(Debug, Deserialize)]
struct RawEntry {
    id: String,
    cel: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    params: Vec<RawParam>,
    return_type: RawType,
    #[serde(default)]
    notes: String,
}

#[derive(Debug, Deserialize)]
struct RawParam {
    name: String,
    r#type: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawType {
    Simple(String),
    Struct { r#struct: Vec<RawStructField> },
}

#[derive(Debug, Deserialize)]
struct RawStructField {
    name: String,
    #[serde(default)]
    human_name: String,
    r#type: String,
    #[serde(default)]
    nullable: bool,
}

fn parse_simple_type(s: &str) -> ValueType {
    match s {
        "bool" => ValueType::Bool,
        "i8" => ValueType::I8,
        "i16" => ValueType::I16,
        "i32" => ValueType::I32,
        "i64" | "int" => ValueType::I64,
        "u8" => ValueType::U8,
        "u16" => ValueType::U16,
        "u32" => ValueType::U32,
        "u64" | "uint" => ValueType::U64,
        "f32" => ValueType::F32,
        "f64" | "double" => ValueType::F64,
        "string" => ValueType::String,
        "bytes" | "blob" => ValueType::Blob,
        "date" => ValueType::Date,
        "uuid" => ValueType::Uuid,
        "ipv4" => ValueType::Ipv4,
        "ipv6" => ValueType::Ipv6,
        "timestamp" => ValueType::Timestamp {
            precision: crate::value::TimestampPrecision::Millis,
            timezone: crate::value::TimestampTimezone::None,
        },
        _ => ValueType::Null,
    }
}

fn convert_type(raw: &RawType) -> ValueType {
    match raw {
        RawType::Simple(s) => parse_simple_type(s),
        RawType::Struct { r#struct: fields } => ValueType::Struct {
            fields: fields.iter().map(|f| StructField {
                name: f.name.clone(),
                human_name: if f.human_name.is_empty() { f.name.clone() } else { f.human_name.clone() },
                value_type: parse_simple_type(&f.r#type),
                nullable: f.nullable,
            }).collect(),
        },
    }
}

pub fn parse_udf_catalog(yaml: &str) -> Result<Vec<UdfCatalogEntry>, String> {
    let entries: Vec<RawEntry> = serde_yaml::from_str(yaml)
        .map_err(|e| format!("failed to parse UDF catalog: {e}"))?;

    Ok(entries.into_iter().map(|e| UdfCatalogEntry {
        id: e.id,
        cel_name: e.cel,
        description: e.description,
        params: e.params.into_iter().map(|p| UdfParam {
            name: p.name,
            value_type: parse_simple_type(&p.r#type),
        }).collect(),
        return_type: convert_type(&e.return_type),
        notes: e.notes,
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_function() {
        let yaml = r#"
- id: my_fn
  cel: my_fn
  description: A test function
  params:
    - { name: x, type: i64 }
  return_type: bool
  notes: test only
"#;
        let entries = parse_udf_catalog(yaml).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cel_name, "my_fn");
        assert_eq!(entries[0].params[0].value_type, ValueType::I64);
        assert_eq!(entries[0].return_type, ValueType::Bool);
    }

    #[test]
    fn parse_struct_return() {
        let yaml = r#"
- id: parse_thing
  cel: parse_thing
  params:
    - { name: input, type: string }
  return_type:
    struct:
      - { name: foo, type: string }
      - { name: bar, type: i64, nullable: true }
"#;
        let entries = parse_udf_catalog(yaml).unwrap();
        let rt = &entries[0].return_type;
        match rt {
            ValueType::Struct { fields } => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "foo");
                assert_eq!(fields[0].value_type, ValueType::String);
                assert!(!fields[0].nullable);
                assert_eq!(fields[1].name, "bar");
                assert_eq!(fields[1].value_type, ValueType::I64);
                assert!(fields[1].nullable);
            }
            _ => panic!("expected struct"),
        }
    }

    #[test]
    fn parse_multiple_functions() {
        let yaml = r#"
- id: fn_a
  cel: fn_a
  params: []
  return_type: string

- id: fn_b
  cel: fn_b
  params:
    - { name: a, type: i64 }
    - { name: b, type: i64 }
  return_type: i64
"#;
        let entries = parse_udf_catalog(yaml).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].cel_name, "fn_a");
        assert_eq!(entries[1].params.len(), 2);
    }
}
