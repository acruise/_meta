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
    String(String),
    Timestamp(i64),
    TimestampTz(i64, i16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueKind {
    Null,
    Bool,
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    F32, F64,
    Date,
    Uuid,
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
