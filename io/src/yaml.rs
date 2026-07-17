//! YAML <-> [`Value`] round-tripping under a [`ValueType`].
//!
//! The third syntax frontier. Like [`json`](crate::json), parsing is
//! *schema-directed*: [`yaml_to_value`] walks the `ValueType` and the YAML
//! node together and produces a canonical `Value` that conforms by
//! construction. The two frontiers agree by construction, not by parallel
//! maintenance: every scalar leaf delegates to `json`'s canonicalization
//! (the YAML scalar data model — null/bool/number/string — embeds losslessly
//! in JSON's), so the RFC 3339, date, decimal, base64, UUID, and enum rules
//! are literally the same code. Only what YAML can express and JSON cannot
//! is handled natively here:
//!
//! - **Typed mapping keys.** A YAML mapping key may be an unquoted scalar,
//!   so `42: x` parses directly into a `Map` with an integer key type
//!   (JSON requires the quoted `"42"`). Keys must still be scalars, and a
//!   `String`-keyed map requires actual string keys — a bare `42` does not
//!   silently stringify.
//! - **Non-finite floats.** `.inf`, `-.inf`, and `.nan` are valid YAML and
//!   inhabit `F32`/`F64` (JSON has no representation; emitting a non-finite
//!   float to JSON is an error, to YAML it is native).
//! - **Custom YAML tags are rejected** (`!whatever`): every type already
//!   has exactly one canonical scalar form, and an application-defined
//!   respelling is a divergence trap, not a feature. (Core-schema `!!` tags
//!   never reach this module: serde_yaml resolves them during parsing, so
//!   `!!binary aGVsbG8=` arrives as the plain string a `Blob` expects.)
//!
//! [`value_to_yaml`] emits the canonical form: the `json` module's scalar
//! spellings, mappings with natively-typed keys (integers and bools
//! unquoted), and null struct slots omitted — such that
//! `yaml_to_value(value_to_yaml(v)) == v`.

use serde_json::Value as Json;
use serde_yaml::Value as Yaml;

use crate::conformance::Mismatch;
use crate::json::{
    child_path, emit as emit_json, err, format_map_key, index_path, key_path,
    parse as parse_json, parse_map_key,
};
use meta_types::value::{MapKey, Value, ValueType};

fn yaml_kind(yaml: &Yaml) -> &'static str {
    match yaml {
        Yaml::Null => "null",
        Yaml::Bool(_) => "bool",
        Yaml::Number(_) => "number",
        Yaml::String(_) => "string",
        Yaml::Sequence(_) => "sequence",
        Yaml::Mapping(_) => "mapping",
        Yaml::Tagged(_) => "tagged value",
    }
}

// ---------------------------------------------------------------------------
// YAML -> Value
// ---------------------------------------------------------------------------

/// Parse `yaml` into the canonical [`Value`] for `ty`. The root itself is
/// non-nullable (a nullable root belongs to the *container* holding it);
/// `ValueType::Null` is the one type YAML `null` inhabits directly.
pub fn yaml_to_value(yaml: &Yaml, ty: &ValueType) -> Result<Value, Mismatch> {
    parse(yaml, ty, false, "")
}

