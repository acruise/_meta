//! Generate schema-specific .proto messages and JSON-to-proto conversion
//! code from an ExprSchema. The proto message is the wire format for
//! events entering the pipeline; the JSON converter runs at the ingest
//! boundary (webhook / Kafka consumer) and is not performance-critical.

use crate::type_check::ExprSchema;
use crate::value::ValueKind;

use prost_types::field_descriptor_proto::Type as ProtoType;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

/// Map a ValueKind to the protobuf field descriptor type.
pub fn value_kind_to_proto_type(kind: ValueKind) -> ProtoType {
    match kind {
        ValueKind::Bool => ProtoType::Bool,
        ValueKind::I8 | ValueKind::I16 | ValueKind::I32 | ValueKind::Date => ProtoType::Sint32,
        ValueKind::I64 | ValueKind::Timestamp | ValueKind::DecimalI64
        | ValueKind::TimestampTz => ProtoType::Sint64,
        ValueKind::U8 | ValueKind::U16 | ValueKind::U32 | ValueKind::Enum
        | ValueKind::Ipv4 => ProtoType::Uint32,
        ValueKind::U64 => ProtoType::Uint64,
        ValueKind::F32 => ProtoType::Float,
        ValueKind::F64 => ProtoType::Double,
        ValueKind::String => ProtoType::String,
        ValueKind::Blob | ValueKind::Clob | ValueKind::Uuid | ValueKind::Ipv6
        | ValueKind::DecimalI128 | ValueKind::Null | ValueKind::Array
        | ValueKind::Map | ValueKind::Struct | ValueKind::EntityRef => ProtoType::Bytes,
    }
}

fn proto_type_keyword(t: ProtoType) -> &'static str {
    match t {
        ProtoType::Bool => "bool",
        ProtoType::Sint32 => "sint32",
        ProtoType::Sint64 => "sint64",
        ProtoType::Uint32 => "uint32",
        ProtoType::Uint64 => "uint64",
        ProtoType::Float => "float",
        ProtoType::Double => "double",
        ProtoType::String => "string",
        ProtoType::Bytes => "bytes",
        ProtoType::Int32 => "int32",
        ProtoType::Int64 => "int64",
        ProtoType::Fixed32 => "fixed32",
        ProtoType::Fixed64 => "fixed64",
        ProtoType::Sfixed32 => "sfixed32",
        ProtoType::Sfixed64 => "sfixed64",
        ProtoType::Enum => "int32",
        ProtoType::Message | ProtoType::Group => "bytes",
    }
}

/// Generate a .proto file defining a message for the given event schema.
pub fn generate_event_proto(
    schema: &ExprSchema,
    message_name: &str,
    package: &str,
) -> String {
    let mut out = String::new();
    out.push_str("syntax = \"proto3\";\n\n");
    out.push_str(&format!("package {package};\n\n"));
    out.push_str(&format!("message {message_name} {{\n"));

    for (i, field) in schema.event_fields.iter().enumerate() {
        let proto_type = proto_type_keyword(value_kind_to_proto_type(field.kind));
        let tag = i + 1;
        out.push_str(&format!("  {proto_type} {} = {tag};\n", field.name));
    }

    out.push_str("}\n");
    out
}

/// Generate a .proto file with only the specified fields, preserving
/// original field numbers for wire compatibility.
pub fn generate_stripped_event_proto(
    schema: &ExprSchema,
    message_name: &str,
    package: &str,
    keep_fields: &[&str],
) -> String {
    let mut out = String::new();
    out.push_str("syntax = \"proto3\";\n\n");
    out.push_str(&format!("package {package};\n\n"));
    out.push_str(&format!("message {message_name} {{\n"));

    for (i, field) in schema.event_fields.iter().enumerate() {
        if keep_fields.contains(&field.name.as_str()) {
            let proto_type = proto_type_keyword(value_kind_to_proto_type(field.kind));
            let tag = i + 1;
            out.push_str(&format!("  {proto_type} {} = {tag};\n", field.name));
        }
    }

    out.push_str("}\n");
    out
}

