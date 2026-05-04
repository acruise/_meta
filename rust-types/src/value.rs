use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

/// Tagged union of every value the system can represent.
///
/// This type exists in tension with a core design principle: the buffer
/// and codegen paths work with typed, unboxed columnar data -- not
/// tagged enums. A `Column<I64Col>` stores raw `i64`s with a validity
/// bitmap; the compiled scatter path reads struct fields directly into
/// typed columns without ever constructing a `Value`.
///
/// So why does `Value` exist? Because the interpreted evaluator, the
/// proto serde boundary, expression literals, test fixtures, and the
/// UDF bridge all need a way to carry "a value of any type" through
/// code that is generic over type. It is the path of least resistance
/// for anything that is NOT the hot ingest/query path.
///
/// The goal is that `Value` never appears on the hot path. Every place
/// it shows up today should be either:
///   (a) cold-path infrastructure (schema transitions, serde, tests), or
///   (b) the interpreted evaluator, which exists as a correctness
///       reference and fallback -- not as the production execution mode.
///
/// If you find `Value` on a path that matters for latency, that is a
/// bug in the architecture, not a reason to optimize `Value`.
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

// ---------------------------------------------------------------------------
// ValueType — the full recursive type tree
// ---------------------------------------------------------------------------

/// A complete, recursive type descriptor — the Rust counterpart of the
/// `ValueType` message in `value.proto`.
///
/// `ValueType` describes the full story ("`this is an Array whose elements are
/// non-nullable Map<String, Timestamp(Millis, UtcOffset)>`"). It is the tree
/// you need whenever you must reason about nested structure — schema validation,
/// column decomposition, codegen, serde, and type checking.
///
/// Nullability is **not** part of `ValueType`; it lives only in containers
/// ([`StructField`], `ArrayType::elements_nullable`, `MapType::values_nullable`)
/// and at the column level.
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
    pub fn is_scalar(&self) -> bool {
        !matches!(self, ValueType::Array { .. } | ValueType::Map { .. } | ValueType::Struct { .. })
    }

    pub fn is_compound(&self) -> bool {
        matches!(self, ValueType::Array { .. } | ValueType::Map { .. } | ValueType::Struct { .. })
    }
}

impl Value {
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

// ---------------------------------------------------------------------------
// Value ↔ EncodedValue conversions (typed serde)
// ---------------------------------------------------------------------------

mod value_serde {
    use super::*;
    use crate::cluonflux::meta as pb;
    use pb::encoded_value::Kind;

