//! Native UDF scaffolding.
//!
//! UDF authors write plain Rust functions with arbitrary input/output types.
//! The framework bridges those types to proto-encoded values via the
//! `ProtoSerde` trait. `Value` never appears in this module's public
//! interface -- the UDF boundary is proto `EncodedValue` only.
//!
//! The conversion chain for a UDF call:
//!
//!   EncodedValue -> (ProtoSerde::decode) -> Rust type
//!   Rust type -> (ProtoSerde::encode) -> EncodedValue
//!
//! The expression system is responsible for converting its internal
//! representation to/from `EncodedValue` before calling through this
//! interface.

use crate::cluonflux::meta as pb;
use crate::value::{ValueType, StructField};
use pb::encoded_value::Kind;

// ---------------------------------------------------------------------------
// ProtoSerde -- bidirectional conversion between Rust types and proto
// ---------------------------------------------------------------------------

pub trait ProtoSerde: Sized {
    fn value_type() -> ValueType;
    fn decode(v: &pb::EncodedValue) -> Option<Self>;
    fn encode(&self) -> pb::EncodedValue;
}

// ---------------------------------------------------------------------------
// Blanket impls -- common scalar types
// ---------------------------------------------------------------------------

impl ProtoSerde for String {
    fn value_type() -> ValueType { ValueType::String }
    fn decode(v: &pb::EncodedValue) -> Option<Self> {
        match &v.kind { Some(Kind::StringValue(s)) => Some(s.clone()), _ => None }
    }
    fn encode(&self) -> pb::EncodedValue {
        pb::EncodedValue { kind: Some(Kind::StringValue(self.clone())) }
    }
}

impl ProtoSerde for i64 {
    fn value_type() -> ValueType { ValueType::I64 }
    fn decode(v: &pb::EncodedValue) -> Option<Self> {
        match &v.kind { Some(Kind::IntValue(n)) => Some(*n), _ => None }
    }
    fn encode(&self) -> pb::EncodedValue {
        pb::EncodedValue { kind: Some(Kind::IntValue(*self)) }
    }
}

impl ProtoSerde for u64 {
    fn value_type() -> ValueType { ValueType::U64 }
    fn decode(v: &pb::EncodedValue) -> Option<Self> {
        match &v.kind { Some(Kind::UintValue(n)) => Some(*n), _ => None }
    }
    fn encode(&self) -> pb::EncodedValue {
        pb::EncodedValue { kind: Some(Kind::UintValue(*self)) }
    }
}

impl ProtoSerde for f64 {
    fn value_type() -> ValueType { ValueType::F64 }
    fn decode(v: &pb::EncodedValue) -> Option<Self> {
        match &v.kind { Some(Kind::FloatValue(n)) => Some(*n), _ => None }
    }
    fn encode(&self) -> pb::EncodedValue {
        pb::EncodedValue { kind: Some(Kind::FloatValue(*self)) }
    }
}

impl ProtoSerde for bool {
    fn value_type() -> ValueType { ValueType::Bool }
    fn decode(v: &pb::EncodedValue) -> Option<Self> {
        match &v.kind { Some(Kind::BoolValue(b)) => Some(*b), _ => None }
    }
    fn encode(&self) -> pb::EncodedValue {
        pb::EncodedValue { kind: Some(Kind::BoolValue(*self)) }
    }
}

impl<T: ProtoSerde> ProtoSerde for Option<T> {
    fn value_type() -> ValueType { T::value_type() }
    fn decode(v: &pb::EncodedValue) -> Option<Self> {
        if v.kind.is_none() { Some(None) } else { Some(T::decode(v)) }
    }
    fn encode(&self) -> pb::EncodedValue {
        match self {
            Some(v) => v.encode(),
            None => pb::EncodedValue { kind: None },
        }
    }
}

impl ProtoSerde for Vec<u8> {
    fn value_type() -> ValueType { ValueType::Blob }
    fn decode(v: &pb::EncodedValue) -> Option<Self> {
        match &v.kind { Some(Kind::BytesValue(b)) => Some(b.clone()), _ => None }
    }
    fn encode(&self) -> pb::EncodedValue {
        pb::EncodedValue { kind: Some(Kind::BytesValue(self.clone())) }
    }
}

// ---------------------------------------------------------------------------
// EncodedStruct -- for UDFs that return structured data
// ---------------------------------------------------------------------------

pub struct EncodedStruct {
    fields: Vec<pb::EncodedValue>,
    schema: Vec<StructField>,
}

impl EncodedStruct {
    pub fn new(schema: Vec<StructField>) -> Self {
        Self { fields: Vec::with_capacity(schema.len()), schema }
    }

    pub fn push<T: ProtoSerde>(&mut self, value: T) {
        self.fields.push(value.encode());
    }

    pub fn push_null(&mut self) {
        self.fields.push(pb::EncodedValue { kind: None });
    }

    pub fn schema(&self) -> &[StructField] {
        &self.schema
    }
}

impl ProtoSerde for EncodedStruct {
    fn value_type() -> ValueType {
        ValueType::Struct { fields: vec![] }
    }

