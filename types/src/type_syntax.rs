//! A compact one-line syntax for `ValueType`: a type literal that fits in an
//! expression argument, an error message, or a YAML string scalar.
//!
//! ```text
//! type   := name                                 scalar keyword or declared type name
//!         | 'array' '<' elem '>'
//!         | 'map' '<' type ',' elem '>'
//!         | 'struct' '{' [field (',' field)*] '}'
//!         | 'enum' '{' member (',' member)* '}'
//!         | 'decimal' '(' int ',' int ')'
//!         | 'timestamp' '(' unit [',' zone] ')'
//!         | 'entity_ref' '<' type '>' '(' target [',' 'pinnable'] ')'
//! elem   := type ['?']                           '?': null elements / values allowed
//! field  := label ':' type ['?']                 '?': nullable field
//! member := label
//! target := label
//! label  := ident | quoted string ('...' or "...", backslash escapes)
//! name   := ident ('.' ident)*
//! unit   := s | ms | us | ns | seconds | millis | micros | nanos | unspecified
//! zone   := none | utc_offset
//! ```
//!
//! Brackets have one meaning each: `<...>` holds types, `(...)` holds
//! non-type parameters, `{...}` holds fields or members. Blank space is
//! allowed between tokens.
//!
//! `?` appears only where `ValueType` records nullability (array elements,
//! map values, struct fields); a top-level `i64?` has nothing to attach to
//! and is rejected rather than ignored.
//!
//! A bare name that is not a keyword is a reference to a declared type,
//! resolved through the caller's resolver. Keywords win over declared names,
//! as in the YAML codec. Bare `timestamp` and `decimal` are the same
//! convenience defaults the YAML codec uses (`timestamp(ms)`,
//! `decimal(38, 0)`).
//!
//! The `Display` impl on `ValueType` renders the canonical form, which
//! parses back to an equal type. The one thing the compact form does not
//! carry is a struct field's `human_name`: it parses as the field's name, so
//! a field whose human name differs from its name does not round-trip (use
//! the YAML form for that).

use std::fmt;

use crate::value::{StructField, TimestampPrecision, TimestampTimezone, ValueType};

/// A syntax or resolution error, with the byte offset where it was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeSyntaxError {
    pub position: usize,
    pub message: String,
}

impl fmt::Display for TypeSyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid type at offset {}: {}", self.position, self.message)
    }
}

impl std::error::Error for TypeSyntaxError {}

/// Parse a type with no declared names to resolve.
pub fn parse_type_literal(src: &str) -> Result<ValueType, TypeSyntaxError> {
    parse_type_literal_with(src, &|_| None)
}

/// Parse a type, resolving declared names with `resolve` (`None` means
/// unknown).
pub fn parse_type_literal_with(
    src: &str,
    resolve: &dyn Fn(&str) -> Option<ValueType>,
) -> Result<ValueType, TypeSyntaxError> {
    try_parse_type_literal_with(src, &|name| {
        resolve(name).ok_or_else(|| format!("unknown type `{name}`"))
    })
}

/// As [`parse_type_literal_with`], for a resolver with its own error message
/// (e.g. one that detects reference cycles).
pub fn try_parse_type_literal_with(
    src: &str,
    resolve: &dyn Fn(&str) -> Result<ValueType, String>,
) -> Result<ValueType, TypeSyntaxError> {
    let mut p = Parser { src, pos: 0, resolve };
    p.blank();
    let t = p.ty()?;
    p.blank();
    if p.pos != src.len() {
        if p.peek() == Some('?') {
            return p.err("'?' is only allowed on array elements, map values, and struct fields");
        }
        return p.err("unexpected trailing input");
    }
    Ok(t)
}

/// A type written in an expression: an argument such as the target of a
/// typed parse, carried on the IR node as a property rather than as a value.
///
/// Syntax is checked when the literal is created, so a malformed type is a
/// compile error at the point it was written. Declared type names, which
/// need the plan's type catalog, are resolved by a later pass; until then
/// the literal is `Unresolved` and keeps its text. A resolved literal holds
/// the full structural type, so a compiled plan does not depend on the
/// catalog revision it was resolved against.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeLiteral {
    /// Syntactically valid, references at least one declared name.
    Unresolved(String),
    Resolved(ValueType),
}