    pub fn encode_value(value: &Value, _vt: &ValueType) -> pb::EncodedValue {
        let kind = match value {
            Value::Null => None,
            Value::Bool(b) => Some(Kind::BoolValue(*b)),

            Value::I8(v) => Some(Kind::IntValue(*v as i64)),
            Value::I16(v) => Some(Kind::IntValue(*v as i64)),
            Value::I32(v) => Some(Kind::IntValue(*v as i64)),
            Value::I64(v) => Some(Kind::IntValue(*v)),
            Value::Date(v) => Some(Kind::IntValue(*v as i64)),

            Value::U8(v) => Some(Kind::UintValue(*v as u64)),
            Value::U16(v) => Some(Kind::UintValue(*v as u64)),
            Value::U32(v) => Some(Kind::UintValue(*v as u64)),
            Value::U64(v) => Some(Kind::UintValue(*v)),
            Value::Enum(v) => Some(Kind::UintValue(*v as u64)),

            Value::F32(v) => Some(Kind::FloatValue(*v as f64)),
            Value::F64(v) => Some(Kind::FloatValue(*v)),

            Value::String(s) => Some(Kind::StringValue(s.clone())),

            Value::Blob(b) => Some(Kind::BytesValue(b.clone())),
            Value::Clob(b) => Some(Kind::BytesValue(b.clone())),
            Value::Uuid(v) => Some(Kind::BytesValue(v.to_le_bytes().to_vec())),
            Value::Ipv4(v) => Some(Kind::BytesValue(v.to_be_bytes().to_vec())),
            Value::Ipv6(v) => Some(Kind::BytesValue(v.to_be_bytes().to_vec())),

            Value::Timestamp(v) => Some(Kind::TimestampValue(*v)),
            Value::TimestampTz(epoch, offset) => Some(Kind::TimestampTzValue(
                pb::TimestampTzValue { epoch: *epoch, offset_minutes: *offset as i32 },
            )),

            Value::DecimalI64(v) => Some(Kind::DecimalI64Value(*v)),
            Value::DecimalI128(v) => Some(Kind::DecimalI128Value(v.to_le_bytes().to_vec())),

            Value::Struct(fields) => {
                let struct_vt = match _vt {
                    ValueType::Struct { fields: field_types } => field_types,
                    _ => return pb::EncodedValue { kind: None },
                };
                let encoded_fields = fields.iter().enumerate().map(|(i, f)| {
                    let ft = struct_vt.get(i)
                        .map(|sf| &sf.value_type)
                        .unwrap_or(&ValueType::Null);
                    encode_value(f, ft)
                }).collect();
                Some(Kind::StructValue(pb::EncodedStructValue { fields: encoded_fields }))
            }

            Value::Array(elems) => {
                let elem_vt = match _vt {
                    ValueType::Array { element_type, .. } => element_type.as_ref(),
                    _ => &ValueType::Null,
                };
                let encoded = elems.iter().map(|e| encode_value(e, elem_vt)).collect();
                Some(Kind::ArrayValue(pb::EncodedArrayValue { elements: encoded }))
            }

            Value::Map(entries) => {
                let (key_vt, val_vt) = match _vt {
                    ValueType::Map { key_type, value_type, .. } => {
                        (key_type.as_ref(), value_type.as_ref())
                    }
                    _ => (&ValueType::Null, &ValueType::Null),
                };
                let encoded = entries.iter().map(|(k, v)| {
                    let key_val = map_key_to_value(k);
                    pb::EncodedMapEntry {
                        key: Some(encode_value(&key_val, key_vt)),
                        value: Some(encode_value(v, val_vt)),
                    }
                }).collect();
                Some(Kind::MapValue(pb::EncodedMapValue { entries: encoded }))
            }
        };
        pb::EncodedValue { kind }
    }

