//! JSON <-> [`Value`] round-tripping under a [`ValueType`].
//!
//! JSON is the second syntax frontier after proto: where the proto path is
//! decode-then-[`conformance`](crate::conformance), JSON parsing here is
//! *schema-directed* -- [`json_to_value`] walks the `ValueType` and the JSON
//! together and produces a canonical `Value` that conforms by construction
//! (property-tested below). Everything above syntax -- the
//! [`validation`](crate::validation) tier -- then applies to JSON-ingested
//! values exactly as it does to proto-ingested ones.
//!
//! Canonicalization rules (strict; no silent coercion):
//!
//! - JSON `null` / absent field are accepted only where the containing type
//!   permits null (`StructField::nullable`, `elements_nullable`,
//!   `values_nullable`).
//! - Integers must be integral and in range; unsigned rejects negatives.
//! - `Date` is `"YYYY-MM-DD"`. `Timestamp` accepts an integer already in
//!   the declared precision unit, or an RFC 3339 string; fraction digits
//!   beyond the precision must be zero. With `TimestampTimezone::UtcOffset`
//!   the offset is preserved (`Value::TimestampTz`); with `None` it is
//!   normalized away (`Value::Timestamp` holds the UTC instant).
//! - `Decimal` accepts exact strings (`"19.99"`), integers, and floats
//!   within 4 ULPs of the decimal they round to (float-arithmetic artifacts
//!   pass; anything that would genuinely round is rejected). Storage is
//!   `DecimalI64` for precision <= 18, else `DecimalI128`.
//! - `Uuid`/`Ipv4`/`Ipv6` are canonical strings; `Blob` is standard base64;
//!   `Enum` strings must be declared members and canonicalize to their
//!   ordinal.
//! - `Struct` parses from a JSON object into the *positional*
//!   `Value::Struct` form; unknown keys are tolerated (forward
//!   compatibility, matching `conformance`).
//! - `Map` parses from a JSON object; keys are parsed from the object's
//!   string keys per the declared key type (string-representable key types
//!   only), values per the value type. `BTreeMap` gives canonical sorted
//!   entry order.
//!
//! [`value_to_json`] emits the canonical JSON form (base64 blobs, hyphenated
//! UUIDs, `"YYYY-MM-DD"` dates, integer timestamps -- RFC 3339 for
//! `TimestampTz`, decimal strings, enum symbols), such that
//! `json_to_value(value_to_json(v)) == v`.

use std::collections::BTreeMap;

use serde_json::Value as Json;

use crate::conformance::Mismatch;
use crate::value::{MapKey, TimestampPrecision, TimestampTimezone, Value, ValueType};

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

fn key_path(path: &str, key: &str) -> String {
    format!("{path}[{key:?}]")
}

fn json_kind(json: &Json) -> &'static str {
    match json {
        Json::Null => "null",
        Json::Bool(_) => "bool",
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}

// ---------------------------------------------------------------------------
// JSON -> Value
// ---------------------------------------------------------------------------

/// Parse `json` into the canonical [`Value`] for `ty`. The root itself is
/// non-nullable (a nullable root belongs to the *container* holding it);
/// `ValueType::Null` is the one type JSON `null` inhabits directly.
pub fn json_to_value(json: &Json, ty: &ValueType) -> Result<Value, Mismatch> {
    parse(json, ty, false, "")
}

