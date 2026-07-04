//! Structural conformance: does a concrete [`Value`] satisfy a [`ValueType`]?
//!
//! This is the cold-path validator used when a value carries arbitrary,
//! externally-supplied structure that must be checked against a declared
//! schema before use -- configuration blobs, deserialized payloads, test
//! fixtures. It is deliberately generic and knows nothing about any
//! particular consumer.
//!
//! Conventions:
//!   - A `ValueType::Struct` is satisfied by EITHER a keyed `Value::Map`
//!     (key = field name as a `String`) OR a positional `Value::Struct`.
//!     The keyed form is the friendlier one for hand-written config.
//!   - Nullability lives in containers (`StructField::nullable`,
//!     `elements_nullable`, `values_nullable`), never in `ValueType`.
//!     A `Value::Null` is accepted only where the container permits it.
//!   - Unknown map keys against a struct schema are tolerated (forward
//!     compatibility); missing non-nullable fields are not.

use crate::value::{MapKey, Value, ValueType};

/// A single point of disagreement between a value and a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// Dotted path from the root value to the offending node, e.g.
    /// `headers["Authorization"]` or `port`. Empty at the root.
    pub path: String,
    /// Human-readable explanation of what was expected vs. found.
    pub message: String,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "at {}: {}", self.path, self.message)
        }
    }
}

/// Returns `Ok(())` if `value` structurally conforms to `ty`, else the first
/// [`Mismatch`] encountered (depth-first, fields in declared order).
pub fn conforms(value: &Value, ty: &ValueType) -> Result<(), Mismatch> {
    check(value, ty, "")
}

fn err(path: &str, message: impl Into<String>) -> Mismatch {
    Mismatch { path: path.to_string(), message: message.into() }
}

fn child_path(path: &str, field: &str) -> String {
    if path.is_empty() {
        field.to_string()
    } else {
        format!("{path}.{field}")
    }
}

fn index_path(path: &str, idx: usize) -> String {
    format!("{path}[{idx}]")
}