    pub fn decode_value(proto: &pb::EncodedValue, vt: &ValueType) -> Value {
        let kind = match &proto.kind {
            None => return Value::Null,
            Some(k) => k,
        };

        match (kind, vt) {
            (Kind::BoolValue(b), _) => Value::Bool(*b),

            (Kind::IntValue(v), ValueType::I8) => Value::I8(*v as i8),
            (Kind::IntValue(v), ValueType::I16) => Value::I16(*v as i16),
            (Kind::IntValue(v), ValueType::I32) => Value::I32(*v as i32),
            (Kind::IntValue(v), ValueType::Date) => Value::Date(*v as i32),
            (Kind::IntValue(v), _) => Value::I64(*v),

            (Kind::UintValue(v), ValueType::U8) => Value::U8(*v as u8),
            (Kind::UintValue(v), ValueType::U16) => Value::U16(*v as u16),
            (Kind::UintValue(v), ValueType::U32) => Value::U32(*v as u32),
            (Kind::UintValue(v), ValueType::Enum { .. }) => Value::Enum(*v as u32),
            (Kind::UintValue(v), _) => Value::U64(*v),

            (Kind::FloatValue(v), ValueType::F32) => Value::F32(*v as f32),
            (Kind::FloatValue(v), _) => Value::F64(*v),

            (Kind::StringValue(s), _) => Value::String(s.clone()),

            (Kind::BytesValue(b), ValueType::Clob) => Value::Clob(b.clone()),
            (Kind::BytesValue(b), ValueType::Uuid) => {
                let arr: [u8; 16] = b.as_slice().try_into().unwrap_or([0; 16]);
                Value::Uuid(u128::from_le_bytes(arr))
            }
            (Kind::BytesValue(b), ValueType::Ipv4) => {
                let arr: [u8; 4] = b.as_slice().try_into().unwrap_or([0; 4]);
                Value::Ipv4(u32::from_be_bytes(arr))
            }
            (Kind::BytesValue(b), ValueType::Ipv6) => {
                let arr: [u8; 16] = b.as_slice().try_into().unwrap_or([0; 16]);
                Value::Ipv6(u128::from_be_bytes(arr))
            }
            (Kind::BytesValue(b), _) => Value::Blob(b.clone()),

            (Kind::TimestampValue(v), _) => Value::Timestamp(*v),
            (Kind::TimestampTzValue(tz), _) => {
                Value::TimestampTz(tz.epoch, tz.offset_minutes as i16)
            }

            (Kind::DecimalI64Value(v), _) => Value::DecimalI64(*v),
            (Kind::DecimalI128Value(b), _) => {
                let arr: [u8; 16] = b.as_slice().try_into().unwrap_or([0; 16]);
                Value::DecimalI128(i128::from_le_bytes(arr))
            }

            (Kind::StructValue(sv), ValueType::Struct { fields: field_types }) => {
                let values = sv.fields.iter().enumerate().map(|(i, f)| {
                    let ft = field_types.get(i)
                        .map(|sf| &sf.value_type)
                        .unwrap_or(&ValueType::Null);
                    decode_value(f, ft)
                }).collect();
                Value::Struct(values)
            }
            (Kind::StructValue(sv), _) => {
                Value::Struct(sv.fields.iter().map(|f| decode_value(f, &ValueType::Null)).collect())
            }

            (Kind::ArrayValue(av), ValueType::Array { element_type, .. }) => {
                Value::Array(av.elements.iter().map(|e| decode_value(e, element_type)).collect())
            }
            (Kind::ArrayValue(av), _) => {
                Value::Array(av.elements.iter().map(|e| decode_value(e, &ValueType::Null)).collect())
            }

            (Kind::MapValue(mv), ValueType::Map { key_type, value_type, .. }) => {
                let map = mv.entries.iter().filter_map(|e| {
                    let k = e.key.as_ref().map(|k| decode_value(k, key_type))?;
                    let v = e.value.as_ref().map(|v| decode_value(v, value_type))?;
                    let mk = value_to_map_key(&k)?;
                    Some((mk, v))
                }).collect();
                Value::Map(map)
            }
            (Kind::MapValue(mv), _) => {
                let map = mv.entries.iter().filter_map(|e| {
                    let k = e.key.as_ref().map(|k| decode_value(k, &ValueType::Null))?;
                    let v = e.value.as_ref().map(|v| decode_value(v, &ValueType::Null))?;
                    let mk = value_to_map_key(&k)?;
                    Some((mk, v))
                }).collect();
                Value::Map(map)
            }
        }
    }

    fn map_key_to_value(k: &MapKey) -> Value {
        match k {
            MapKey::Bool(v) => Value::Bool(*v),
            MapKey::I8(v) => Value::I8(*v),
            MapKey::I16(v) => Value::I16(*v),
            MapKey::I32(v) => Value::I32(*v),
            MapKey::I64(v) => Value::I64(*v),
            MapKey::U8(v) => Value::U8(*v),
            MapKey::U16(v) => Value::U16(*v),
            MapKey::U32(v) => Value::U32(*v),
            MapKey::U64(v) => Value::U64(*v),
            MapKey::Date(v) => Value::Date(*v),
            MapKey::Uuid(v) => Value::Uuid(*v),
            MapKey::Ipv4(v) => Value::Ipv4(*v),
            MapKey::Ipv6(v) => Value::Ipv6(*v),
            MapKey::String(s) => Value::String(s.clone()),
            MapKey::Timestamp(v) => Value::Timestamp(*v),
            MapKey::TimestampTz(v, off) => Value::TimestampTz(*v, *off),
        }
    }

    fn value_to_map_key(v: &Value) -> Option<MapKey> {
        Some(match v {
            Value::Bool(b) => MapKey::Bool(*b),
            Value::I8(n) => MapKey::I8(*n),
            Value::I16(n) => MapKey::I16(*n),
            Value::I32(n) => MapKey::I32(*n),
            Value::I64(n) => MapKey::I64(*n),
            Value::U8(n) => MapKey::U8(*n),
            Value::U16(n) => MapKey::U16(*n),
            Value::U32(n) => MapKey::U32(*n),
            Value::U64(n) => MapKey::U64(*n),
            Value::Date(n) => MapKey::Date(*n),
            Value::Uuid(n) => MapKey::Uuid(*n),
            Value::Ipv4(n) => MapKey::Ipv4(*n),
            Value::Ipv6(n) => MapKey::Ipv6(*n),
            Value::String(s) => MapKey::String(s.clone()),
            Value::Timestamp(n) => MapKey::Timestamp(*n),
            Value::TimestampTz(n, off) => MapKey::TimestampTz(*n, *off),
            _ => return None,
        })
    }
}

pub use value_serde::{encode_value, decode_value};

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