    fn decode(_v: &pb::EncodedValue) -> Option<Self> {
        None
    }

    fn encode(&self) -> pb::EncodedValue {
        pb::EncodedValue {
            kind: Some(Kind::StructValue(pb::EncodedStructValue {
                fields: self.fields.clone(),
            })),
        }
    }
}

// ---------------------------------------------------------------------------
// UdfCatalogEntry -- mandatory metadata descriptor for a native UDF
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct UdfCatalogEntry {
    pub id: String,
    pub cel_name: String,
    pub description: String,
    pub params: Vec<UdfParam>,
    pub return_type: ValueType,
    pub notes: String,
}

#[derive(Debug, Clone)]
pub struct UdfParam {
    pub name: String,
    pub value_type: ValueType,
}

// ---------------------------------------------------------------------------
// NativeFn -- the type-erased UDF interface (proto only)
// ---------------------------------------------------------------------------

pub trait NativeFn: Send + Sync {
    fn catalog_entry(&self) -> &UdfCatalogEntry;
    fn call(&self, args: &[pb::EncodedValue]) -> pb::EncodedValue;
}

// Access metadata through catalog_entry() directly. Inherent methods
// on `dyn NativeFn` cause borrow-checker issues when the trait object
// is behind references (e.g. in closures passed to Iterator::find).

// ---------------------------------------------------------------------------
// Typed wrappers -- bridge plain Rust functions to NativeFn
// ---------------------------------------------------------------------------

pub struct NativeFn1<A, R, F> {
    pub entry: UdfCatalogEntry,
    pub func: F,
    pub _phantom: std::marker::PhantomData<fn(A) -> R>,
}

impl<A, R, F> NativeFn for NativeFn1<A, R, F>
where
    A: ProtoSerde + Send + Sync,
    R: ProtoSerde + Send + Sync,
    F: Fn(A) -> R + Send + Sync,
{
    fn catalog_entry(&self) -> &UdfCatalogEntry { &self.entry }

    fn call(&self, args: &[pb::EncodedValue]) -> pb::EncodedValue {
        let arg = match args.first().and_then(A::decode) {
            Some(a) => a,
            None => return pb::EncodedValue { kind: None },
        };
        (self.func)(arg).encode()
    }
}

pub struct NativeFn2<A1, A2, R, F> {
    pub entry: UdfCatalogEntry,
    pub func: F,
    pub _phantom: std::marker::PhantomData<fn(A1, A2) -> R>,
}

impl<A1, A2, R, F> NativeFn for NativeFn2<A1, A2, R, F>
where
    A1: ProtoSerde + Send + Sync,
    A2: ProtoSerde + Send + Sync,
    R: ProtoSerde + Send + Sync,
    F: Fn(A1, A2) -> R + Send + Sync,
{
    fn catalog_entry(&self) -> &UdfCatalogEntry { &self.entry }

    fn call(&self, args: &[pb::EncodedValue]) -> pb::EncodedValue {
        let a1 = match args.first().and_then(A1::decode) {
            Some(a) => a,
            None => return pb::EncodedValue { kind: None },
        };
        let a2 = match args.get(1).and_then(A2::decode) {
            Some(a) => a,
            None => return pb::EncodedValue { kind: None },
        };
        (self.func)(a1, a2).encode()
    }
}

pub fn native_fn_1<A: ProtoSerde, R: ProtoSerde>(
    entry: UdfCatalogEntry,
    func: impl Fn(A) -> R + Send + Sync,
) -> NativeFn1<A, R, impl Fn(A) -> R + Send + Sync> {
    NativeFn1 { entry, func, _phantom: std::marker::PhantomData }
}