impl TypeLiteral {
    /// Check `text` and resolve it immediately if it names no declared type.
    pub fn parse(text: &str) -> Result<TypeLiteral, TypeSyntaxError> {
        let names = std::cell::Cell::new(false);
        let t = try_parse_type_literal_with(text, &|_| {
            names.set(true);
            Ok(ValueType::Null)
        })?;
        Ok(if names.get() { TypeLiteral::Unresolved(text.to_string()) } else { TypeLiteral::Resolved(t) })
    }

    pub fn resolved(&self) -> Option<&ValueType> {
        match self {
            TypeLiteral::Resolved(t) => Some(t),
            TypeLiteral::Unresolved(_) => None,
        }
    }

    /// Resolve declared names, in place. A no-op when already resolved.
    pub fn resolve(
        &mut self,
        resolve: &dyn Fn(&str) -> Option<ValueType>,
    ) -> Result<&ValueType, TypeSyntaxError> {
        if let TypeLiteral::Unresolved(text) = self {
            *self = TypeLiteral::Resolved(parse_type_literal_with(text, resolve)?);
        }
        Ok(self.resolved().expect("just resolved"))
    }
}

impl fmt::Display for TypeLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeLiteral::Unresolved(text) => f.write_str(text),
            TypeLiteral::Resolved(t) => write!(f, "{t}"),
        }
    }
}

/// Bare scalar keywords, and the convenience defaults for `timestamp` and
/// `decimal`. `None` means "not a keyword".
pub fn scalar_keyword(s: &str) -> Option<ValueType> {
    Some(match s {
        "bool" | "boolean" => ValueType::Bool,
        "i8" => ValueType::I8,
        "i16" => ValueType::I16,
        "i32" => ValueType::I32,
        "i64" | "int" => ValueType::I64,
        "u8" => ValueType::U8,
        "u16" => ValueType::U16,
        "u32" => ValueType::U32,
        "u64" | "uint" => ValueType::U64,
        "f32" => ValueType::F32,
        "f64" | "double" | "float" => ValueType::F64,
        "string" => ValueType::String,
        "blob" | "bytes" => ValueType::Blob,
        "clob" => ValueType::Clob,
        "date" => ValueType::Date,
        "uuid" => ValueType::Uuid,
        "ipv4" => ValueType::Ipv4,
        "ipv6" => ValueType::Ipv6,
        "null" => ValueType::Null,
        "json" => ValueType::Json,
        "timestamp" => ValueType::Timestamp {
            precision: TimestampPrecision::Millis,
            timezone: TimestampTimezone::None,
        },
        "decimal" => ValueType::Decimal { precision: 38, scale: 0 },
        _ => return None,
    })
}

pub fn timestamp_precision(s: &str) -> Option<TimestampPrecision> {
    Some(match s {
        "unspecified" => TimestampPrecision::Unspecified,
        "seconds" | "s" => TimestampPrecision::Seconds,
        "millis" | "ms" => TimestampPrecision::Millis,
        "micros" | "us" => TimestampPrecision::Micros,
        "nanos" | "ns" => TimestampPrecision::Nanos,
        _ => return None,
    })
}

pub fn timestamp_timezone(s: &str) -> Option<TimestampTimezone> {
    Some(match s {
        "none" => TimestampTimezone::None,
        "utc_offset" => TimestampTimezone::UtcOffset,
        _ => return None,
    })
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    resolve: &'a dyn Fn(&str) -> Result<ValueType, String>,
}

type PResult<T> = Result<T, TypeSyntaxError>;

impl<'a> Parser<'a> {
    fn err<T>(&self, message: impl Into<String>) -> PResult<T> {
        Err(TypeSyntaxError { position: self.pos, message: message.into() })
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn blank(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, c: char) -> bool {
        self.blank();
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> PResult<()> {
        if self.eat(c) { Ok(()) } else { self.err(format!("expected '{c}'")) }
    }

    fn ident(&mut self) -> PResult<&'a str> {
        self.blank();
        let start = self.pos;
        match self.peek() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => self.pos += 1,
            _ => return self.err("expected an identifier"),
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == '_') {
            self.pos += 1;
        }
        let src: &'a str = self.src;
        Ok(&src[start..self.pos])
    }

