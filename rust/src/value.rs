use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,

    // --- Scalar ---
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Date(i32),
    Uuid(u128),
    Ipv4(u32),
    Ipv6(u128),
    Blob(Vec<u8>),
    Clob(Vec<u8>),
    String(String),

    // --- Parameterized scalar ---
    DecimalI64(i64),
    DecimalI128(i128),
    Timestamp(i64),
    TimestampTz(i64, i16),

    // --- Compound ---
    Enum(u32),
    Array(Vec<Value>),
    Map(BTreeMap<MapKey, Value>),
    Struct(Vec<Value>),

}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MapKey {
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    Date(i32),
    Uuid(u128),
    Ipv4(u32),
    Ipv6(u128),
    String(String),
    Timestamp(i64),
    TimestampTz(i64, i16),
}

/// A flat, `Copy`-able discriminant tag identifying which *family* a value belongs to.
///
/// `ValueKind` deliberately carries no parameters — no precision, no element types,
/// no field lists. It is the moral equivalent of Substrait's "kind" and exists for
/// fast switching, storage in bitmasks, and anywhere a full type tree would be
/// needlessly heavy (e.g. discriminant checks in `Value::kind()`).
///
/// Contrast with [`ValueType`], which is a *tree* describing the complete type
/// including parameters and recursively nested element/field types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueKind {
    Null,
    Bool,
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    F32, F64,
    Date,
    Uuid,
    Ipv4,
    Ipv6,
    Blob,
    Clob,
    String,
    DecimalI64,
    DecimalI128,
    Timestamp,
    TimestampTz,
    Enum,
    Array,
    Map,
    Struct,
    EntityRef,
}

// ---------------------------------------------------------------------------
// ValueType — the full recursive type tree
// ---------------------------------------------------------------------------