pub fn native_fn_2<A1: ProtoSerde, A2: ProtoSerde, R: ProtoSerde>(
    entry: UdfCatalogEntry,
    func: impl Fn(A1, A2) -> R + Send + Sync,
) -> NativeFn2<A1, A2, R, impl Fn(A1, A2) -> R + Send + Sync> {
    NativeFn2 { entry, func, _phantom: std::marker::PhantomData }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn null() -> pb::EncodedValue { pb::EncodedValue { kind: None } }

    fn enc_string(s: &str) -> pb::EncodedValue {
        pb::EncodedValue { kind: Some(Kind::StringValue(s.to_string())) }
    }

    fn enc_i64(n: i64) -> pb::EncodedValue {
        pb::EncodedValue { kind: Some(Kind::IntValue(n)) }
    }

    fn test_entry(cel_name: &str, params: Vec<UdfParam>, return_type: ValueType) -> UdfCatalogEntry {
        UdfCatalogEntry {
            id: cel_name.into(),
            cel_name: cel_name.into(),
            description: String::new(),
            params,
            return_type,
            notes: String::new(),
        }
    }

    fn param(name: &str, vt: ValueType) -> UdfParam {
        UdfParam { name: name.into(), value_type: vt }
    }

    #[test]
    fn native_fn_1_string_to_i64() {
        let entry = test_entry("str_len", vec![param("s", ValueType::String)], ValueType::I64);
        let f = native_fn_1(entry, |s: String| -> i64 { s.len() as i64 });
        let nf: &dyn NativeFn = &f;
        assert_eq!(nf.catalog_entry().cel_name, "str_len");
        assert_eq!(nf.catalog_entry().params.iter().map(|p| p.value_type.clone()).collect::<Vec<_>>(), vec![ValueType::String]);
        assert_eq!(nf.catalog_entry().return_type, ValueType::I64);
        let result = f.call(&[enc_string("hello")]);
        assert_eq!(result.kind, Some(Kind::IntValue(5)));
    }

    #[test]
    fn native_fn_1_null_returns_null() {
        let entry = test_entry("str_len", vec![param("s", ValueType::String)], ValueType::I64);
        let f = native_fn_1(entry, |s: String| -> i64 { s.len() as i64 });
        assert_eq!(f.call(&[null()]).kind, None);
    }

    #[test]
    fn native_fn_1_wrong_type_returns_null() {
        let entry = test_entry("str_len", vec![param("s", ValueType::String)], ValueType::I64);
        let f = native_fn_1(entry, |s: String| -> i64 { s.len() as i64 });
        assert_eq!(f.call(&[enc_i64(42)]).kind, None);
    }

    #[test]
    fn native_fn_1_optional_arg() {
        let entry = test_entry("maybe_double", vec![param("n", ValueType::I64)], ValueType::I64);
        let f = native_fn_1(entry, |n: Option<i64>| -> Option<i64> {
            n.map(|v| v * 2)
        });
        assert_eq!(f.call(&[enc_i64(21)]).kind, Some(Kind::IntValue(42)));
        assert_eq!(f.call(&[null()]).kind, None);
    }

    #[test]
    fn native_fn_2_add() {
        let entry = test_entry("add", vec![param("a", ValueType::I64), param("b", ValueType::I64)], ValueType::I64);
        let f = native_fn_2(entry, |a: i64, b: i64| -> i64 { a + b });
        let nf: &dyn NativeFn = &f;
        assert_eq!(nf.catalog_entry().params.len(), 2);
        let result = f.call(&[enc_i64(3), enc_i64(7)]);
        assert_eq!(result.kind, Some(Kind::IntValue(10)));
    }

    #[test]
    fn native_fn_returning_struct() {
        let schema = vec![
            StructField { name: "browser".into(), human_name: "".into(), value_type: ValueType::String, nullable: false },
            StructField { name: "os".into(), human_name: "".into(), value_type: ValueType::String, nullable: false },
        ];
        let return_type = ValueType::Struct { fields: schema.clone() };
        let entry = test_entry("parse_ua", vec![param("agent", ValueType::String)], return_type.clone());

        let f = native_fn_1(entry, move |agent: String| -> EncodedStruct {
            let mut s = EncodedStruct::new(schema.clone());
            if agent.contains("Firefox") {
                s.push("Firefox".to_string());
            } else {
                s.push("Unknown".to_string());
            }
            s.push("Linux".to_string());
            s
        });

        let nf: &dyn NativeFn = &f;
        assert_eq!(nf.catalog_entry().return_type, return_type);

        let result = f.call(&[enc_string("Mozilla/5.0 Firefox/120")]);
        match &result.kind {
            Some(Kind::StructValue(sv)) => {
                assert_eq!(sv.fields.len(), 2);
                assert_eq!(sv.fields[0].kind, Some(Kind::StringValue("Firefox".into())));
                assert_eq!(sv.fields[1].kind, Some(Kind::StringValue("Linux".into())));
            }
            other => panic!("expected StructValue, got {other:?}"),
        }
    }

    #[test]
    fn encoded_struct_with_nulls() {
        let schema = vec![
            StructField { name: "x".into(), human_name: "".into(), value_type: ValueType::I64, nullable: true },
            StructField { name: "y".into(), human_name: "".into(), value_type: ValueType::String, nullable: true },
        ];
        let mut s = EncodedStruct::new(schema);
        s.push(42i64);
        s.push_null();

        let encoded = s.encode();
        match &encoded.kind {
            Some(Kind::StructValue(sv)) => {
                assert_eq!(sv.fields[0].kind, Some(Kind::IntValue(42)));
                assert_eq!(sv.fields[1].kind, None);
            }
            other => panic!("expected StructValue, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_through_proto() {
        let entry = test_entry("upper", vec![param("s", ValueType::String)], ValueType::String);
        let f = native_fn_1(entry, |s: String| -> String { s.to_uppercase() });

        let input = "hello".to_string();
        let encoded_arg = input.encode();
        let encoded_result = f.call(&[encoded_arg]);
        let output = String::decode(&encoded_result).unwrap();
        assert_eq!(output, "HELLO");
    }

    #[test]
    fn catalog_entry_accessible() {
        let entry = test_entry("my_fn", vec![param("x", ValueType::I64)], ValueType::Bool);
        let f = native_fn_1(entry, |_x: i64| -> bool { true });
        let e = f.catalog_entry();
        assert_eq!(e.id, "my_fn");
        assert_eq!(e.cel_name, "my_fn");
        assert_eq!(e.params.len(), 1);
        assert_eq!(e.params[0].name, "x");
        assert_eq!(e.return_type, ValueType::Bool);
    }
}