fn parse(yaml: &Yaml, ty: &ValueType, nullable: bool, path: &str) -> Result<Value, Mismatch> {
    use ValueType as T;

    if matches!(yaml, Yaml::Tagged(_)) {
        return Err(err(path, "YAML tags are not supported"));
    }
    if yaml.is_null() {
        if matches!(ty, T::Null) || nullable {
            return Ok(Value::Null);
        }
        return Err(err(path, "null where the type does not permit it"));
    }

    let expected = |what: &str| err(path, format!("expected {what}, found {}", yaml_kind(yaml)));

    match ty {
        T::Array { element_type, elements_nullable } => {
            let items = yaml.as_sequence().ok_or_else(|| expected("sequence"))?;
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                out.push(parse(item, element_type, *elements_nullable, &index_path(path, i))?);
            }
            Ok(Value::Array(out))
        }
        T::Struct { fields } => {
            let mapping = yaml.as_mapping().ok_or_else(|| expected("mapping"))?;
            let mut out = Vec::with_capacity(fields.len());
            for field in fields {
                let slot = mapping.get(Yaml::String(field.name.clone())).unwrap_or(&Yaml::Null);
                out.push(parse(
                    slot,
                    &field.value_type,
                    field.nullable,
                    &child_path(path, &field.name),
                )?);
            }
            // Unknown keys tolerated: forward compatibility, matching the
            // JSON frontier and `conformance`.
            Ok(Value::Struct(out))
        }
        T::Map { key_type, value_type, values_nullable } => {
            let mapping = yaml.as_mapping().ok_or_else(|| expected("mapping"))?;
            let mut out = std::collections::BTreeMap::new();
            for (k, v) in mapping {
                let key_text = map_key_text(k, key_type, path)?;
                let kp = key_path(path, &key_text);
                let key = parse_map_key(&key_text, key_type)
                    .ok_or_else(|| err(&kp, format!("cannot parse key for {key_type:?}")))?;
                let value = parse(v, value_type, *values_nullable, &kp)?;
                out.insert(key, value);
            }
            Ok(Value::Map(out))
        }
        // A foreign key travels as its key value.
        T::EntityRef { key_type, .. } => parse(yaml, key_type, nullable, path),
        // Native: YAML floats include .inf/-.inf/.nan, which have no JSON
        // form. (Integers into float fields arrive here too, via as_f64.)
        T::F32 => yaml
            .as_f64()
            .map(|v| Value::F32(v as f32))
            .ok_or_else(|| expected("number")),
        T::F64 => yaml.as_f64().map(Value::F64).ok_or_else(|| expected("number")),
        // Every other scalar leaf: the YAML scalar embeds losslessly in the
        // JSON data model, and the canonicalization rules are `json`'s.
        _ => parse_json(&scalar_to_json(yaml, path)?, ty, nullable, path),
    }
}

/// A YAML mapping key as the canonical text `parse_map_key` speaks. Keys
/// must be scalars; a `String`-keyed map requires actual string keys (no
/// silent stringification of `42`), while typed key domains accept their
/// native unquoted scalars and the quoted spelling alike.
fn map_key_text(key: &Yaml, key_type: &ValueType, path: &str) -> Result<String, Mismatch> {
    Ok(match key {
        Yaml::String(s) => s.clone(),
        Yaml::Number(n) if !matches!(key_type, ValueType::String) => n.to_string(),
        Yaml::Bool(b) if matches!(key_type, ValueType::Bool) => b.to_string(),
        other => {
            return Err(err(
                path,
                format!("cannot use a {} key for {key_type:?}", yaml_kind(other)),
            ));
        }
    })
}

/// A YAML scalar into the identical JSON scalar. Containers and tags don't
/// reach here with a well-formed document; they become kind errors.
fn scalar_to_json(yaml: &Yaml, path: &str) -> Result<Json, Mismatch> {
    Ok(match yaml {
        Yaml::Null => Json::Null,
        Yaml::Bool(b) => Json::Bool(*b),
        Yaml::String(s) => Json::String(s.clone()),
        Yaml::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into()
            } else if let Some(u) = n.as_u64() {
                u.into()
            } else {
                let f = n.as_f64().expect("yaml number is i64, u64, or f64");
                serde_json::Number::from_f64(f).map(Json::Number).ok_or_else(|| {
                    err(path, "non-finite number where the type is not a float")
                })?
            }
        }
        other => return Err(err(path, format!("expected a scalar, found {}", yaml_kind(other)))),
    })
}

// ---------------------------------------------------------------------------
// Value -> YAML
// ---------------------------------------------------------------------------

/// Emit the canonical YAML form of `value` under `ty`. Inverse of
/// [`yaml_to_value`]: round-tripping a canonical value is the identity.
pub fn value_to_yaml(value: &Value, ty: &ValueType) -> Result<Yaml, Mismatch> {
    emit(value, ty, "")
}