fn value_kind_to_json_extract(kind: ValueKind, field_name: &str) -> TokenStream {
    let name = field_name;
    match kind {
        ValueKind::Bool => quote! {
            obj.get(#name).and_then(|v| v.as_bool()).unwrap_or_default()
        },
        ValueKind::I8 | ValueKind::I16 | ValueKind::I32 | ValueKind::Date => quote! {
            obj.get(#name).and_then(|v| v.as_i64()).unwrap_or_default() as i32
        },
        ValueKind::I64 | ValueKind::Timestamp | ValueKind::DecimalI64 | ValueKind::TimestampTz => quote! {
            obj.get(#name).and_then(|v| v.as_i64()).unwrap_or_default()
        },
        ValueKind::U8 | ValueKind::U16 | ValueKind::U32 | ValueKind::Enum => quote! {
            obj.get(#name).and_then(|v| v.as_u64()).unwrap_or_default() as u32
        },
        ValueKind::U64 => quote! {
            obj.get(#name).and_then(|v| v.as_u64()).unwrap_or_default()
        },
        ValueKind::F32 => quote! {
            obj.get(#name).and_then(|v| v.as_f64()).unwrap_or_default() as f32
        },
        ValueKind::F64 => quote! {
            obj.get(#name).and_then(|v| v.as_f64()).unwrap_or_default()
        },
        ValueKind::String => quote! {
            obj.get(#name).and_then(|v| v.as_str()).unwrap_or_default().to_string()
        },
        _ => quote! {
            obj.get(#name).map(|v| v.to_string().into_bytes()).unwrap_or_default()
        },
    }
}

/// Generate a Rust function that parses JSON into the proto struct.
/// The generated code assumes the proto struct exists with the given name.
pub fn generate_json_to_proto_fn(
    schema: &ExprSchema,
    message_name: &str,
) -> String {
    let struct_ident = Ident::new(message_name, Span::call_site());
    let fn_name = Ident::new(
        &format!("json_to_{}", to_snake_case(message_name)),
        Span::call_site(),
    );

    let field_assignments: Vec<TokenStream> = schema.event_fields.iter().map(|field| {
        let field_ident = Ident::new(&field.name, Span::call_site());
        let extract = value_kind_to_json_extract(field.kind, &field.name);
        quote! { #field_ident: #extract }
    }).collect();

    let tokens = quote! {
        fn #fn_name(json: &str) -> Result<#struct_ident, serde_json::Error> {
            let obj: serde_json::Value = serde_json::from_str(json)?;
            Ok(#struct_ident {
                #(#field_assignments),*
            })
        }
    };

    let file: syn::File = syn::parse2(tokens).expect("generated code should parse");
    prettyplease::unparse(&file)
}

fn to_snake_case(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_lowercase().next().unwrap());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::type_check::FieldSchema;

    fn order_schema() -> ExprSchema {
        ExprSchema {
            event_fields: vec![
                FieldSchema { name: "ts".into(), kind: ValueKind::I64, nullable: false },
                FieldSchema { name: "price".into(), kind: ValueKind::I64, nullable: false },
                FieldSchema { name: "qty".into(), kind: ValueKind::I64, nullable: false },
                FieldSchema { name: "status".into(), kind: ValueKind::String, nullable: false },
            ],
            enrichment_fields: vec![],
        }
    }

    #[test]
    fn proto_generation() {
        let proto = generate_event_proto(&order_schema(), "OrderEvent", "notochord.event.v1");
        assert!(proto.contains("syntax = \"proto3\""));
        assert!(proto.contains("package notochord.event.v1"));
        assert!(proto.contains("message OrderEvent {"));
        assert!(proto.contains("sint64 ts = 1;"));
        assert!(proto.contains("sint64 price = 2;"));
        assert!(proto.contains("sint64 qty = 3;"));
        assert!(proto.contains("string status = 4;"));
    }

    #[test]
    fn stripped_proto_preserves_field_numbers() {
        let proto = generate_stripped_event_proto(
            &order_schema(),
            "StrippedOrderEvent",
            "notochord.event.v1",
            &["ts", "price"],
        );
        assert!(proto.contains("sint64 ts = 1;"));
        assert!(proto.contains("sint64 price = 2;"));
        assert!(!proto.contains("qty"));
        assert!(!proto.contains("status"));
    }

    #[test]
    fn json_converter_generation() {
        let code = generate_json_to_proto_fn(&order_schema(), "OrderEvent");
        assert!(code.contains("fn json_to_order_event"));
        assert!(code.contains("serde_json::from_str"));
        assert!(code.contains("as_i64"));
        assert!(code.contains("as_str"));
    }

    #[test]
    fn mixed_types() {
        let schema = ExprSchema {
            event_fields: vec![
                FieldSchema { name: "active".into(), kind: ValueKind::Bool, nullable: false },
                FieldSchema { name: "score".into(), kind: ValueKind::F64, nullable: false },
                FieldSchema { name: "count".into(), kind: ValueKind::U32, nullable: false },
                FieldSchema { name: "name".into(), kind: ValueKind::String, nullable: false },
            ],
            enrichment_fields: vec![],
        };
        let proto = generate_event_proto(&schema, "MixedEvent", "test.v1");
        assert!(proto.contains("bool active = 1;"));
        assert!(proto.contains("double score = 2;"));
        assert!(proto.contains("uint32 count = 3;"));
        assert!(proto.contains("string name = 4;"));
    }
}