    /// `ident ('.' ident)*`
    fn name(&mut self) -> PResult<String> {
        let mut name = self.ident()?.to_string();
        while self.peek() == Some('.') {
            self.pos += 1;
            name.push('.');
            name.push_str(self.ident()?);
        }
        Ok(name)
    }

    fn label(&mut self) -> PResult<String> {
        self.blank();
        match self.peek() {
            Some(q @ ('\'' | '"')) => {
                self.pos += 1;
                let mut out = String::new();
                loop {
                    match self.peek() {
                        None => return self.err("unterminated string"),
                        Some(c) if c == q => {
                            self.pos += 1;
                            return Ok(out);
                        }
                        Some('\\') => {
                            self.pos += 1;
                            match self.peek() {
                                Some(c) => {
                                    self.pos += c.len_utf8();
                                    out.push(c);
                                }
                                None => return self.err("unterminated escape"),
                            }
                        }
                        Some(c) => {
                            self.pos += c.len_utf8();
                            out.push(c);
                        }
                    }
                }
            }
            _ => Ok(self.ident()?.to_string()),
        }
    }

    fn uint(&mut self) -> PResult<u32> {
        self.blank();
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        self.src[start..self.pos].parse().or_else(|_| {
            self.pos = start;
            self.err("expected a non-negative integer")
        })
    }

    /// A type followed by an optional '?'.
    fn nullable_ty(&mut self) -> PResult<(ValueType, bool)> {
        let t = self.ty()?;
        Ok((t, self.eat('?')))
    }