fn parse(json: &Json, ty: &ValueType, nullable: bool, path: &str) -> Result<Value, Mismatch> {
    use ValueType as T;

    if json.is_null() {
        if matches!(ty, T::Null) || nullable {
            return Ok(Value::Null);
        }
        return Err(err(path, "null where the type does not permit it"));
    }

    let expected = |what: &str| err(path, format!("expected {what}, found {}", json_kind(json)));

    match ty {
        T::Null => Err(expected("null")),
        T::Bool => json.as_bool().map(Value::Bool).ok_or_else(|| expected("bool")),
        T::I8 => parse_signed(json, path, "i8", Value::I8),
        T::I16 => parse_signed(json, path, "i16", Value::I16),
        T::I32 => parse_signed(json, path, "i32", Value::I32),
        T::I64 => parse_signed(json, path, "i64", Value::I64),
        T::U8 => parse_unsigned(json, path, "u8", Value::U8),
        T::U16 => parse_unsigned(json, path, "u16", Value::U16),
        T::U32 => parse_unsigned(json, path, "u32", Value::U32),
        T::U64 => parse_unsigned(json, path, "u64", Value::U64),
        T::F32 => json
            .as_f64()
            .map(|v| Value::F32(v as f32))
            .ok_or_else(|| expected("number")),
        T::F64 => json.as_f64().map(Value::F64).ok_or_else(|| expected("number")),
        T::String => json
            .as_str()
            .map(|s| Value::String(s.to_string()))
            .ok_or_else(|| expected("string")),
        T::Clob => json
            .as_str()
            .map(|s| Value::Clob(s.as_bytes().to_vec()))
            .ok_or_else(|| expected("string")),
        T::Blob => {
            let s = json.as_str().ok_or_else(|| expected("base64 string"))?;
            base64_decode(s)
                .map(Value::Blob)
                .ok_or_else(|| err(path, "invalid base64"))
        }
        T::Uuid => {
            let s = json.as_str().ok_or_else(|| expected("UUID string"))?;
            parse_uuid(s).map(Value::Uuid).ok_or_else(|| err(path, "invalid UUID"))
        }
        T::Ipv4 => {
            let s = json.as_str().ok_or_else(|| expected("IPv4 string"))?;
            s.parse::<std::net::Ipv4Addr>()
                .map(|a| Value::Ipv4(u32::from(a)))
                .map_err(|e| err(path, format!("invalid IPv4: {e}")))
        }
        T::Ipv6 => {
            let s = json.as_str().ok_or_else(|| expected("IPv6 string"))?;
            s.parse::<std::net::Ipv6Addr>()
                .map(|a| Value::Ipv6(u128::from(a)))
                .map_err(|e| err(path, format!("invalid IPv6: {e}")))
        }
        T::Date => {
            let s = json.as_str().ok_or_else(|| expected("YYYY-MM-DD string"))?;
            parse_date(s).map(Value::Date).ok_or_else(|| err(path, "invalid date"))
        }
        T::Timestamp { precision, timezone } => {
            parse_timestamp(json, *precision, *timezone, path)
        }
        T::Decimal { precision, scale } => parse_decimal(json, *precision, *scale, path),
        T::Enum { values } => {
            let s = json.as_str().ok_or_else(|| expected("enum symbol string"))?;
            values
                .iter()
                .position(|v| v == s)
                .map(|i| Value::Enum(i as u32))
                .ok_or_else(|| err(path, format!("{s:?} is not a member of the enum")))
        }
        T::Array { element_type, elements_nullable } => {
            let items = json.as_array().ok_or_else(|| expected("array"))?;
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                out.push(parse(item, element_type, *elements_nullable, &index_path(path, i))?);
            }
            Ok(Value::Array(out))
        }
        T::Struct { fields } => {
            let object = json.as_object().ok_or_else(|| expected("object"))?;
            let mut out = Vec::with_capacity(fields.len());
            for field in fields {
                let slot = object.get(&field.name).unwrap_or(&Json::Null);
                out.push(parse(
                    slot,
                    &field.value_type,
                    field.nullable,
                    &child_path(path, &field.name),
                )?);
            }
            // Unknown keys tolerated: forward compatibility, matching
            // `conformance`'s convention.
            Ok(Value::Struct(out))
        }
        T::Map { key_type, value_type, values_nullable } => {
            let object = json.as_object().ok_or_else(|| expected("object (map)"))?;
            let mut out = BTreeMap::new();
            for (k, v) in object {
                let kp = key_path(path, k);
                let key = parse_map_key(k, key_type)
                    .ok_or_else(|| err(&kp, format!("cannot parse key for {key_type:?}")))?;
                let value = parse(v, value_type, *values_nullable, &kp)?;
                out.insert(key, value);
            }
            Ok(Value::Map(out))
        }
        // A foreign key travels as its key value.
        T::EntityRef { key_type, .. } => parse(json, key_type, nullable, path),
    }
}

fn parse_signed<T: TryFrom<i64>>(
    json: &Json,
    path: &str,
    name: &str,
    wrap: impl Fn(T) -> Value,
) -> Result<Value, Mismatch> {
    let raw = json
        .as_i64()
        .ok_or_else(|| err(path, format!("expected integer ({name}), found {}", json_kind(json))))?;
    T::try_from(raw).map(wrap).map_err(|_| err(path, format!("{raw} out of range for {name}")))
}

fn parse_unsigned<T: TryFrom<u64>>(
    json: &Json,
    path: &str,
    name: &str,
    wrap: impl Fn(T) -> Value,
) -> Result<Value, Mismatch> {
    let raw = json.as_u64().ok_or_else(|| {
        err(path, format!("expected non-negative integer ({name}), found {}", json_kind(json)))
    })?;
    T::try_from(raw).map(wrap).map_err(|_| err(path, format!("{raw} out of range for {name}")))
}