fn emit(value: &Value, ty: &ValueType, path: &str) -> Result<Yaml, Mismatch> {
    use Value as V;
    use ValueType as T;

    if matches!(value, V::Null) {
        return Ok(Yaml::Null);
    }
    let mismatch =
        |expected: &str| err(path, format!("value/type mismatch: expected {expected}"));

    Ok(match (ty, value) {
        (T::Array { element_type, .. }, V::Array(items)) => Yaml::Sequence(
            items
                .iter()
                .enumerate()
                .map(|(i, item)| emit(item, element_type, &index_path(path, i)))
                .collect::<Result<_, _>>()?,
        ),
        (T::Struct { fields }, V::Struct(slots)) => {
            if fields.len() != slots.len() {
                return Err(err(
                    path,
                    format!("struct has {} slots, type declares {}", slots.len(), fields.len()),
                ));
            }
            let mut mapping = serde_yaml::Mapping::with_capacity(fields.len());
            for (field, slot) in fields.iter().zip(slots) {
                // Canonical YAML omits null slots (round-trips identically:
                // absent parses back to Null).
                if !matches!(slot, V::Null) {
                    mapping.insert(
                        Yaml::String(field.name.clone()),
                        emit(slot, &field.value_type, &child_path(path, &field.name))?,
                    );
                }
            }
            Yaml::Mapping(mapping)
        }
        // Keyed-map form of a struct (accepted by `conformance`) emits too.
        (T::Struct { fields }, V::Map(entries)) => {
            let mut mapping = serde_yaml::Mapping::with_capacity(entries.len());
            for field in fields {
                if let Some(v) = entries.get(&MapKey::String(field.name.clone())) {
                    if !matches!(v, V::Null) {
                        mapping.insert(
                            Yaml::String(field.name.clone()),
                            emit(v, &field.value_type, &child_path(path, &field.name))?,
                        );
                    }
                }
            }
            Yaml::Mapping(mapping)
        }
        (T::Map { value_type, .. }, V::Map(entries)) => {
            let mut mapping = serde_yaml::Mapping::with_capacity(entries.len());
            for (k, v) in entries {
                let key = key_to_yaml(k);
                let value = emit(v, value_type, &key_path(path, &format_map_key(k)))?;
                mapping.insert(key, value);
            }
            Yaml::Mapping(mapping)
        }
        (T::EntityRef { key_type, .. }, v) => emit(v, key_type, path)?,
        // Native floats: non-finite values are representable in YAML.
        (T::F32, V::F32(v)) => Yaml::Number((*v as f64).into()),
        (T::F64, V::F64(v)) => Yaml::Number((*v).into()),
        (T::F32, _) | (T::F64, _) => return Err(mismatch("float")),
        // Every other scalar leaf: `json`'s canonical spelling, verbatim.
        _ => json_scalar_to_yaml(emit_json(value, ty, path)?),
    })
}

/// Integer and bool keys emit as native YAML scalars (unquoted); every other
/// key domain keeps the `json` frontier's canonical string spelling.
fn key_to_yaml(k: &MapKey) -> Yaml {
    match k {
        MapKey::Bool(b) => Yaml::Bool(*b),
        MapKey::I8(v) => Yaml::Number((*v as i64).into()),
        MapKey::I16(v) => Yaml::Number((*v as i64).into()),
        MapKey::I32(v) => Yaml::Number((*v as i64).into()),
        MapKey::I64(v) => Yaml::Number((*v).into()),
        MapKey::U8(v) => Yaml::Number((*v as u64).into()),
        MapKey::U16(v) => Yaml::Number((*v as u64).into()),
        MapKey::U32(v) => Yaml::Number((*v as u64).into()),
        MapKey::U64(v) => Yaml::Number((*v).into()),
        other => Yaml::String(format_map_key(other)),
    }
}