    fn ty(&mut self) -> PResult<ValueType> {
        self.blank();
        let start = self.pos;
        let word = self.name()?;
        match word.as_str() {
            "array" => {
                self.expect('<')?;
                let (element, nullable) = self.nullable_ty()?;
                self.expect('>')?;
                Ok(ValueType::Array { element_type: Box::new(element), elements_nullable: nullable })
            }
            "map" => {
                self.expect('<')?;
                let key = self.ty()?;
                self.expect(',')?;
                let (value, nullable) = self.nullable_ty()?;
                self.expect('>')?;
                Ok(ValueType::Map {
                    key_type: Box::new(key),
                    value_type: Box::new(value),
                    values_nullable: nullable,
                })
            }
            "struct" => {
                self.expect('{')?;
                let mut fields = Vec::new();
                if !self.eat('}') {
                    loop {
                        let name = self.label()?;
                        if fields.iter().any(|f: &StructField| f.name == name) {
                            return self.err(format!("duplicate field `{name}`"));
                        }
                        self.expect(':')?;
                        let (value_type, nullable) = self.nullable_ty()?;
                        fields.push(StructField { human_name: name.clone(), name, value_type, nullable });
                        if self.eat(',') {
                            continue;
                        }
                        self.expect('}')?;
                        break;
                    }
                }
                Ok(ValueType::Struct { fields })
            }
            "enum" => {
                self.expect('{')?;
                let mut values: Vec<String> = Vec::new();
                loop {
                    let v = self.label()?;
                    if values.contains(&v) {
                        return self.err(format!("duplicate enum member `{v}`"));
                    }
                    values.push(v);
                    if self.eat(',') {
                        continue;
                    }
                    self.expect('}')?;
                    break;
                }
                Ok(ValueType::Enum { values })
            }
            "entity_ref" => {
                self.expect('<')?;
                let key = self.ty()?;
                self.expect('>')?;
                self.expect('(')?;
                let target = self.label()?;
                let pinnable = if self.eat(',') {
                    match self.ident()? {
                        "pinnable" => true,
                        other => return self.err(format!("expected `pinnable`, found `{other}`")),
                    }
                } else {
                    false
                };
                self.expect(')')?;
                Ok(ValueType::EntityRef { target_type_id: target, key_type: Box::new(key), revision_pinnable: pinnable })
            }
            "decimal" if self.eat('(') => {
                let precision = self.uint()?;
                self.expect(',')?;
                let scale = self.uint()?;
                self.expect(')')?;
                if scale > precision {
                    return self.err(format!("decimal scale {scale} exceeds precision {precision}"));
                }
                Ok(ValueType::Decimal { precision, scale })
            }
            "timestamp" if self.eat('(') => {
                let unit = self.ident()?;
                let precision = match timestamp_precision(unit) {
                    Some(p) => p,
                    None => return self.err(format!("unknown timestamp unit `{unit}`")),
                };
                let timezone = if self.eat(',') {
                    let zone = self.ident()?;
                    match timestamp_timezone(zone) {
                        Some(z) => z,
                        None => return self.err(format!("unknown timestamp zone `{zone}`")),
                    }
                } else {
                    TimestampTimezone::None
                };
                self.expect(')')?;
                Ok(ValueType::Timestamp { precision, timezone })
            }
            _ => {
                if let Some(t) = scalar_keyword(&word) {
                    return Ok(t);
                }
                (self.resolve)(&word).map_err(|message| TypeSyntaxError { position: start, message })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A label that must not be mistaken for anything else is quoted.
fn write_label(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    if is_ident(s) {
        f.write_str(s)
    } else {
        f.write_str("'")?;
        for c in s.chars() {
            if c == '\'' || c == '\\' {
                f.write_str("\\")?;
            }
            write!(f, "{c}")?;
        }
        f.write_str("'")
    }
}

fn q(nullable: bool) -> &'static str {
    if nullable { "?" } else { "" }
}

fn unit_str(p: TimestampPrecision) -> &'static str {
    match p {
        TimestampPrecision::Unspecified => "unspecified",
        TimestampPrecision::Seconds => "s",
        TimestampPrecision::Millis => "ms",
        TimestampPrecision::Micros => "us",
        TimestampPrecision::Nanos => "ns",
    }
}

impl fmt::Display for ValueType {
    /// The canonical compact form; see the module docs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueType::Null => f.write_str("null"),
            ValueType::Bool => f.write_str("bool"),
            ValueType::I8 => f.write_str("i8"),
            ValueType::I16 => f.write_str("i16"),
            ValueType::I32 => f.write_str("i32"),
            ValueType::I64 => f.write_str("i64"),
            ValueType::U8 => f.write_str("u8"),
            ValueType::U16 => f.write_str("u16"),
            ValueType::U32 => f.write_str("u32"),
            ValueType::U64 => f.write_str("u64"),
            ValueType::F32 => f.write_str("f32"),
            ValueType::F64 => f.write_str("f64"),
            ValueType::Date => f.write_str("date"),
            ValueType::Uuid => f.write_str("uuid"),
            ValueType::Ipv4 => f.write_str("ipv4"),
            ValueType::Ipv6 => f.write_str("ipv6"),
            ValueType::Blob => f.write_str("blob"),
            ValueType::Clob => f.write_str("clob"),
            ValueType::String => f.write_str("string"),
            ValueType::Json => f.write_str("json"),
            ValueType::Decimal { precision, scale } => write!(f, "decimal({precision}, {scale})"),
            ValueType::Timestamp { precision, timezone } => match timezone {
                TimestampTimezone::None => write!(f, "timestamp({})", unit_str(*precision)),
                TimestampTimezone::UtcOffset => write!(f, "timestamp({}, utc_offset)", unit_str(*precision)),
            },
            ValueType::Enum { values } => {
                f.write_str("enum{")?;
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write_label(f, v)?;
                }
                f.write_str("}")
            }
            ValueType::Array { element_type, elements_nullable } => {
                write!(f, "array<{element_type}{}>", q(*elements_nullable))
            }
            ValueType::Map { key_type, value_type, values_nullable } => {
                write!(f, "map<{key_type}, {value_type}{}>", q(*values_nullable))
            }
            ValueType::Struct { fields } => {
                f.write_str("struct{")?;
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write_label(f, &field.name)?;
                    write!(f, ": {}{}", field.value_type, q(field.nullable))?;
                }
                f.write_str("}")
            }
            ValueType::EntityRef { target_type_id, key_type, revision_pinnable } => {
                write!(f, "entity_ref<{key_type}>(")?;
                write_label(f, target_type_id)?;
                f.write_str(if *revision_pinnable { ", pinnable)" } else { ")" })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> ValueType {
        parse_type_literal(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn rejects(s: &str) {
        assert!(parse_type_literal(s).is_err(), "should reject {s:?}");
    }

    #[test]
    fn scalars_and_aliases() {
        assert_eq!(parse("i64"), ValueType::I64);
        assert_eq!(parse(" int "), ValueType::I64);
        assert_eq!(parse("json"), ValueType::Json);
        assert_eq!(parse("timestamp"), parse("timestamp(ms)"));
        assert_eq!(parse("decimal"), parse("decimal(38, 0)"));
    }

    #[test]
    fn compounds() {
        assert_eq!(
            parse("map<string, array<i64?>?>"),
            ValueType::Map {
                key_type: Box::new(ValueType::String),
                value_type: Box::new(ValueType::Array {
                    element_type: Box::new(ValueType::I64),
                    elements_nullable: true,
                }),
                values_nullable: true,
            }
        );
        let ValueType::Struct { fields } = parse("struct{ id: u64, 'full name': string?, at: timestamp(us, utc_offset) }")
        else {
            panic!()
        };
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[1].name, "full name");
        assert!(fields[1].nullable && !fields[0].nullable);
        assert_eq!(parse("struct{}"), ValueType::Struct { fields: vec![] });
        assert_eq!(parse("enum{red, 'dark green'}"), ValueType::Enum { values: vec!["red".into(), "dark green".into()] });
        assert_eq!(
            parse("entity_ref<uuid>(user, pinnable)"),
            ValueType::EntityRef { target_type_id: "user".into(), key_type: Box::new(ValueType::Uuid), revision_pinnable: true }
        );
    }

    #[test]
    fn declared_names_resolve() {
        let resolve = |n: &str| (n == "acme.Point").then(|| parse("struct{x: f64, y: f64}"));
        let t = parse_type_literal_with("array<acme.Point>", &resolve).unwrap();
        assert_eq!(t.to_string(), "array<struct{x: f64, y: f64}>");
        let e = parse_type_literal_with("array<Nope>", &resolve).unwrap_err();
        assert_eq!(e.position, 6);
    }

    #[test]
    fn type_literal_defers_only_declared_names() {
        assert_eq!(TypeLiteral::parse("array<i64>").unwrap(), TypeLiteral::Resolved(parse("array<i64>")));
        let mut t = TypeLiteral::parse("map<string, Order>").unwrap();
        assert_eq!(t, TypeLiteral::Unresolved("map<string, Order>".into()));
        assert!(TypeLiteral::parse("map<string").is_err());
        let order = parse("struct{id: u64}");
        let resolved = t.resolve(&|n| (n == "Order").then(|| order.clone())).unwrap().clone();
        assert_eq!(resolved.to_string(), "map<string, struct{id: u64}>");
        assert!(TypeLiteral::parse("Nope").unwrap().resolve(&|_| None).is_err());
    }

    #[test]
    fn rejections() {
        for s in [
            "", "i64?", "array<i64", "array<>", "map<string>", "decimal(2, 3)", "decimal(18)",
            "timestamp(fortnights)", "timestamp(ms, mars)", "struct{a: i64, a: string}",
            "enum{}", "enum{a, a}", "entity_ref<uuid>(user, maybe)", "Unknown", "array<i64> x",
            "struct{a i64}", "'quoted'",
        ] {
            rejects(s);
        }
    }

    #[test]
    fn canonical_rendering_round_trips() {
        for s in [
            "null", "bool", "i8", "u64", "f32", "date", "uuid", "ipv6", "blob", "clob", "string", "json",
            "decimal(18, 2)", "timestamp(ns)", "timestamp(s, utc_offset)", "timestamp(unspecified)",
            "enum{red, 'dark green', 'it\\'s'}", "array<i64>", "array<json?>", "map<i32, string?>",
            "struct{a: i64, 'b c': array<struct{d: date?}>?}", "entity_ref<string>(order)",
            "entity_ref<uuid>('user type', pinnable)",
        ] {
            let t = parse(s);
            assert_eq!(t.to_string(), s, "canonical form of {s}");
            assert_eq!(parse(&t.to_string()), t);
        }
    }
}