fn check(value: &Value, ty: &ValueType, path: &str) -> Result<(), Mismatch> {
    use ValueType as T;
    use Value as V;

    let ok = |b: bool, expected: &str| {
        if b {
            Ok(())
        } else {
            Err(err(path, format!("expected {expected}, found {}", variant_name(value))))
        }
    };

    match ty {
        T::Null => ok(matches!(value, V::Null), "null"),
        T::Bool => ok(matches!(value, V::Bool(_)), "bool"),
        T::I8 => ok(matches!(value, V::I8(_)), "i8"),
        T::I16 => ok(matches!(value, V::I16(_)), "i16"),
        T::I32 => ok(matches!(value, V::I32(_)), "i32"),
        T::I64 => ok(matches!(value, V::I64(_)), "i64"),
        T::U8 => ok(matches!(value, V::U8(_)), "u8"),
        T::U16 => ok(matches!(value, V::U16(_)), "u16"),
        T::U32 => ok(matches!(value, V::U32(_)), "u32"),
        T::U64 => ok(matches!(value, V::U64(_)), "u64"),
        T::F32 => ok(matches!(value, V::F32(_)), "f32"),
        T::F64 => ok(matches!(value, V::F64(_)), "f64"),
        T::Date => ok(matches!(value, V::Date(_)), "date"),
        T::Uuid => ok(matches!(value, V::Uuid(_)), "uuid"),
        T::Ipv4 => ok(matches!(value, V::Ipv4(_)), "ipv4"),
        T::Ipv6 => ok(matches!(value, V::Ipv6(_)), "ipv6"),
        T::Blob => ok(matches!(value, V::Blob(_)), "blob"),
        T::Clob => ok(matches!(value, V::Clob(_)), "clob"),
        T::String => ok(matches!(value, V::String(_)), "string"),

        T::Decimal { .. } => {
            ok(matches!(value, V::DecimalI64(_) | V::DecimalI128(_)), "decimal")
        }
        T::Timestamp { .. } => {
            ok(matches!(value, V::Timestamp(_) | V::TimestampTz(_, _)), "timestamp")
        }

        T::Enum { values } => match value {
            // Accept either the symbolic name or the ordinal.
            V::String(s) if values.iter().any(|v| v == s) => Ok(()),
            V::Enum(i) if (*i as usize) < values.len() => Ok(()),
            V::String(s) => Err(err(
                path,
                format!("'{s}' is not one of enum {{{}}}", values.join(", ")),
            )),
            V::Enum(i) => {
                Err(err(path, format!("enum ordinal {i} out of range 0..{}", values.len())))
            }
            _ => Err(err(path, format!("expected enum, found {}", variant_name(value)))),
        },

        T::Array { element_type, elements_nullable } => match value {
            V::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    let p = index_path(path, i);
                    if matches!(item, V::Null) {
                        if !elements_nullable {
                            return Err(err(&p, "null element not permitted"));
                        }
                    } else {
                        check(item, element_type, &p)?;
                    }
                }
                Ok(())
            }
            _ => Err(err(path, format!("expected array, found {}", variant_name(value)))),
        },

        T::Map { key_type, value_type, values_nullable } => match value {
            V::Map(m) => {
                for (k, v) in m {
                    let p = format!("{path}[{}]", map_key_display(k));
                    check_map_key(k, key_type, &p)?;
                    if matches!(v, V::Null) {
                        if !values_nullable {
                            return Err(err(&p, "null value not permitted"));
                        }
                    } else {
                        check(v, value_type, &p)?;
                    }
                }
                Ok(())
            }
            _ => Err(err(path, format!("expected map, found {}", variant_name(value)))),
        },

        T::Struct { fields } => match value {
            // Keyed form: Map<String, _> indexed by field name.
            V::Map(m) => {
                for f in fields {
                    let p = child_path(path, &f.name);
                    match m.get(&MapKey::String(f.name.clone())) {
                        None => {
                            if !f.nullable {
                                return Err(err(&p, "missing required field"));
                            }
                        }
                        Some(V::Null) => {
                            if !f.nullable {
                                return Err(err(&p, "null not permitted for required field"));
                            }
                        }
                        Some(v) => check(v, &f.value_type, &p)?,
                    }
                }
                Ok(())
            }
            // Positional form: Struct(Vec<Value>) aligned to declared fields.
            V::Struct(vals) => {
                if vals.len() != fields.len() {
                    return Err(err(
                        path,
                        format!("expected {} struct fields, found {}", fields.len(), vals.len()),
                    ));
                }
                for (f, v) in fields.iter().zip(vals) {
                    let p = child_path(path, &f.name);
                    if matches!(v, V::Null) {
                        if !f.nullable {
                            return Err(err(&p, "null not permitted for required field"));
                        }
                    } else {
                        check(v, &f.value_type, &p)?;
                    }
                }
                Ok(())
            }
            _ => Err(err(path, format!("expected struct, found {}", variant_name(value)))),
        },

        // A foreign key is checked at the level of its key value.
        T::EntityRef { key_type, .. } => check(value, key_type, path),
    }
}

fn check_map_key(key: &MapKey, key_type: &ValueType, path: &str) -> Result<(), Mismatch> {
    use MapKey as K;
    use ValueType as T;
    let ok = matches!(
        (key, key_type),
        (K::Bool(_), T::Bool)
            | (K::I8(_), T::I8)
            | (K::I16(_), T::I16)
            | (K::I32(_), T::I32)
            | (K::I64(_), T::I64)
            | (K::U8(_), T::U8)
            | (K::U16(_), T::U16)
            | (K::U32(_), T::U32)
            | (K::U64(_), T::U64)
            | (K::Date(_), T::Date)
            | (K::Uuid(_), T::Uuid)
            | (K::Ipv4(_), T::Ipv4)
            | (K::Ipv6(_), T::Ipv6)
            | (K::String(_), T::String)
            | (K::Timestamp(_), T::Timestamp { .. })
            | (K::TimestampTz(_, _), T::Timestamp { .. })
    );
    if ok {
        Ok(())
    } else {
        Err(err(path, format!("map key {} does not match key type", map_key_display(key))))
    }
}

