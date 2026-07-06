//! Parse an external UDF module catalog from YAML.
//!
//! Each module declares a namespace, version, and list of functions:
//!
//! ```yaml
//! namespace: com.example.udf.useragent
//! version: "1"
//! functions:
//!   - id: ua_parse
//!     cel: ua_parse
//!     description: Parse a User-Agent string
//!     params:
//!       - { name: user_agent, type: string }
//!     return_type:
//!       struct:
//!         - { name: browser, type: string }
//!         - { name: os, type: string }
//! ```

use serde::Deserialize;
use crate::external_fn::{UdfModuleMeta, UdfCatalogEntry, UdfParam};
use crate::value::{ValueType, StructField};

#[derive(Debug, Deserialize)]
struct RawModule {
    namespace: String,
    version: String,
    functions: Vec<RawEntry>,
}

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

fn convert_entry(e: RawEntry, namespace: &str, version: &str) -> UdfCatalogEntry {
    UdfCatalogEntry {
        namespace: namespace.to_string(),
        version: version.to_string(),
        id: e.id,
        cel_name: e.cel,
        description: e.description,
        params: e.params.into_iter().map(|p| UdfParam {
            name: p.name,
            value_type: parse_simple_type(&p.r#type),
        }).collect(),
        return_type: convert_type(&e.return_type),
        notes: e.notes,
    }
}

pub fn parse_udf_module(yaml: &str) -> Result<UdfModuleMeta, String> {
    let raw: RawModule = serde_yaml::from_str(yaml)
        .map_err(|e| format!("failed to parse UDF module: {e}"))?;

    let ns = &raw.namespace;
    let ver = &raw.version;
    Ok(UdfModuleMeta {
        namespace: ns.clone(),
        version: ver.clone(),
        functions: raw.functions.into_iter().map(|e| convert_entry(e, ns, ver)).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_module() {
        let yaml = r#"
namespace: com.example.test
version: "1"
functions:
  - id: my_fn
    cel: my_fn
    description: A test function
    params:
      - { name: x, type: i64 }
    return_type: bool
    notes: test only
"#;
        let module = parse_udf_module(yaml).unwrap();
        assert_eq!(module.namespace, "com.example.test");
        assert_eq!(module.version, "1");
        assert_eq!(module.functions.len(), 1);
        assert_eq!(module.functions[0].cel_name, "my_fn");
        assert_eq!(module.functions[0].params[0].value_type, ValueType::I64);
        assert_eq!(module.functions[0].return_type, ValueType::Bool);
    }

    #[test]
    fn parse_struct_return() {
        let yaml = r#"
namespace: com.example.test
version: "2"
functions:
  - id: parse_thing
    cel: parse_thing
    params:
      - { name: input, type: string }
    return_type:
      struct:
        - { name: foo, type: string }
        - { name: bar, type: i64, nullable: true }
"#;
        let module = parse_udf_module(yaml).unwrap();
        assert_eq!(module.version, "2");
        let rt = &module.functions[0].return_type;
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
namespace: com.example.multi
version: "1"
functions:
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
        let module = parse_udf_module(yaml).unwrap();
        assert_eq!(module.functions.len(), 2);
        assert_eq!(module.functions[0].cel_name, "fn_a");
        assert_eq!(module.functions[1].params.len(), 2);
    }

    #[test]
    fn missing_version_fails() {
        let yaml = r#"
namespace: com.example.test
functions:
  - id: my_fn
    cel: my_fn
    params: []
    return_type: bool
"#;
        assert!(parse_udf_module(yaml).is_err());
    }

    #[test]
    fn missing_namespace_fails() {
        let yaml = r#"
version: "1"
functions:
  - id: my_fn
    cel: my_fn
    params: []
    return_type: bool
"#;
        assert!(parse_udf_module(yaml).is_err());
    }
}