/// A complete, recursive type descriptor — the Rust counterpart of the
/// `ValueType` message in `value.proto`.
///
/// Where [`ValueKind`] is a flat tag ("`this is an Array`"), `ValueType` is the
/// whole story ("`this is an Array whose elements are non-nullable Map<String,
/// Timestamp(Millis, UtcOffset)>`"). Think of `ValueKind` as the top-level
/// discriminant you get from [`ValueType::kind()`]; `ValueType` itself is the
/// tree you need whenever you must reason about nested structure — schema
/// validation, column decomposition, codegen, serde, and type checking.
///
/// The parallel to Substrait is intentional: Substrait distinguishes *kinds*
/// (simple tags) from *types* (parameterized, recursively composable).  We do
/// the same.  Nullability is **not** part of `ValueType`; it lives only in
/// containers ([`StructField`], `ArrayType::elements_nullable`,
/// `MapType::values_nullable`) and at the column level.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ValueType {
    // --- Scalar (no parameters) ---
    Null,
    Bool,
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    F32, F64,
    Date,
    Uuid,
    Ipv4,
    Ipv6,
    Blob,
    Clob,
    String,

    // --- Parameterized scalar ---
    Decimal {
        precision: u32,
        scale: u32,
    },
    Timestamp {
        precision: TimestampPrecision,
        timezone: TimestampTimezone,
    },

    // --- Compound ---
    Enum {
        values: Vec<String>,
    },
    Array {
        element_type: Box<ValueType>,
        elements_nullable: bool,
    },
    Map {
        key_type: Box<ValueType>,
        value_type: Box<ValueType>,
        values_nullable: bool,
    },
    Struct {
        fields: Vec<StructField>,
    },

    // --- Foreign key ---
    EntityRef {
        target_type_id: String,
        key_type: Box<ValueType>,
        revision_pinnable: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructField {
    pub name: String,
    pub human_name: String,
    pub value_type: ValueType,
    pub nullable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimestampPrecision {
    Unspecified,
    Seconds,
    Millis,
    Micros,
    Nanos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimestampTimezone {
    None,
    UtcOffset,
}

impl ValueType {
    /// Return the flat [`ValueKind`] tag for this type, discarding all parameters.
    pub fn kind(&self) -> ValueKind {
        match self {
            ValueType::Null => ValueKind::Null,
            ValueType::Bool => ValueKind::Bool,
            ValueType::I8 => ValueKind::I8,
            ValueType::I16 => ValueKind::I16,
            ValueType::I32 => ValueKind::I32,
            ValueType::I64 => ValueKind::I64,
            ValueType::U8 => ValueKind::U8,
            ValueType::U16 => ValueKind::U16,
            ValueType::U32 => ValueKind::U32,
            ValueType::U64 => ValueKind::U64,
            ValueType::F32 => ValueKind::F32,
            ValueType::F64 => ValueKind::F64,
            ValueType::Date => ValueKind::Date,
            ValueType::Uuid => ValueKind::Uuid,
            ValueType::Ipv4 => ValueKind::Ipv4,
            ValueType::Ipv6 => ValueKind::Ipv6,
            ValueType::Blob => ValueKind::Blob,
            ValueType::Clob => ValueKind::Clob,
            ValueType::String => ValueKind::String,
            ValueType::Decimal { precision, .. } => {
                if *precision <= 18 { ValueKind::DecimalI64 } else { ValueKind::DecimalI128 }
            }
            ValueType::Timestamp { timezone: TimestampTimezone::None, .. } => ValueKind::Timestamp,
            ValueType::Timestamp { timezone: TimestampTimezone::UtcOffset, .. } => ValueKind::TimestampTz,
            ValueType::Enum { .. } => ValueKind::Enum,
            ValueType::Array { .. } => ValueKind::Array,
            ValueType::Map { .. } => ValueKind::Map,
            ValueType::Struct { .. } => ValueKind::Struct,
            ValueType::EntityRef { .. } => ValueKind::EntityRef,
        }
    }

    pub fn is_scalar(&self) -> bool {
        !matches!(self, ValueType::Array { .. } | ValueType::Map { .. } | ValueType::Struct { .. })
    }

    pub fn is_compound(&self) -> bool {
        matches!(self, ValueType::Array { .. } | ValueType::Map { .. } | ValueType::Struct { .. })
    }
}

impl Value {
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::Null => ValueKind::Null,
            Value::Bool(_) => ValueKind::Bool,
            Value::I8(_) => ValueKind::I8,
            Value::I16(_) => ValueKind::I16,
            Value::I32(_) => ValueKind::I32,
            Value::I64(_) => ValueKind::I64,
            Value::U8(_) => ValueKind::U8,
            Value::U16(_) => ValueKind::U16,
            Value::U32(_) => ValueKind::U32,
            Value::U64(_) => ValueKind::U64,
            Value::F32(_) => ValueKind::F32,
            Value::F64(_) => ValueKind::F64,
            Value::Date(_) => ValueKind::Date,
            Value::Uuid(_) => ValueKind::Uuid,
            Value::Ipv4(_) => ValueKind::Ipv4,
            Value::Ipv6(_) => ValueKind::Ipv6,
            Value::Blob(_) => ValueKind::Blob,
            Value::Clob(_) => ValueKind::Clob,
            Value::String(_) => ValueKind::String,
            Value::DecimalI64(_) => ValueKind::DecimalI64,
            Value::DecimalI128(_) => ValueKind::DecimalI128,
            Value::Timestamp(_) => ValueKind::Timestamp,
            Value::TimestampTz(_, _) => ValueKind::TimestampTz,
            Value::Enum(_) => ValueKind::Enum,
            Value::Array(_) => ValueKind::Array,
            Value::Map(_) => ValueKind::Map,
            Value::Struct(_) => ValueKind::Struct,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Returns `true` if this value is a collection type (Array, Map, Struct).
    pub fn is_collection(&self) -> bool {
        matches!(self, Value::Array(_) | Value::Map(_) | Value::Struct(_))
    }

    pub fn is_zero(&self) -> bool {
        match self {
            Value::I8(0) | Value::I16(0) | Value::I32(0) | Value::I64(0)
            | Value::U8(0) | Value::U16(0) | Value::U32(0) | Value::U64(0)
            | Value::DecimalI64(0) | Value::DecimalI128(0) => true,
            Value::F32(v) => *v == 0.0,
            Value::F64(v) => *v == 0.0,
            _ => false,
        }
    }

    pub fn is_one(&self) -> bool {
        match self {
            Value::I8(1) | Value::I16(1) | Value::I32(1) | Value::I64(1)
            | Value::U8(1) | Value::U16(1) | Value::U32(1) | Value::U64(1)
            | Value::DecimalI64(1) | Value::DecimalI128(1) => true,
            Value::F32(v) => *v == 1.0,
            Value::F64(v) => *v == 1.0,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Value::Null => {}
            Value::Bool(v) => v.hash(state),
            Value::I8(v) => v.hash(state),
            Value::I16(v) => v.hash(state),
            Value::I32(v) => v.hash(state),
            Value::I64(v) => v.hash(state),
            Value::U8(v) => v.hash(state),
            Value::U16(v) => v.hash(state),
            Value::U32(v) => v.hash(state),
            Value::U64(v) => v.hash(state),
            Value::F32(v) => v.to_bits().hash(state),
            Value::F64(v) => v.to_bits().hash(state),
            Value::Date(v) => v.hash(state),
            Value::Uuid(v) => v.hash(state),
            Value::Ipv4(v) => v.hash(state),
            Value::Ipv6(v) => v.hash(state),
            Value::Blob(v) => v.hash(state),
            Value::Clob(v) => v.hash(state),
            Value::String(v) => v.hash(state),
            Value::DecimalI64(v) => v.hash(state),
            Value::DecimalI128(v) => v.hash(state),
            Value::Timestamp(v) => v.hash(state),
            Value::TimestampTz(v, off) => { v.hash(state); off.hash(state); }
            Value::Enum(v) => v.hash(state),
            Value::Array(v) => v.hash(state),
            Value::Map(v) => {
                v.len().hash(state);
                for (k, val) in v {
                    k.hash(state);
                    val.hash(state);
                }
            }
            Value::Struct(v) => v.hash(state),
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self { Value::Bool(v) }
}

impl From<i8> for Value {
    fn from(v: i8) -> Self { Value::I8(v) }
}

impl From<i16> for Value {
    fn from(v: i16) -> Self { Value::I16(v) }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self { Value::I64(v as i64) }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self { Value::I64(v) }
}

impl From<u8> for Value {
    fn from(v: u8) -> Self { Value::U8(v) }
}

impl From<u16> for Value {
    fn from(v: u16) -> Self { Value::U16(v) }
}

impl From<u32> for Value {
    fn from(v: u32) -> Self { Value::U64(v as u64) }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self { Value::U64(v) }
}

impl From<f32> for Value {
    fn from(v: f32) -> Self { Value::F64(v as f64) }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self { Value::F64(v) }
}

impl From<String> for Value {
    fn from(v: String) -> Self { Value::String(v) }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self { Value::String(v.to_string()) }
}

impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self { Value::Blob(v) }
}

impl From<&[u8]> for Value {
    fn from(v: &[u8]) -> Self { Value::Blob(v.to_vec()) }
}

impl From<u128> for Value {
    fn from(v: u128) -> Self { Value::Uuid(v) }
}

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self { Value::Array(v) }
}

// ---------------------------------------------------------------------------
// IP address parsing and formatting
// ---------------------------------------------------------------------------

pub fn parse_ipv4(s: &str) -> Option<u32> {
    s.parse::<std::net::Ipv4Addr>().ok().map(|a| u32::from(a))
}

pub fn format_ipv4(addr: u32) -> String {
    std::net::Ipv4Addr::from(addr).to_string()
}

pub fn parse_ipv6(s: &str) -> Option<u128> {
    s.parse::<std::net::Ipv6Addr>().ok().map(|a| u128::from(a))
}

pub fn format_ipv6(addr: u128) -> String {
    std::net::Ipv6Addr::from(addr).to_string()
}

/// Parse a CIDR block into (network, mask). Supports full notation
/// (`192.168.1.0/24`) and shorthand (`10/8`, `192.168/16`) where
/// omitted trailing octets are treated as zero.
pub fn parse_cidr_v4(s: &str) -> Option<(u32, u32)> {
    let (addr_part, prefix_str) = s.split_once('/')?;
    let prefix_len: u32 = prefix_str.parse().ok()?;
    if prefix_len > 32 {
        return None;
    }

    let octets: Vec<&str> = addr_part.split('.').collect();
    if octets.is_empty() || octets.len() > 4 {
        return None;
    }
    let mut bytes = [0u8; 4];
    for (i, octet) in octets.iter().enumerate() {
        bytes[i] = octet.parse().ok()?;
    }

    let addr = u32::from_be_bytes(bytes);
    let mask = if prefix_len == 0 { 0 } else { !0u32 << (32 - prefix_len) };
    Some((addr & mask, mask))
}

/// Parse an IPv6 CIDR block into (network, mask).
pub fn parse_cidr_v6(s: &str) -> Option<(u128, u128)> {
    let (addr_part, prefix_str) = s.split_once('/')?;
    let prefix_len: u32 = prefix_str.parse().ok()?;
    if prefix_len > 128 {
        return None;
    }

    let addr = parse_ipv6(addr_part)?;
    let mask = if prefix_len == 0 { 0 } else { !0u128 << (128 - prefix_len) };
    Some((addr & mask, mask))
}

/// Try parsing as v4 CIDR first, then v6. Returns (is_v4, network, mask).
pub fn parse_cidr(s: &str) -> Option<CidrBlock> {
    if let Some((network, mask)) = parse_cidr_v4(s) {
        Some(CidrBlock::V4 { network, mask })
    } else if let Some((network, mask)) = parse_cidr_v6(s) {
        Some(CidrBlock::V6 { network, mask })
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CidrBlock {
    V4 { network: u32, mask: u32 },
    V6 { network: u128, mask: u128 },
}

impl CidrBlock {
    pub fn contains_v4(&self, addr: u32) -> bool {
        match self {
            CidrBlock::V4 { network, mask } => (addr & mask) == *network,
            CidrBlock::V6 { network, mask } => {
                let mapped = 0xffff_0000_0000u128 | addr as u128;
                (mapped & mask) == *network
            }
        }
    }

    pub fn contains_v6(&self, addr: u128) -> bool {
        match self {
            CidrBlock::V4 { network, mask } => {
                if addr >> 32 == 0x0000_ffff {
                    ((addr as u32) & mask) == *network
                } else {
                    false
                }
            }
            CidrBlock::V6 { network, mask } => (addr & mask) == *network,
        }
    }
}

// ---------------------------------------------------------------------------
// Proto ↔ ValueType conversions
// ---------------------------------------------------------------------------

mod proto_conv {
    use super::*;
    use crate::cluonflux::meta as pb;

    impl From<&ValueType> for pb::ValueType {
        fn from(vt: &ValueType) -> Self {
            use pb::value_type::Kind;
            let kind = match vt {
                ValueType::Null => Kind::Boolean(pb::BooleanType {}),
                ValueType::Bool => Kind::Boolean(pb::BooleanType {}),
                ValueType::I8 => Kind::Int8(pb::Int8Type {}),
                ValueType::I16 => Kind::Int16(pb::Int16Type {}),
                ValueType::I32 => Kind::Int32(pb::Int32Type {}),
                ValueType::I64 => Kind::Int64(pb::Int64Type {}),
                ValueType::U8 => Kind::Uint8(pb::UInt8Type {}),
                ValueType::U16 => Kind::Uint16(pb::UInt16Type {}),
                ValueType::U32 => Kind::Uint32(pb::UInt32Type {}),
                ValueType::U64 => Kind::Uint64(pb::UInt64Type {}),
                ValueType::F32 => Kind::Float32(pb::Float32Type {}),
                ValueType::F64 => Kind::Float64(pb::Float64Type {}),
                ValueType::Date => Kind::Date(pb::DateType {}),
                ValueType::Uuid => Kind::Uuid(pb::UuidType {}),
                ValueType::Ipv4 => Kind::Ipv4(pb::Ipv4Type {}),
                ValueType::Ipv6 => Kind::Ipv6(pb::Ipv6Type {}),
                ValueType::Blob => Kind::Blob(pb::BlobType {}),
                ValueType::Clob => Kind::Clob(pb::ClobType { encoding: String::new() }),
                ValueType::String => Kind::String(pb::StringType { max_length: 0 }),
                ValueType::Decimal { precision, scale } => Kind::Decimal(pb::DecimalType {
                    precision: *precision,
                    scale: *scale,
                }),
                ValueType::Timestamp { precision, timezone } => Kind::Timestamp(pb::TimestampType {
                    precision: ts_precision_to_proto(*precision) as i32,
                    timezone: ts_timezone_to_proto(*timezone) as i32,
                }),
                ValueType::Enum { values } => Kind::EnumType(pb::EnumType {
                    values: values.clone(),
                }),
                ValueType::Array { element_type, elements_nullable } => Kind::Array(Box::new(pb::ArrayType {
                    element_type: Some(Box::new(pb::ValueType::from(element_type.as_ref()))),
                    elements_nullable: *elements_nullable,
                })),
                ValueType::Map { key_type, value_type, values_nullable } => Kind::Map(Box::new(pb::MapType {
                    key_type: Some(Box::new(pb::ValueType::from(key_type.as_ref()))),
                    value_type: Some(Box::new(pb::ValueType::from(value_type.as_ref()))),
                    values_nullable: *values_nullable,
                })),
                ValueType::Struct { fields } => Kind::StructType(pb::StructType {
                    fields: fields.iter().map(|f| pb::StructField {
                        name: f.name.clone(),
                        human_name: f.human_name.clone(),
                        value_type: Some(pb::ValueType::from(&f.value_type)),
                        nullable: f.nullable,
                    }).collect(),
                }),
                ValueType::EntityRef { target_type_id, key_type, revision_pinnable } => {
                    Kind::EntityRef(Box::new(pb::EntityRefType {
                        target_type_id: target_type_id.clone(),
                        key_type: Some(Box::new(pb::ValueType::from(key_type.as_ref()))),
                        revision_pinnable: *revision_pinnable,
                    }))
                }
            };
            pb::ValueType { kind: Some(kind) }
        }
    }

    impl From<&pb::ValueType> for ValueType {
        fn from(proto: &pb::ValueType) -> Self {
            use pb::value_type::Kind;
            match &proto.kind {
                None => ValueType::Null,
                Some(kind) => match kind {
                    Kind::Boolean(_) => ValueType::Bool,
                    Kind::String(_) => ValueType::String,
                    Kind::Int8(_) => ValueType::I8,
                    Kind::Int16(_) => ValueType::I16,
                    Kind::Int32(_) => ValueType::I32,
                    Kind::Int64(_) => ValueType::I64,
                    Kind::Uint8(_) => ValueType::U8,
                    Kind::Uint16(_) => ValueType::U16,
                    Kind::Uint32(_) => ValueType::U32,
                    Kind::Uint64(_) => ValueType::U64,
                    Kind::Float32(_) => ValueType::F32,
                    Kind::Float64(_) => ValueType::F64,
                    Kind::Date(_) => ValueType::Date,
                    Kind::Uuid(_) => ValueType::Uuid,
                    Kind::Ipv4(_) => ValueType::Ipv4,
                    Kind::Ipv6(_) => ValueType::Ipv6,
                    Kind::Blob(_) => ValueType::Blob,
                    Kind::Clob(_) => ValueType::Clob,
                    Kind::Decimal(d) => ValueType::Decimal {
                        precision: d.precision,
                        scale: d.scale,
                    },
                    Kind::Timestamp(t) => ValueType::Timestamp {
                        precision: ts_precision_from_proto(t.precision),
                        timezone: ts_timezone_from_proto(t.timezone),
                    },
                    Kind::EnumType(e) => ValueType::Enum {
                        values: e.values.clone(),
                    },
                    Kind::Array(a) => ValueType::Array {
                        element_type: Box::new(a.element_type.as_deref()
                            .map(ValueType::from).unwrap_or(ValueType::Null)),
                        elements_nullable: a.elements_nullable,
                    },
                    Kind::Map(m) => ValueType::Map {
                        key_type: Box::new(m.key_type.as_deref()
                            .map(ValueType::from).unwrap_or(ValueType::String)),
                        value_type: Box::new(m.value_type.as_deref()
                            .map(ValueType::from).unwrap_or(ValueType::Null)),
                        values_nullable: m.values_nullable,
                    },
                    Kind::StructType(s) => ValueType::Struct {
                        fields: s.fields.iter().map(|f| StructField {
                            name: f.name.clone(),
                            human_name: f.human_name.clone(),
                            value_type: f.value_type.as_ref()
                                .map(ValueType::from).unwrap_or(ValueType::Null),
                            nullable: f.nullable,
                        }).collect(),
                    },
                    Kind::EntityRef(e) => ValueType::EntityRef {
                        target_type_id: e.target_type_id.clone(),
                        key_type: Box::new(e.key_type.as_deref()
                            .map(ValueType::from).unwrap_or(ValueType::Uuid)),
                        revision_pinnable: e.revision_pinnable,
                    },
                    Kind::TypeRef(_) => ValueType::Null,
                },
            }
        }
    }

    fn ts_precision_to_proto(p: TimestampPrecision) -> pb::TimestampPrecision {
        match p {
            TimestampPrecision::Unspecified => pb::TimestampPrecision::Unspecified,
            TimestampPrecision::Seconds => pb::TimestampPrecision::Seconds,
            TimestampPrecision::Millis => pb::TimestampPrecision::Millis,
            TimestampPrecision::Micros => pb::TimestampPrecision::Micros,
            TimestampPrecision::Nanos => pb::TimestampPrecision::Nanos,
        }
    }

    fn ts_precision_from_proto(v: i32) -> TimestampPrecision {
        match pb::TimestampPrecision::try_from(v) {
            Ok(pb::TimestampPrecision::Seconds) => TimestampPrecision::Seconds,
            Ok(pb::TimestampPrecision::Millis) => TimestampPrecision::Millis,
            Ok(pb::TimestampPrecision::Micros) => TimestampPrecision::Micros,
            Ok(pb::TimestampPrecision::Nanos) => TimestampPrecision::Nanos,
            _ => TimestampPrecision::Unspecified,
        }
    }

    fn ts_timezone_to_proto(t: TimestampTimezone) -> pb::TimestampTimezone {
        match t {
            TimestampTimezone::None => pb::TimestampTimezone::None,
            TimestampTimezone::UtcOffset => pb::TimestampTimezone::UtcOffset,
        }
    }

    fn ts_timezone_from_proto(v: i32) -> TimestampTimezone {
        match pb::TimestampTimezone::try_from(v) {
            Ok(pb::TimestampTimezone::UtcOffset) => TimestampTimezone::UtcOffset,
            _ => TimestampTimezone::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ipv4_basic() {
        assert_eq!(parse_ipv4("192.168.1.1"), Some(0xc0a80101));
        assert_eq!(parse_ipv4("0.0.0.0"), Some(0));
        assert_eq!(parse_ipv4("255.255.255.255"), Some(0xffffffff));
        assert_eq!(parse_ipv4("not an ip"), None);
    }

    #[test]
    fn format_ipv4_basic() {
        assert_eq!(format_ipv4(0xc0a80101), "192.168.1.1");
        assert_eq!(format_ipv4(0), "0.0.0.0");
    }

    #[test]
    fn parse_ipv6_basic() {
        assert_eq!(parse_ipv6("::1"), Some(1));
        assert_eq!(parse_ipv6("fe80::1"), Some(0xfe80_0000_0000_0000_0000_0000_0000_0001));
        assert_eq!(parse_ipv6("nope"), None);
    }

    #[test]
    fn cidr_v4_full_notation() {
        let (net, mask) = parse_cidr_v4("192.168.1.0/24").unwrap();
        assert_eq!(net, 0xc0a80100);
        assert_eq!(mask, 0xffffff00);
    }

    #[test]
    fn cidr_v4_shorthand() {
        let (net, mask) = parse_cidr_v4("10/8").unwrap();
        assert_eq!(net, 0x0a000000);
        assert_eq!(mask, 0xff000000);

        let (net, mask) = parse_cidr_v4("192.168/16").unwrap();
        assert_eq!(net, 0xc0a80000);
        assert_eq!(mask, 0xffff0000);

        let (net, mask) = parse_cidr_v4("172.16.0/12").unwrap();
        assert_eq!(net, 0xac100000);
        assert_eq!(mask, 0xfff00000);
    }

    #[test]
    fn cidr_v4_host_route() {
        let (net, mask) = parse_cidr_v4("10.0.0.1/32").unwrap();
        assert_eq!(mask, 0xffffffff);
        assert_eq!(net, 0x0a000001);
    }

    #[test]
    fn cidr_v4_default_route() {
        let (net, mask) = parse_cidr_v4("0/0").unwrap();
        assert_eq!(net, 0);
        assert_eq!(mask, 0);
    }

    #[test]
    fn cidr_v4_invalid() {
        assert!(parse_cidr_v4("10.0.0.0/33").is_none());
        assert!(parse_cidr_v4("no-slash").is_none());
        assert!(parse_cidr_v4("/8").is_none());
    }

    #[test]
    fn cidr_v6_basic() {
        let (net, mask) = parse_cidr_v6("fe80::/10").unwrap();
        assert_eq!(net, 0xfe80_0000_0000_0000_0000_0000_0000_0000);
        assert_eq!(mask, 0xffc0_0000_0000_0000_0000_0000_0000_0000);
    }

    #[test]
    fn cidr_contains_v4() {
        let block = parse_cidr("10/8").unwrap();
        assert!(block.contains_v4(0x0a010203)); // 10.1.2.3
        assert!(!block.contains_v4(0x0b000000)); // 11.0.0.0
    }

    #[test]
    fn cidr_contains_v6() {
        let block = parse_cidr("fe80::/10").unwrap();
        assert!(block.contains_v6(0xfe80_0000_0000_0000_0000_0000_0000_0001));
        assert!(!block.contains_v6(0xff00_0000_0000_0000_0000_0000_0000_0001));
    }
}