/// The JSON scalar emitted by `json::emit` into the identical YAML scalar.
/// Scalar types never emit JSON containers.
fn json_scalar_to_yaml(json: Json) -> Yaml {
    match json {
        Json::Null => Yaml::Null,
        Json::Bool(b) => Yaml::Bool(b),
        Json::String(s) => Yaml::String(s),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Yaml::Number(i.into())
            } else if let Some(u) = n.as_u64() {
                Yaml::Number(u.into())
            } else {
                Yaml::Number(n.as_f64().expect("json number is i64, u64, or f64").into())
            }
        }
        Json::Array(_) | Json::Object(_) => {
            unreachable!("scalar types never emit JSON containers")
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance::conforms;
    use crate::json::json_to_value;
    use meta_types::value::{StructField, TimestampPrecision, TimestampTimezone};

    fn field(name: &str, value_type: ValueType, nullable: bool) -> StructField {
        StructField {
            name: name.to_string(),
            human_name: name.to_string(),
            value_type,
            nullable,
        }
    }

    fn full_schema() -> ValueType {
        ValueType::Struct {
            fields: vec![
                field("id", ValueType::U64, false),
                field("name", ValueType::String, true),
                field("active", ValueType::Bool, false),
                field("ip", ValueType::Ipv4, false),
                field("uid", ValueType::Uuid, false),
                field("blob", ValueType::Blob, false),
                field("day", ValueType::Date, false),
                field(
                    "when",
                    ValueType::Timestamp {
                        precision: TimestampPrecision::Millis,
                        timezone: TimestampTimezone::UtcOffset,
                    },
                    false,
                ),
                field("price", ValueType::Decimal { precision: 10, scale: 2 }, true),
                field(
                    "kind",
                    ValueType::Enum { values: vec!["click".into(), "view".into()] },
                    false,
                ),
                field(
                    "tags",
                    ValueType::Array {
                        element_type: Box::new(ValueType::String),
                        elements_nullable: false,
                    },
                    true,
                ),
                field(
                    "attrs",
                    ValueType::Map {
                        key_type: Box::new(ValueType::String),
                        value_type: Box::new(ValueType::I64),
                        values_nullable: true,
                    },
                    true,
                ),
            ],
        }
    }

    const SAMPLE_YAML: &str = r#"
        id: 7
        name: alpha
        active: true
        ip: 10.1.2.3
        uid: 67e55044-10b1-426f-9247-bb680e5fe0c8
        blob: aGVsbG8=
        day: 2026-07-06
        when: "2026-07-06T12:34:56.789-07:00"
        price: "19.99"
        kind: view
        tags: [a, b]
        attrs:
          zeta: 26
          alpha: 1
          gap: null
    "#;

    fn sample() -> Yaml {
        serde_yaml::from_str(SAMPLE_YAML).unwrap()
    }

    #[test]
    fn parsed_values_conform_by_construction() {
        let ty = full_schema();
        let value = yaml_to_value(&sample(), &ty).unwrap();
        conforms(&value, &ty).unwrap();
    }

    /// The frontiers agree: the same logical document parsed from YAML and
    /// from JSON produces the identical canonical `Value`.
    #[test]
    fn yaml_and_json_frontiers_agree() {
        let ty = full_schema();
        let from_yaml = yaml_to_value(&sample(), &ty).unwrap();
        let json: serde_json::Value = serde_json::json!({
            "id": 7,
            "name": "alpha",
            "active": true,
            "ip": "10.1.2.3",
            "uid": "67e55044-10b1-426f-9247-bb680e5fe0c8",
            "blob": "aGVsbG8=",
            "day": "2026-07-06",
            "when": "2026-07-06T12:34:56.789-07:00",
            "price": "19.99",
            "kind": "view",
            "tags": ["a", "b"],
            "attrs": {"zeta": 26, "alpha": 1, "gap": null}
        });
        assert_eq!(from_yaml, json_to_value(&json, &ty).unwrap());
    }

    #[test]
    fn roundtrip_is_identity_on_canonical_values() {
        let ty = full_schema();
        let value = yaml_to_value(&sample(), &ty).unwrap();
        let emitted = value_to_yaml(&value, &ty).unwrap();
        let reparsed = yaml_to_value(&emitted, &ty).unwrap();
        assert_eq!(value, reparsed, "value -> yaml -> value must be identity");
        // Canonical emission omits null slots.
        assert!(emitted.as_mapping().unwrap().get(Yaml::from("gap")).is_none());
    }

    /// YAML's unquoted scalars land exactly like JSON's quoted ones: `day`
    /// above is a bare `2026-07-06` (a YAML string), `ip` a bare dotted quad.
    #[test]
    fn unquoted_scalar_spellings_canonicalize() {
        let ty = full_schema();
        let value = yaml_to_value(&sample(), &ty).unwrap();
        let Value::Struct(slots) = &value else { panic!("positional struct") };
        assert_eq!(slots[3], Value::Ipv4(0x0A010203));
        assert_eq!(slots[5], Value::Blob(b"hello".to_vec()));
        assert_eq!(slots[7], Value::TimestampTz(1_783_366_496_789, -420));
        assert_eq!(slots[8], Value::DecimalI64(1999));
        assert_eq!(slots[9], Value::Enum(1));
    }

    /// What JSON cannot say: mapping keys that are native typed scalars.
    #[test]
    fn native_typed_map_keys() {
        let ty = ValueType::Map {
            key_type: Box::new(ValueType::U64),
            value_type: Box::new(ValueType::String),
            values_nullable: false,
        };
        // Unquoted integer keys, plus a quoted one — both spellings parse.
        let yaml: Yaml = serde_yaml::from_str("42: answer\n\"7\": lucky\n").unwrap();
        let v = yaml_to_value(&yaml, &ty).unwrap();
        let Value::Map(entries) = &v else { panic!("map") };
        assert_eq!(entries[&MapKey::U64(42)], Value::String("answer".into()));
        assert_eq!(entries.keys().next(), Some(&MapKey::U64(7)), "numeric sort");

        // Canonical emission uses the native (unquoted) key form.
        let emitted = value_to_yaml(&v, &ty).unwrap();
        let mapping = emitted.as_mapping().unwrap();
        assert!(mapping.contains_key(Yaml::Number(42.into())));
        assert_eq!(yaml_to_value(&emitted, &ty).unwrap(), v, "roundtrip");

        // A string-keyed map does NOT silently stringify a bare 42.
        let string_keyed = ValueType::Map {
            key_type: Box::new(ValueType::String),
            value_type: Box::new(ValueType::String),
            values_nullable: false,
        };
        let err = yaml_to_value(&yaml, &string_keyed).unwrap_err();
        assert!(err.message.contains("number key"), "{err}");
    }

    /// What JSON cannot say: non-finite floats.
    #[test]
    fn non_finite_floats_are_native() {
        let ty = ValueType::Struct {
            fields: vec![field("x", ValueType::F64, false), field("y", ValueType::F32, false)],
        };
        let yaml: Yaml = serde_yaml::from_str("x: .inf\ny: -.inf\n").unwrap();
        let v = yaml_to_value(&yaml, &ty).unwrap();
        let Value::Struct(slots) = &v else { panic!() };
        assert_eq!(slots[0], Value::F64(f64::INFINITY));
        assert_eq!(slots[1], Value::F32(f32::NEG_INFINITY));
        let emitted = value_to_yaml(&v, &ty).unwrap();
        assert_eq!(yaml_to_value(&emitted, &ty).unwrap(), v, "roundtrip");

        // ...but a non-finite number where the type is not a float is still
        // an error, not a coercion.
        let int_ty = ValueType::Struct { fields: vec![field("x", ValueType::I64, false)] };
        let yaml: Yaml = serde_yaml::from_str("x: .inf\n").unwrap();
        assert!(yaml_to_value(&yaml, &int_ty).is_err());
    }

    #[test]
    fn custom_tags_are_rejected_but_core_tags_resolve() {
        let ty = ValueType::Struct { fields: vec![field("blob", ValueType::Blob, false)] };
        // Custom application tags surface as Tagged nodes: rejected.
        let yaml: Yaml = serde_yaml::from_str("blob: !mystery aGVsbG8=\n").unwrap();
        let err = yaml_to_value(&yaml, &ty).unwrap_err();
        assert!(err.message.contains("tags"), "{err}");
        // Core-schema !! tags are resolved by the YAML parser itself and
        // arrive as plain scalars — !!binary content is just the string.
        let yaml: Yaml = serde_yaml::from_str("blob: !!binary aGVsbG8=\n").unwrap();
        let v = yaml_to_value(&yaml, &ty).unwrap();
        assert_eq!(v, Value::Struct(vec![Value::Blob(b"hello".to_vec())]));
    }

    #[test]
    fn strictness_and_paths() {
        let ty = full_schema();
        for (doc, expect_in_path) in [
            ("id: -1", "id"),
            ("id: 7\nkind: purchase", "kind"),
            ("id: 7\nprice: \"1.234\"", "price"),
            ("id: 7\nday: 2026-02-30", "day"),
            ("id: 7\nblob: \"not base64!\"", "blob"),
            ("id: 7\nattrs: {k: not an i64}", "attrs"),
            ("id: 7\ntags: [ok, {no: way}]", "tags[1]"),
        ] {
            // Overlay the mutation on the full valid doc.
            let mut base: serde_yaml::Mapping =
                sample().as_mapping().unwrap().clone();
            let overlay: serde_yaml::Mapping = serde_yaml::from_str(doc).unwrap();
            for (k, v) in overlay {
                base.insert(k, v);
            }
            let err = yaml_to_value(&Yaml::Mapping(base), &ty).unwrap_err();
            assert!(
                err.path.contains(expect_in_path),
                "expected {expect_in_path} in path, got: {err}"
            );
        }
    }

    #[test]
    fn missing_required_and_unknown_keys() {
        let ty = full_schema();
        let err = yaml_to_value(&serde_yaml::from_str("active: true").unwrap(), &ty).unwrap_err();
        assert!(err.path.contains("id"), "{err}");

        // Unknown keys tolerated (forward compatibility).
        let mut doc: serde_yaml::Mapping = sample().as_mapping().unwrap().clone();
        doc.insert(Yaml::from("someday"), Yaml::from("a new field"));
        yaml_to_value(&Yaml::Mapping(doc), &ty).unwrap();
    }
}