fn map_key_display(k: &MapKey) -> String {
    match k {
        MapKey::String(s) => format!("\"{s}\""),
        MapKey::Bool(b) => b.to_string(),
        MapKey::I8(v) => v.to_string(),
        MapKey::I16(v) => v.to_string(),
        MapKey::I32(v) => v.to_string(),
        MapKey::I64(v) => v.to_string(),
        MapKey::U8(v) => v.to_string(),
        MapKey::U16(v) => v.to_string(),
        MapKey::U32(v) => v.to_string(),
        MapKey::U64(v) => v.to_string(),
        MapKey::Date(v) => v.to_string(),
        MapKey::Uuid(v) => v.to_string(),
        MapKey::Ipv4(v) => v.to_string(),
        MapKey::Ipv6(v) => v.to_string(),
        MapKey::Timestamp(v) => v.to_string(),
        MapKey::TimestampTz(v, o) => format!("{v}+{o}"),
    }
}

fn variant_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::I8(_) => "i8",
        Value::I16(_) => "i16",
        Value::I32(_) => "i32",
        Value::I64(_) => "i64",
        Value::U8(_) => "u8",
        Value::U16(_) => "u16",
        Value::U32(_) => "u32",
        Value::U64(_) => "u64",
        Value::F32(_) => "f32",
        Value::F64(_) => "f64",
        Value::Date(_) => "date",
        Value::Uuid(_) => "uuid",
        Value::Ipv4(_) => "ipv4",
        Value::Ipv6(_) => "ipv6",
        Value::Blob(_) => "blob",
        Value::Clob(_) => "clob",
        Value::String(_) => "string",
        Value::DecimalI64(_) | Value::DecimalI128(_) => "decimal",
        Value::Timestamp(_) | Value::TimestampTz(_, _) => "timestamp",
        Value::Enum(_) => "enum",
        Value::Array(_) => "array",
        Value::Map(_) => "map",
        Value::Struct(_) => "struct",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::StructField;
    use std::collections::BTreeMap;

    fn field(name: &str, value_type: ValueType, nullable: bool) -> StructField {
        StructField {
            name: name.to_string(),
            human_name: name.to_string(),
            value_type,
            nullable,
        }
    }

    fn map(entries: Vec<(&str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(k, v)| (MapKey::String(k.to_string()), v))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn schema() -> ValueType {
        ValueType::Struct {
            fields: vec![
                field("host", ValueType::String, false),
                field("port", ValueType::U16, true),
            ],
        }
    }

    #[test]
    fn keyed_struct_ok() {
        let v = map(vec![("host", Value::String("db".into())), ("port", Value::U16(5432))]);
        assert!(conforms(&v, &schema()).is_ok());
    }

    #[test]
    fn nullable_field_may_be_absent() {
        let v = map(vec![("host", Value::String("db".into()))]);
        assert!(conforms(&v, &schema()).is_ok());
    }

    #[test]
    fn missing_required_field_fails() {
        let v = map(vec![("port", Value::U16(5432))]);
        let e = conforms(&v, &schema()).unwrap_err();
        assert_eq!(e.path, "host");
    }

    #[test]
    fn wrong_scalar_type_fails() {
        let v = map(vec![("host", Value::I64(7))]);
        let e = conforms(&v, &schema()).unwrap_err();
        assert_eq!(e.path, "host");
        assert!(e.message.contains("string"));
    }

    #[test]
    fn unknown_keys_tolerated() {
        let v = map(vec![("host", Value::String("db".into())), ("extra", Value::Bool(true))]);
        assert!(conforms(&v, &schema()).is_ok());
    }

    #[test]
    fn enum_accepts_name_and_ordinal() {
        let ty = ValueType::Enum { values: vec!["postgres".into(), "mysql".into()] };
        assert!(conforms(&Value::String("mysql".into()), &ty).is_ok());
        assert!(conforms(&Value::Enum(1), &ty).is_ok());
        assert!(conforms(&Value::String("oracle".into()), &ty).is_err());
        assert!(conforms(&Value::Enum(2), &ty).is_err());
    }

    #[test]
    fn nested_map_values_checked() {
        let ty = ValueType::Map {
            key_type: Box::new(ValueType::String),
            value_type: Box::new(ValueType::String),
            values_nullable: false,
        };
        let good = map(vec![("a", Value::String("x".into()))]);
        assert!(conforms(&good, &ty).is_ok());
        let bad = map(vec![("a", Value::I64(1))]);
        let e = conforms(&bad, &ty).unwrap_err();
        assert_eq!(e.path, "[\"a\"]");
    }
}