    fn round_trip(value: Value, vt: ValueType) {
        let encoded = encode_value(&value, &vt);
        let decoded = decode_value(&encoded, &vt);
        assert_eq!(decoded, value, "round-trip failed for {vt:?}");
    }

    #[test]
    fn value_serde_scalars() {
        round_trip(Value::Null, ValueType::I64);
        round_trip(Value::Bool(true), ValueType::Bool);
        round_trip(Value::Bool(false), ValueType::Bool);
        round_trip(Value::I8(-42), ValueType::I8);
        round_trip(Value::I16(1000), ValueType::I16);
        round_trip(Value::I32(-100_000), ValueType::I32);
        round_trip(Value::I64(i64::MAX), ValueType::I64);
        round_trip(Value::U8(255), ValueType::U8);
        round_trip(Value::U16(60000), ValueType::U16);
        round_trip(Value::U32(4_000_000_000), ValueType::U32);
        round_trip(Value::U64(u64::MAX), ValueType::U64);
        round_trip(Value::F64(3.14), ValueType::F64);
        round_trip(Value::String("hello".into()), ValueType::String);
        round_trip(Value::Blob(vec![1, 2, 3]), ValueType::Blob);
        round_trip(Value::Date(19000), ValueType::Date);
        round_trip(Value::Timestamp(1_700_000_000_000), ValueType::Timestamp {
            precision: TimestampPrecision::Millis,
            timezone: TimestampTimezone::None,
        });
        round_trip(Value::TimestampTz(1_700_000_000, -300), ValueType::Timestamp {
            precision: TimestampPrecision::Seconds,
            timezone: TimestampTimezone::UtcOffset,
        });
        round_trip(Value::Uuid(0xdeadbeef_12345678_aabbccdd_eeff0011), ValueType::Uuid);
        round_trip(Value::Ipv4(0xc0a80101), ValueType::Ipv4);
        round_trip(Value::Ipv6(0xfe80_0000_0000_0000_0000_0000_0000_0001), ValueType::Ipv6);
        round_trip(Value::Enum(42), ValueType::Enum { values: vec![] });
        round_trip(Value::DecimalI64(12345), ValueType::Decimal { precision: 10, scale: 2 });
        round_trip(Value::DecimalI128(99999999999999999), ValueType::Decimal { precision: 30, scale: 5 });
    }

    #[test]
    fn value_serde_struct() {
        let vt = ValueType::Struct {
            fields: vec![
                StructField { name: "name".into(), human_name: "".into(), value_type: ValueType::String, nullable: false },
                StructField { name: "age".into(), human_name: "".into(), value_type: ValueType::I64, nullable: true },
            ],
        };
        let value = Value::Struct(vec![
            Value::String("alice".into()),
            Value::I64(30),
        ]);
        round_trip(value, vt.clone());

        let with_null = Value::Struct(vec![
            Value::String("bob".into()),
            Value::Null,
        ]);
        round_trip(with_null, vt);
    }

    #[test]
    fn value_serde_array() {
        let vt = ValueType::Array {
            element_type: Box::new(ValueType::I64),
            elements_nullable: false,
        };
        round_trip(Value::Array(vec![Value::I64(1), Value::I64(2), Value::I64(3)]), vt);
    }

    #[test]
    fn value_serde_map() {
        let vt = ValueType::Map {
            key_type: Box::new(ValueType::String),
            value_type: Box::new(ValueType::I64),
            values_nullable: false,
        };
        let mut map = std::collections::BTreeMap::new();
        map.insert(MapKey::String("a".into()), Value::I64(1));
        map.insert(MapKey::String("b".into()), Value::I64(2));
        round_trip(Value::Map(map), vt);
    }

    #[test]
    fn value_serde_f32_lossy() {
        let vt = ValueType::F32;
        let val = Value::F32(1.5);
        let encoded = encode_value(&val, &vt);
        let decoded = decode_value(&encoded, &vt);
        assert_eq!(decoded, Value::F32(1.5));
    }
}