/// String-representable map keys: parsed from the JSON object's string key.
fn parse_map_key(k: &str, key_type: &ValueType) -> Option<MapKey> {
    use ValueType as T;
    Some(match key_type {
        T::String => MapKey::String(k.to_string()),
        T::Bool => MapKey::Bool(k.parse().ok()?),
        T::I8 => MapKey::I8(k.parse().ok()?),
        T::I16 => MapKey::I16(k.parse().ok()?),
        T::I32 => MapKey::I32(k.parse().ok()?),
        T::I64 => MapKey::I64(k.parse().ok()?),
        T::U8 => MapKey::U8(k.parse().ok()?),
        T::U16 => MapKey::U16(k.parse().ok()?),
        T::U32 => MapKey::U32(k.parse().ok()?),
        T::U64 => MapKey::U64(k.parse().ok()?),
        T::Uuid => MapKey::Uuid(parse_uuid(k)?),
        T::Date => MapKey::Date(parse_date(k)?),
        T::Ipv4 => MapKey::Ipv4(u32::from(k.parse::<std::net::Ipv4Addr>().ok()?)),
        T::Ipv6 => MapKey::Ipv6(u128::from(k.parse::<std::net::Ipv6Addr>().ok()?)),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Value -> JSON
// ---------------------------------------------------------------------------

/// Emit the canonical JSON form of `value` under `ty`. Inverse of
/// [`json_to_value`]: round-tripping a canonical value is the identity.
pub fn value_to_json(value: &Value, ty: &ValueType) -> Result<Json, Mismatch> {
    emit(value, ty, "")
}

fn emit(value: &Value, ty: &ValueType, path: &str) -> Result<Json, Mismatch> {
    use Value as V;
    use ValueType as T;

    if matches!(value, V::Null) {
        return Ok(Json::Null);
    }
    let mismatch =
        |expected: &str| err(path, format!("value/type mismatch: expected {expected}"));

    Ok(match (ty, value) {
        (T::Bool, V::Bool(b)) => Json::Bool(*b),
        (T::I8, V::I8(v)) => (*v).into(),
        (T::I16, V::I16(v)) => (*v).into(),
        (T::I32, V::I32(v)) => (*v).into(),
        (T::I64, V::I64(v)) => (*v).into(),
        (T::U8, V::U8(v)) => (*v).into(),
        (T::U16, V::U16(v)) => (*v).into(),
        (T::U32, V::U32(v)) => (*v).into(),
        (T::U64, V::U64(v)) => (*v).into(),
        (T::F32, V::F32(v)) => serde_json::Number::from_f64(*v as f64)
            .map(Json::Number)
            .ok_or_else(|| err(path, "non-finite f32 has no JSON form"))?,
        (T::F64, V::F64(v)) => serde_json::Number::from_f64(*v)
            .map(Json::Number)
            .ok_or_else(|| err(path, "non-finite f64 has no JSON form"))?,
        (T::String, V::String(s)) => Json::String(s.clone()),
        (T::Clob, V::Clob(bytes)) => Json::String(
            String::from_utf8(bytes.clone())
                .map_err(|_| err(path, "clob is not valid UTF-8"))?,
        ),
        (T::Blob, V::Blob(bytes)) => Json::String(base64_encode(bytes)),
        (T::Uuid, V::Uuid(v)) => Json::String(format_uuid(*v)),
        (T::Ipv4, V::Ipv4(v)) => Json::String(std::net::Ipv4Addr::from(*v).to_string()),
        (T::Ipv6, V::Ipv6(v)) => Json::String(std::net::Ipv6Addr::from(*v).to_string()),
        (T::Date, V::Date(days)) => Json::String(format_date(*days)),
        (T::Timestamp { .. }, V::Timestamp(v)) => (*v).into(),
        (T::Timestamp { precision, .. }, V::TimestampTz(v, offset_minutes)) => {
            Json::String(format_rfc3339(*v, *offset_minutes, *precision))
        }
        (T::Decimal { scale, .. }, V::DecimalI64(v)) => {
            Json::String(format_decimal(*v as i128, *scale))
        }
        (T::Decimal { scale, .. }, V::DecimalI128(v)) => {
            Json::String(format_decimal(*v, *scale))
        }
        (T::Enum { values }, V::Enum(ordinal)) => Json::String(
            values
                .get(*ordinal as usize)
                .ok_or_else(|| err(path, format!("enum ordinal {ordinal} out of range")))?
                .clone(),
        ),
        (T::Array { element_type, .. }, V::Array(items)) => Json::Array(
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
            let mut object = serde_json::Map::with_capacity(fields.len());
            for (field, slot) in fields.iter().zip(slots) {
                // Canonical JSON omits null slots (round-trips identically:
                // absent parses back to Null).
                if !matches!(slot, V::Null) {
                    object.insert(
                        field.name.clone(),
                        emit(slot, &field.value_type, &child_path(path, &field.name))?,
                    );
                }
            }
            Json::Object(object)
        }
        // Keyed-map form of a struct (accepted by `conformance`) emits too.
        (T::Struct { fields }, V::Map(entries)) => {
            let mut object = serde_json::Map::with_capacity(entries.len());
            for field in fields {
                if let Some(v) = entries.get(&MapKey::String(field.name.clone())) {
                    if !matches!(v, V::Null) {
                        object.insert(
                            field.name.clone(),
                            emit(v, &field.value_type, &child_path(path, &field.name))?,
                        );
                    }
                }
            }
            Json::Object(object)
        }
        (T::Map { value_type, .. }, V::Map(entries)) => {
            let mut object = serde_json::Map::with_capacity(entries.len());
            for (k, v) in entries {
                let key = format_map_key(k);
                let value = emit(v, value_type, &key_path(path, &key))?;
                object.insert(key, value);
            }
            Json::Object(object)
        }
        (T::EntityRef { key_type, .. }, v) => emit(v, key_type, path)?,
        (T::Null, _) => return Err(mismatch("null")),
        _ => return Err(mismatch(&format!("{ty:?}"))),
    })
}

fn format_map_key(k: &MapKey) -> String {
    match k {
        MapKey::String(s) => s.clone(),
        MapKey::Bool(b) => b.to_string(),
        MapKey::I8(v) => v.to_string(),
        MapKey::I16(v) => v.to_string(),
        MapKey::I32(v) => v.to_string(),
        MapKey::I64(v) => v.to_string(),
        MapKey::U8(v) => v.to_string(),
        MapKey::U16(v) => v.to_string(),
        MapKey::U32(v) => v.to_string(),
        MapKey::U64(v) => v.to_string(),
        MapKey::Uuid(v) => format_uuid(*v),
        MapKey::Date(days) => format_date(*days),
        MapKey::Ipv4(v) => std::net::Ipv4Addr::from(*v).to_string(),
        MapKey::Ipv6(v) => std::net::Ipv6Addr::from(*v).to_string(),
        MapKey::Timestamp(v) => v.to_string(),
        MapKey::TimestampTz(v, offset) => format!("{v}@{offset}"),
    }
}

// ---------------------------------------------------------------------------
// Timestamps (RFC 3339) and civil dates -- dependency-free
// ---------------------------------------------------------------------------

fn unit_per_second(precision: TimestampPrecision) -> i128 {
    match precision {
        TimestampPrecision::Seconds => 1,
        TimestampPrecision::Millis => 1_000,
        // Unspecified canonicalizes to micros.
        TimestampPrecision::Unspecified | TimestampPrecision::Micros => 1_000_000,
        TimestampPrecision::Nanos => 1_000_000_000,
    }
}

fn fraction_digits(precision: TimestampPrecision) -> usize {
    match precision {
        TimestampPrecision::Seconds => 0,
        TimestampPrecision::Millis => 3,
        TimestampPrecision::Unspecified | TimestampPrecision::Micros => 6,
        TimestampPrecision::Nanos => 9,
    }
}

fn parse_timestamp(
    json: &Json,
    precision: TimestampPrecision,
    timezone: TimestampTimezone,
    path: &str,
) -> Result<Value, Mismatch> {
    match json {
        // Integer: already in the declared precision unit, UTC.
        Json::Number(_) => {
            let v = json
                .as_i64()
                .ok_or_else(|| err(path, "timestamp integer must fit i64"))?;
            Ok(match timezone {
                TimestampTimezone::None => Value::Timestamp(v),
                TimestampTimezone::UtcOffset => Value::TimestampTz(v, 0),
            })
        }
        Json::String(s) => {
            let (instant, offset_minutes) = parse_rfc3339(s, precision)
                .ok_or_else(|| err(path, format!("invalid RFC 3339 timestamp {s:?}")))?;
            Ok(match timezone {
                // Offset normalized away; the instant is UTC.
                TimestampTimezone::None => Value::Timestamp(instant),
                TimestampTimezone::UtcOffset => Value::TimestampTz(instant, offset_minutes),
            })
        }
        other => Err(err(
            path,
            format!("expected RFC 3339 string or integer, found {}", json_kind(other)),
        )),
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.frac][Z|±HH:MM]` into (instant-in-precision-
/// units since epoch UTC, offset minutes). Fraction digits beyond the
/// precision must be zero (no silent rounding).
fn parse_rfc3339(s: &str, precision: TimestampPrecision) -> Option<(i64, i16)> {
    let b = s.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let date = parse_date(std::str::from_utf8(&b[..10]).ok()?)?;
    if b[10] != b'T' && b[10] != b't' && b[10] != b' ' {
        return None;
    }
    let time = &b[11..];
    if time.len() < 8 || time[2] != b':' || time[5] != b':' {
        return None;
    }
    let hh: i64 = std::str::from_utf8(&time[0..2]).ok()?.parse().ok()?;
    let mm: i64 = std::str::from_utf8(&time[3..5]).ok()?.parse().ok()?;
    let ss: i64 = std::str::from_utf8(&time[6..8]).ok()?.parse().ok()?;
    if hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let mut rest = &time[8..];

    // Optional fraction.
    let mut frac: i128 = 0;
    if rest.first() == Some(&b'.') {
        rest = &rest[1..];
        let digits_len = rest.iter().take_while(|c| c.is_ascii_digit()).count();
        if digits_len == 0 {
            return None;
        }
        let allowed = fraction_digits(precision);
        let mut scale_remaining = allowed;
        for (i, c) in rest[..digits_len].iter().enumerate() {
            let d = (c - b'0') as i128;
            if i < allowed {
                frac = frac * 10 + d;
                scale_remaining -= 1;
            } else if d != 0 {
                return None; // sub-precision digits must be zero
            }
        }
        frac *= 10i128.pow(scale_remaining as u32);
        rest = &rest[digits_len..];
    }

    // Offset.
    let offset_minutes: i16 = match rest.first()? {
        b'Z' | b'z' if rest.len() == 1 => 0,
        b'+' | b'-' if rest.len() == 6 && rest[3] == b':' => {
            let oh: i16 = std::str::from_utf8(&rest[1..3]).ok()?.parse().ok()?;
            let om: i16 = std::str::from_utf8(&rest[4..6]).ok()?.parse().ok()?;
            if oh > 23 || om > 59 {
                return None;
            }
            let total = oh * 60 + om;
            if rest[0] == b'-' { -total } else { total }
        }
        _ => return None,
    };

    let unit = unit_per_second(precision);
    let secs = (date as i128) * 86_400 + hh as i128 * 3_600 + mm as i128 * 60 + ss as i128
        - offset_minutes as i128 * 60;
    let instant = secs * unit + frac;
    i64::try_from(instant).ok().map(|v| (v, offset_minutes))
}

fn format_rfc3339(instant: i64, offset_minutes: i16, precision: TimestampPrecision) -> String {
    let unit = unit_per_second(precision);
    let local: i128 = instant as i128 + offset_minutes as i128 * 60 * unit;
    let secs = local.div_euclid(unit);
    let frac = local.rem_euclid(unit);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days as i64);
    let (hh, mm, ss) = (tod / 3_600, (tod % 3_600) / 60, tod % 60);
    let mut out = format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}");
    let digits = fraction_digits(precision);
    if digits > 0 {
        out.push('.');
        out.push_str(&format!("{frac:0width$}", width = digits));
    }
    if offset_minutes == 0 {
        out.push('Z');
    } else {
        let sign = if offset_minutes < 0 { '-' } else { '+' };
        let abs = offset_minutes.unsigned_abs();
        out.push_str(&format!("{sign}{:02}:{:02}", abs / 60, abs % 60));
    }
    out
}

/// `"YYYY-MM-DD"` -> days since 1970-01-01 (proleptic Gregorian).
fn parse_date(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = s[0..4].parse().ok()?;
    let m: u32 = s[5..7].parse().ok()?;
    let d: u32 = s[8..10].parse().ok()?;
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d) as i32)
}

fn format_date(days: i32) -> String {
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

// Howard Hinnant's civil-date algorithms.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = ((m as i64) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + (d as i64) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// Decimals
// ---------------------------------------------------------------------------

/// Floats are accepted within this many ULPs of the decimal they round to:
/// short float-arithmetic chains perturb a value by a few units-in-the-last-
/// place (the `0.30000000000000004` family), while genuinely different
/// values sit astronomically further away.
const FLOAT_DECIMAL_ULP_TOLERANCE: u64 = 4;

fn parse_decimal(json: &Json, precision: u32, scale: u32, path: &str) -> Result<Value, Mismatch> {
    let factor = 10i128.pow(scale);
    let scaled: i128 = match json {
        Json::String(s) => parse_decimal_str(s, scale)
            .ok_or_else(|| err(path, format!("{s:?} is not a valid decimal(scale={scale})")))?,
        Json::Number(n) if n.is_f64() => {
            let v = n.as_f64().expect("is_f64");
            if !v.is_finite() {
                return Err(err(path, "non-finite number"));
            }
            let scaled_f = (v * factor as f64).round();
            if scaled_f.abs() >= 1.7e38 {
                return Err(err(path, format!("{v} out of decimal range")));
            }
            let scaled = scaled_f as i128;
            let nearest = (scaled as f64) / factor as f64;
            if ulps_apart(v, nearest) > FLOAT_DECIMAL_ULP_TOLERANCE {
                return Err(err(
                    path,
                    format!(
                        "{v} does not round cleanly to a decimal(scale={scale}); \
                         send decimals as strings for exactness"
                    ),
                ));
            }
            scaled
        }
        Json::Number(n) => {
            let raw: i128 = if let Some(i) = n.as_i64() {
                i as i128
            } else if let Some(u) = n.as_u64() {
                u as i128
            } else {
                return Err(err(path, "unsupported number form"));
            };
            raw.checked_mul(factor)
                .ok_or_else(|| err(path, format!("{raw} out of decimal range")))?
        }
        other => {
            return Err(err(
                path,
                format!("expected decimal string or number, found {}", json_kind(other)),
            ));
        }
    };
    if precision < 39 && scaled.abs() >= 10i128.pow(precision.min(38)) {
        return Err(err(
            path,
            format!("value has more than {precision} total digits at scale {scale}"),
        ));
    }
    // i64 comfortably holds 18 digits; wider decimals take the i128 form.
    Ok(if precision <= 18 {
        Value::DecimalI64(scaled as i64)
    } else {
        Value::DecimalI128(scaled)
    })
}

fn parse_decimal_str(s: &str, scale: u32) -> Option<i128> {
    let (negative, rest) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int_part, frac_part) = match rest.split_once('.') {
        Some((i, f)) => (i, f),
        None => (rest, ""),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    if !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let (frac_in_scale, frac_beyond) = frac_part.split_at(frac_part.len().min(scale as usize));
    if frac_beyond.bytes().any(|b| b != b'0') {
        return None; // no silent rounding
    }
    let mut acc: i128 = 0;
    for b in int_part.bytes().chain(frac_in_scale.bytes()) {
        acc = acc.checked_mul(10)?.checked_add((b - b'0') as i128)?;
    }
    acc = acc.checked_mul(10i128.checked_pow(scale - frac_in_scale.len() as u32)?)?;
    Some(if negative { -acc } else { acc })
}

fn format_decimal(scaled: i128, scale: u32) -> String {
    let negative = scaled < 0;
    let abs = scaled.unsigned_abs();
    let factor = 10u128.pow(scale);
    let int_part = abs / factor;
    let frac_part = abs % factor;
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push_str(&int_part.to_string());
    if scale > 0 {
        out.push('.');
        out.push_str(&format!("{frac_part:0width$}", width = scale as usize));
    }
    out
}

/// Distance in units-in-the-last-place via the monotone integer mapping of
/// IEEE 754 bit patterns.
fn ulps_apart(a: f64, b: f64) -> u64 {
    fn ordered(x: f64) -> i64 {
        let bits = x.to_bits() as i64;
        if bits < 0 { i64::MIN - bits } else { bits }
    }
    ordered(a).wrapping_sub(ordered(b)).unsigned_abs()
}

// ---------------------------------------------------------------------------
// UUIDs and base64 -- dependency-free
// ---------------------------------------------------------------------------

fn parse_uuid(s: &str) -> Option<u128> {
    let b = s.as_bytes();
    if b.len() != 36 || b[8] != b'-' || b[13] != b'-' || b[18] != b'-' || b[23] != b'-' {
        return None;
    }
    let mut acc: u128 = 0;
    for (i, c) in b.iter().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            continue;
        }
        acc = acc << 4 | (*c as char).to_digit(16)? as u128;
    }
    Some(acc)
}

fn format_uuid(v: u128) -> String {
    let hex = format!("{v:032x}");
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(BASE64_ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { BASE64_ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { BASE64_ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let b = s.as_bytes();
    if !b.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for chunk in b.chunks(4) {
        let pad = chunk.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 || chunk[..4 - pad].contains(&b'=') {
            return None;
        }
        let mut n: u32 = 0;
        for c in &chunk[..4 - pad] {
            n = n << 6 | value(*c)?;
        }
        n <<= 6 * pad as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance::conforms;
    use crate::value::StructField;
    use serde_json::json;

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

    fn sample_event() -> serde_json::Value {
        json!({
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
        })
    }

    #[test]
    fn parsed_values_conform_by_construction() {
        let ty = full_schema();
        let value = json_to_value(&sample_event(), &ty).unwrap();
        conforms(&value, &ty).unwrap();
    }

    #[test]
    fn roundtrip_is_identity_on_canonical_values() {
        let ty = full_schema();
        let value = json_to_value(&sample_event(), &ty).unwrap();
        let emitted = value_to_json(&value, &ty).unwrap();
        let reparsed = json_to_value(&emitted, &ty).unwrap();
        assert_eq!(value, reparsed, "value -> json -> value must be identity");
    }

    #[test]
    fn canonicalization_specifics() {
        let ty = full_schema();
        let value = json_to_value(&sample_event(), &ty).unwrap();
        let Value::Struct(slots) = &value else { panic!("positional struct") };
        assert_eq!(slots[0], Value::U64(7));
        assert_eq!(slots[3], Value::Ipv4(0x0A010203));
        assert_eq!(slots[5], Value::Blob(b"hello".to_vec()));
        // 2026-07-06T12:34:56.789-07:00 == 19:34:56.789Z; offset preserved.
        assert_eq!(slots[7], Value::TimestampTz(1_783_366_496_789, -420));
        assert_eq!(slots[8], Value::DecimalI64(1999));
        assert_eq!(slots[9], Value::Enum(1), "symbol canonicalizes to ordinal");
        let Value::Map(attrs) = &slots[11] else { panic!("map") };
        let keys: Vec<_> = attrs.keys().collect();
        assert_eq!(
            keys,
            [
                &MapKey::String("alpha".into()),
                &MapKey::String("gap".into()),
                &MapKey::String("zeta".into())
            ],
            "sorted entry order"
        );
        assert_eq!(attrs[&MapKey::String("gap".into())], Value::Null);
    }

    #[test]
    fn strictness() {
        let ty = full_schema();
        for (mutation, expect_in_path) in [
            (json!({"id": -1}), "id"),
            (json!({"id": 7, "kind": "purchase"}), "kind"),
            (json!({"id": 7, "price": "1.234"}), "price"),
            (json!({"id": 7, "price": 0.001}), "price"),
            (json!({"id": 7, "day": "2026-02-30"}), "day"),
            (json!({"id": 7, "when": "2026-07-06T25:00:00Z"}), "when"),
            (json!({"id": 7, "blob": "not base64!"}), "blob"),
            (json!({"id": 7, "attrs": {"k": "not an i64"}}), "attrs"),
        ] {
            let mut event = sample_event().as_object().unwrap().clone();
            for (k, v) in mutation.as_object().unwrap() {
                event.insert(k.clone(), v.clone());
            }
            let err = json_to_value(&Json::Object(event), &ty).unwrap_err();
            assert!(
                err.path.contains(expect_in_path),
                "expected {expect_in_path} in path, got: {err}"
            );
        }
    }

    #[test]
    fn float_decimal_ulp_tolerance() {
        let ty = ValueType::Decimal { precision: 10, scale: 2 };
        // Artifact of 0.1 + 0.2: one ULP from 0.3.
        let v = json_to_value(&json!(0.1_f64 + 0.2_f64), &ty).unwrap();
        assert_eq!(v, Value::DecimalI64(30));
        // Genuinely different values still refuse.
        assert!(json_to_value(&json!(19.994), &ty).is_err());
    }

    #[test]
    fn wide_decimals_take_i128() {
        let ty = ValueType::Decimal { precision: 30, scale: 4 };
        let v = json_to_value(&json!("123456789012345678901234.5678"), &ty).unwrap();
        assert_eq!(v, Value::DecimalI128(1_234_567_890_123_456_789_012_345_678));
        let back = value_to_json(&v, &ty).unwrap();
        assert_eq!(back, json!("123456789012345678901234.5678"));
    }

    #[test]
    fn typed_map_keys() {
        let ty = ValueType::Map {
            key_type: Box::new(ValueType::U64),
            value_type: Box::new(ValueType::String),
            values_nullable: false,
        };
        let v = json_to_value(&json!({"42": "answer", "7": "lucky"}), &ty).unwrap();
        let Value::Map(entries) = &v else { panic!("map") };
        assert_eq!(entries[&MapKey::U64(42)], Value::String("answer".into()));
        // Numeric keys sort numerically in canonical form, not lexically.
        assert_eq!(entries.keys().next(), Some(&MapKey::U64(7)));
        let back = value_to_json(&v, &ty).unwrap();
        assert_eq!(back, json!({"7": "lucky", "42": "answer"}));
    }

    #[test]
    fn timestamp_forms() {
        let no_tz = ValueType::Timestamp {
            precision: TimestampPrecision::Micros,
            timezone: TimestampTimezone::None,
        };
        // Integer passthrough.
        assert_eq!(json_to_value(&json!(123456), &no_tz).unwrap(), Value::Timestamp(123456));
        // RFC 3339 with offset normalizes to the UTC instant when tz=None.
        let a = json_to_value(&json!("1970-01-01T01:00:00+01:00"), &no_tz).unwrap();
        assert_eq!(a, Value::Timestamp(0));
        // Sub-precision digits must be zero.
        let millis = ValueType::Timestamp {
            precision: TimestampPrecision::Millis,
            timezone: TimestampTimezone::None,
        };
        assert!(json_to_value(&json!("1970-01-01T00:00:00.1239Z"), &millis).is_err());
        assert_eq!(
            json_to_value(&json!("1970-01-01T00:00:00.1230Z"), &millis).unwrap(),
            Value::Timestamp(123)
        );
        // TimestampTz round-trips through RFC 3339 with the offset intact.
        let tz = ValueType::Timestamp {
            precision: TimestampPrecision::Millis,
            timezone: TimestampTimezone::UtcOffset,
        };
        let v = json_to_value(&json!("2026-07-06T12:34:56.789-07:00"), &tz).unwrap();
        let emitted = value_to_json(&v, &tz).unwrap();
        assert_eq!(emitted, json!("2026-07-06T12:34:56.789-07:00"));
    }

    #[test]
    fn nulls_and_defaults() {
        let ty = full_schema();
        // Only required fields; every nullable one absent.
        let v = json_to_value(
            &json!({
                "id": 1, "active": false, "ip": "0.0.0.1",
                "uid": "67e55044-10b1-426f-9247-bb680e5fe0c8", "blob": "",
                "day": "1970-01-01", "when": 0, "kind": "click"
            }),
            &ty,
        )
        .unwrap();
        conforms(&v, &ty).unwrap();
        let Value::Struct(slots) = &v else { panic!() };
        assert_eq!(slots[1], Value::Null, "absent nullable is Null");
        // Missing required field errors with its path.
        let err = json_to_value(&json!({"active": true}), &ty).unwrap_err();
        assert!(err.path.contains("id"), "{err}");
        // Canonical emission omits null slots and round-trips.
        let emitted = value_to_json(&v, &ty).unwrap();
        assert!(emitted.get("name").is_none());
        assert_eq!(json_to_value(&emitted, &ty).unwrap(), v);
    }

    #[test]
    fn civil_date_math_agrees_with_known_anchors() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("1970-01-02"), Some(1));
        assert_eq!(parse_date("1969-12-31"), Some(-1));
        assert_eq!(parse_date("2000-03-01"), Some(11017));
        assert_eq!(parse_date("2026-07-06"), Some(20640));
        assert_eq!(format_date(20640), "2026-07-06");
        assert_eq!(parse_date("2026-02-30"), None);
        assert_eq!(parse_date("2024-02-29").is_some(), true, "leap year");
        assert_eq!(parse_date("2023-02-29"), None);
    }

    #[test]
    fn base64_roundtrip() {
        for input in [&b""[..], b"f", b"fo", b"foo", b"foob", b"hello world!"] {
            let encoded = base64_encode(input);
            assert_eq!(base64_decode(&encoded).as_deref(), Some(input), "{encoded}");
        }
        assert_eq!(base64_decode("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(base64_decode("bad!"), None);
        assert_eq!(base64_decode("abc"), None, "length must be a multiple of 4");
    }
}
