//! CEL text → LogExpr translation with partial raising.
//!
//! Parses CEL source into the `cel_parser` AST, then walks it to produce
//! a `LogExpr` tree. Functions and operators that appear in the function
//! catalog become native IR nodes; unrecognized constructs become `CelUdf`
//! leaves that the evaluator delegates to the CEL runtime.
//!
//! The translator always succeeds if the CEL parses — there are no
//! "unsupported" errors, only varying degrees of raising.

use std::cell::Cell;

use cel_parser::{self, Expression, Atom, Member, RelationOp, ArithmeticOp, UnaryOp};

use crate::expr_gen::LogExpr;
use crate::value::Value;

#[derive(Debug)]
pub enum CelConvertError {
    Parse(cel_parser::error::ParseError),
    InvalidArgCount { function: String, expected: usize, got: usize },
}

impl std::fmt::Display for CelConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "CEL parse error: {e}"),
            Self::InvalidArgCount { function, expected, got } =>
                write!(f, "{function}: expected {expected} args, got {got}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslationStatus {
    FullyRaised,
    PartiallyRaised,
    Opaque,
}

#[derive(Debug)]
pub struct CelTranslation {
    pub expr: LogExpr,
    pub status: TranslationStatus,
}

pub fn cel_to_log_expr(source: &str) -> Result<CelTranslation, CelConvertError> {
    let ast = cel_parser::parse(source).map_err(CelConvertError::Parse)?;
    let has_udf = Cell::new(false);
    let expr = convert(&ast, &has_udf)?;
    let is_root_udf = matches!(&expr, LogExpr::CelUdf { .. });
    let status = if !has_udf.get() {
        TranslationStatus::FullyRaised
    } else if is_root_udf {
        TranslationStatus::Opaque
    } else {
        TranslationStatus::PartiallyRaised
    };
    Ok(CelTranslation { expr, status })
}

fn convert(expr: &Expression, has_udf: &Cell<bool>) -> Result<LogExpr, CelConvertError> {
    match expr {
        Expression::Atom(atom) => Ok(convert_atom(atom)),
        Expression::Ident(name) => Ok(LogExpr::GetFieldByName { field_name: name.to_string() }),

        Expression::Or(lhs, rhs) => Ok(LogExpr::LogicalOr {
            lhs: boxed(convert(lhs, has_udf)?),
            rhs: boxed(convert(rhs, has_udf)?),
        }),
        Expression::And(lhs, rhs) => Ok(LogExpr::LogicalAnd {
            lhs: boxed(convert(lhs, has_udf)?),
            rhs: boxed(convert(rhs, has_udf)?),
        }),
        Expression::Unary(op, operand) => convert_unary(op, operand, has_udf),
        Expression::Relation(lhs, op, rhs) => convert_relation(lhs, op, rhs, has_udf),
        Expression::Arithmetic(lhs, op, rhs) => convert_arithmetic(lhs, op, rhs, has_udf),
        Expression::Ternary(cond, then_expr, else_expr) => Ok(LogExpr::Conditional {
            condition: boxed(convert(cond, has_udf)?),
            then_expr: boxed(convert(then_expr, has_udf)?),
            else_expr: boxed(convert(else_expr, has_udf)?),
        }),

        Expression::Member(base, member) => convert_member(base, member, has_udf),
        Expression::FunctionCall(name_expr, target, args) =>
            convert_function_call(name_expr, target, args, has_udf),

        Expression::List(items) => {
            let values: Result<Vec<Value>, ()> = items.iter().map(convert_to_literal_value).collect();
            match values {
                Ok(vals) => Ok(LogExpr::Literal(Value::Array(vals))),
                Err(()) => Ok(make_udf(expr, has_udf)),
            }
        }

        Expression::Map(_) => Ok(make_udf(expr, has_udf)),
    }
}

fn make_udf(expr: &Expression, has_udf: &Cell<bool>) -> LogExpr {
    has_udf.set(true);
    let refs = expr.references();
    let args: Vec<Box<LogExpr>> = refs.variables().into_iter()
        .map(|v| Box::new(LogExpr::GetFieldByName { field_name: v.to_string() }))
        .collect();
    LogExpr::CelUdf {
        source: format_cel(expr),
        args,
    }
}

fn convert_atom(atom: &Atom) -> LogExpr {
    match atom {
        Atom::Int(n) => LogExpr::Literal(Value::I64(*n)),
        Atom::UInt(n) => LogExpr::Literal(Value::U64(*n)),
        Atom::Float(n) => LogExpr::Literal(Value::F64(*n)),
        Atom::String(s) => LogExpr::Literal(Value::String(s.to_string())),
        Atom::Bytes(b) => LogExpr::Literal(Value::Blob(b.to_vec())),
        Atom::Bool(b) => LogExpr::Literal(Value::Bool(*b)),
        Atom::Null => LogExpr::Literal(Value::Null),
    }
}

fn convert_to_literal_value(expr: &Expression) -> Result<Value, ()> {
    match expr {
        Expression::Atom(atom) => match atom {
            Atom::Int(n) => Ok(Value::I64(*n)),
            Atom::UInt(n) => Ok(Value::U64(*n)),
            Atom::Float(n) => Ok(Value::F64(*n)),
            Atom::String(s) => Ok(Value::String(s.to_string())),
            Atom::Bytes(b) => Ok(Value::Blob(b.to_vec())),
            Atom::Bool(b) => Ok(Value::Bool(*b)),
            Atom::Null => Ok(Value::Null),
        },
        Expression::List(items) => {
            let vals: Result<Vec<Value>, ()> = items.iter().map(convert_to_literal_value).collect();
            Ok(Value::Array(vals?))
        }
        _ => Err(()),
    }
}

fn convert_unary(op: &UnaryOp, operand: &Expression, has_udf: &Cell<bool>) -> Result<LogExpr, CelConvertError> {
    let inner = convert(operand, has_udf)?;
    match op {
        UnaryOp::Not => Ok(LogExpr::LogicalNot { operand: boxed(inner) }),
        UnaryOp::DoubleNot => Ok(LogExpr::LogicalNot {
            operand: boxed(LogExpr::LogicalNot { operand: boxed(inner) }),
        }),
        UnaryOp::Minus => Ok(LogExpr::Negate { operand: boxed(inner) }),
        UnaryOp::DoubleMinus => Ok(LogExpr::Negate {
            operand: boxed(LogExpr::Negate { operand: boxed(inner) }),
        }),
    }
}

fn convert_relation(lhs: &Expression, op: &RelationOp, rhs: &Expression, has_udf: &Cell<bool>) -> Result<LogExpr, CelConvertError> {
    let l = boxed(convert(lhs, has_udf)?);
    let r = boxed(convert(rhs, has_udf)?);
    Ok(match op {
        RelationOp::Equals => LogExpr::Equal { lhs: l, rhs: r },
        RelationOp::NotEquals => LogExpr::NotEqual { lhs: l, rhs: r },
        RelationOp::LessThan => LogExpr::LessThan { lhs: l, rhs: r },
        RelationOp::LessThanEq => LogExpr::LessOrEqual { lhs: l, rhs: r },
        RelationOp::GreaterThan => LogExpr::GreaterThan { lhs: l, rhs: r },
        RelationOp::GreaterThanEq => LogExpr::GreaterOrEqual { lhs: l, rhs: r },
        RelationOp::In => LogExpr::In { lhs: l, rhs: r },
    })
}

fn convert_arithmetic(lhs: &Expression, op: &ArithmeticOp, rhs: &Expression, has_udf: &Cell<bool>) -> Result<LogExpr, CelConvertError> {
    let l = boxed(convert(lhs, has_udf)?);
    let r = boxed(convert(rhs, has_udf)?);
    Ok(match op {
        ArithmeticOp::Add => LogExpr::Add { lhs: l, rhs: r },
        ArithmeticOp::Subtract => LogExpr::Subtract { lhs: l, rhs: r },
        ArithmeticOp::Multiply => LogExpr::Multiply { lhs: l, rhs: r },
        ArithmeticOp::Divide => LogExpr::Divide { lhs: l, rhs: r },
        ArithmeticOp::Modulus => LogExpr::Modulus { lhs: l, rhs: r },
    })
}

fn convert_member(base: &Expression, member: &Member, has_udf: &Cell<bool>) -> Result<LogExpr, CelConvertError> {
    match member {
        Member::Attribute(name) => {
            let parent = convert(base, has_udf)?;
            Ok(LogExpr::GetChildByName {
                child_name: name.to_string(),
                operand: boxed(parent),
            })
        }
        Member::Index(idx_expr) => {
            let parent = convert(base, has_udf)?;
            let idx = convert(idx_expr, has_udf)?;
            Ok(LogExpr::Index { lhs: boxed(parent), rhs: boxed(idx) })
        }
        Member::Fields(_) => Ok(make_udf(base, has_udf)),
    }
}

fn convert_function_call(
    name_expr: &Expression,
    target: &Option<Box<Expression>>,
    args: &[Expression],
    has_udf: &Cell<bool>,
) -> Result<LogExpr, CelConvertError> {
    let name = match name_expr {
        Expression::Ident(n) => n.as_str(),
        _ => {
            let full = Expression::FunctionCall(
                Box::new(name_expr.clone()),
                target.clone(),
                args.to_vec(),
            );
            return Ok(make_udf(&full, has_udf));
        }
    };

    let full_expr = Expression::FunctionCall(
        Box::new(name_expr.clone()),
        target.clone(),
        args.to_vec(),
    );

    match target {
        None => convert_free_function(name, args, has_udf, &full_expr),
        Some(receiver) => convert_method_call(name, receiver, args, has_udf, &full_expr),
    }
}

fn convert_free_function(
    name: &str,
    args: &[Expression],
    has_udf: &Cell<bool>,
    full_expr: &Expression,
) -> Result<LogExpr, CelConvertError> {
    match name {
        "size" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::Size { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "bool" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::CastBool { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "int" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::CastInt { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "uint" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::CastUint { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "double" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::CastDouble { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "string" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::CastString { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "bytes" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::CastBytes { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "duration" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::CastDuration { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "timestamp" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::CastTimestamp { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "type" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::TypeOf { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "dyn" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::Dyn { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "has" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::Has { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "cidr_contains" => {
            expect_args(name, 2, args)?;
            Ok(LogExpr::CidrContains {
                lhs: boxed(convert(&args[0], has_udf)?),
                rhs: boxed(convert(&args[1], has_udf)?),
            })
        }
        "cidr_match" => {
            expect_args(name, 2, args)?;
            Ok(LogExpr::CidrMatch {
                lhs: boxed(convert(&args[0], has_udf)?),
                rhs: boxed(convert(&args[1], has_udf)?),
            })
        }
        "ip_to_int" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::IpToInt { operand: boxed(convert(&args[0], has_udf)?) })
        }
        "int_to_ip" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::IntToIp { operand: boxed(convert(&args[0], has_udf)?) })
        }
        _ => Ok(make_udf(full_expr, has_udf)),
    }
}

fn convert_method_call(
    name: &str,
    receiver: &Expression,
    args: &[Expression],
    has_udf: &Cell<bool>,
    full_expr: &Expression,
) -> Result<LogExpr, CelConvertError> {
    match name {
        "contains" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::Contains {
                receiver: boxed(convert(receiver, has_udf)?),
                arg: boxed(convert(&args[0], has_udf)?),
            })
        }
        "startsWith" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::StartsWith {
                receiver: boxed(convert(receiver, has_udf)?),
                arg: boxed(convert(&args[0], has_udf)?),
            })
        }
        "endsWith" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::EndsWith {
                receiver: boxed(convert(receiver, has_udf)?),
                arg: boxed(convert(&args[0], has_udf)?),
            })
        }
        "matches" => {
            expect_args(name, 1, args)?;
            Ok(LogExpr::RegexMatch {
                receiver: boxed(convert(receiver, has_udf)?),
                arg: boxed(convert(&args[0], has_udf)?),
            })
        }
        "size" => {
            expect_args(name, 0, args)?;
            Ok(LogExpr::Size { operand: boxed(convert(receiver, has_udf)?) })
        }
        "all" | "exists" | "exists_one" | "filter" | "map" => {
            convert_hof(name, receiver, args, has_udf)
        }
        _ => Ok(make_udf(full_expr, has_udf)),
    }
}

fn convert_hof(
    name: &str,
    receiver: &Expression,
    args: &[Expression],
    has_udf: &Cell<bool>,
) -> Result<LogExpr, CelConvertError> {
    expect_args(name, 2, args)?;
    let collection = boxed(convert(receiver, has_udf)?);

    let binding = match &args[0] {
        Expression::Ident(s) => s.to_string(),
        _ => return Err(CelConvertError::InvalidArgCount {
            function: format!("{name}: first argument must be an identifier"),
            expected: 2,
            got: args.len(),
        }),
    };

    let body = boxed(convert(&args[1], has_udf)?);

    Ok(match name {
        "all" => LogExpr::All { collection, binding, body },
        "exists" => LogExpr::Exists { collection, binding, body },
        "exists_one" => LogExpr::ExistsOne { collection, binding, body },
        "filter" => LogExpr::Filter { collection, binding, body },
        "map" => LogExpr::MapTransform { collection, binding, body },
        _ => unreachable!(),
    })
}

fn expect_args(name: &str, expected: usize, args: &[Expression]) -> Result<(), CelConvertError> {
    if args.len() != expected {
        return Err(CelConvertError::InvalidArgCount {
            function: name.to_string(),
            expected,
            got: args.len(),
        });
    }
    Ok(())
}

fn boxed(expr: LogExpr) -> Box<LogExpr> {
    Box::new(expr)
}

// --- Minimal CEL formatter ---
// Reconstructs recognizable CEL text from a parsed AST.
// Not a full round-trip printer — just enough for debugging CelUdf source fields.

fn format_cel(expr: &Expression) -> String {
    match expr {
        Expression::Atom(atom) => match atom {
            Atom::Int(n) => n.to_string(),
            Atom::UInt(n) => format!("{n}u"),
            Atom::Float(n) => format!("{n:?}"),
            Atom::String(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
            Atom::Bytes(b) => format!("b'{}'", String::from_utf8_lossy(b)),
            Atom::Bool(b) => b.to_string(),
            Atom::Null => "null".to_string(),
        },
        Expression::Ident(name) => name.to_string(),
        Expression::Or(l, r) => format!("{} || {}", format_cel(l), format_cel(r)),
        Expression::And(l, r) => format!("{} && {}", format_cel(l), format_cel(r)),
        Expression::Unary(op, e) => {
            let inner = format_cel(e);
            match op {
                UnaryOp::Not => format!("!{inner}"),
                UnaryOp::DoubleNot => format!("!!{inner}"),
                UnaryOp::Minus => format!("-{inner}"),
                UnaryOp::DoubleMinus => format!("--{inner}"),
            }
        }
        Expression::Relation(l, op, r) => {
            let op_str = match op {
                RelationOp::Equals => "==",
                RelationOp::NotEquals => "!=",
                RelationOp::LessThan => "<",
                RelationOp::LessThanEq => "<=",
                RelationOp::GreaterThan => ">",
                RelationOp::GreaterThanEq => ">=",
                RelationOp::In => "in",
            };
            format!("{} {op_str} {}", format_cel(l), format_cel(r))
        }
        Expression::Arithmetic(l, op, r) => {
            let op_str = match op {
                ArithmeticOp::Add => "+",
                ArithmeticOp::Subtract => "-",
                ArithmeticOp::Multiply => "*",
                ArithmeticOp::Divide => "/",
                ArithmeticOp::Modulus => "%",
            };
            format!("{} {op_str} {}", format_cel(l), format_cel(r))
        }
        Expression::Ternary(c, t, e) =>
            format!("{} ? {} : {}", format_cel(c), format_cel(t), format_cel(e)),
        Expression::Member(base, member) => match member.as_ref() {
            Member::Attribute(name) => format!("{}.{name}", format_cel(base)),
            Member::Index(idx) => format!("{}[{}]", format_cel(base), format_cel(idx)),
            Member::Fields(fields) => {
                let parts: Vec<String> = fields.iter()
                    .map(|(k, v)| format!("{k}: {}", format_cel(v)))
                    .collect();
                format!("{}{{ {} }}", format_cel(base), parts.join(", "))
            }
        },
        Expression::FunctionCall(name, target, args) => {
            let name_str = format_cel(name);
            let args_str: Vec<String> = args.iter().map(format_cel).collect();
            match target {
                Some(t) => format!("{}.{name_str}({})", format_cel(t), args_str.join(", ")),
                None => format!("{name_str}({})", args_str.join(", ")),
            }
        }
        Expression::List(items) => {
            let parts: Vec<String> = items.iter().map(format_cel).collect();
            format!("[{}]", parts.join(", "))
        }
        Expression::Map(entries) => {
            let parts: Vec<String> = entries.iter()
                .map(|(k, v)| format!("{}: {}", format_cel(k), format_cel(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str) -> LogExpr {
        LogExpr::GetFieldByName { field_name: name.into() }
    }

    fn lit_i64(n: i64) -> LogExpr {
        LogExpr::Literal(Value::I64(n))
    }

    fn lit_str(s: &str) -> LogExpr {
        LogExpr::Literal(Value::String(s.into()))
    }

    /// Integer literals become Value::I64 in the IR.
    #[test]
    fn literal_int() {
        let t = cel_to_log_expr("42").unwrap();
        assert_eq!(t.expr, lit_i64(42));
        assert_eq!(t.status, TranslationStatus::FullyRaised);
    }

    /// CEL single-quoted strings become Value::String.
    #[test]
    fn literal_string() {
        let t = cel_to_log_expr("'hello'").unwrap();
        assert_eq!(t.expr, lit_str("hello"));
        assert_eq!(t.status, TranslationStatus::FullyRaised);
    }

    /// Boolean literals map directly.
    #[test]
    fn literal_bool() {
        let t = cel_to_log_expr("true").unwrap();
        assert_eq!(t.expr, LogExpr::Literal(Value::Bool(true)));
    }

    /// A bare identifier becomes a field reference — the event schema
    /// resolves it later during type checking.
    #[test]
    fn field_access() {
        assert_eq!(cel_to_log_expr("age").unwrap().expr, field("age"));
    }

    /// Relational operators map to the corresponding LogExpr variant.
    /// The whole expression is fully raised (no CelUdf nodes).
    #[test]
    fn simple_comparison() {
        let t = cel_to_log_expr("age > 18").unwrap();
        assert_eq!(t.expr, LogExpr::GreaterThan {
            lhs: boxed(field("age")),
            rhs: boxed(lit_i64(18)),
        });
        assert_eq!(t.status, TranslationStatus::FullyRaised);
    }

    /// `&&` composes a field reference (used as bool) with a comparison.
    /// Both sides are fully raised; the conjunction itself is native IR.
    #[test]
    fn boolean_logic() {
        let t = cel_to_log_expr("active && score > 50").unwrap();
        assert_eq!(t.expr, LogExpr::LogicalAnd {
            lhs: boxed(field("active")),
            rhs: boxed(LogExpr::GreaterThan {
                lhs: boxed(field("score")),
                rhs: boxed(lit_i64(50)),
            }),
        });
        assert_eq!(t.status, TranslationStatus::FullyRaised);
    }

    /// CEL method-call syntax: `receiver.method(arg)`. The receiver
    /// becomes the `receiver` field, the argument becomes `arg`.
    #[test]
    fn string_method() {
        let t = cel_to_log_expr("email.contains('admin')").unwrap();
        assert_eq!(t.expr, LogExpr::Contains {
            receiver: boxed(field("email")),
            arg: boxed(lit_str("admin")),
        });
    }

    /// CEL ternary `cond ? then : else` maps to LogExpr::Conditional.
    #[test]
    fn ternary() {
        let t = cel_to_log_expr("vip ? 'premium' : 'standard'").unwrap();
        assert_eq!(t.expr, LogExpr::Conditional {
            condition: boxed(field("vip")),
            then_expr: boxed(lit_str("premium")),
            else_expr: boxed(lit_str("standard")),
        });
    }

    /// Dotted member access chains become nested GetChildByName nodes,
    /// innermost first: `payload.shipping.country` →
    /// GetChildByName("country", GetChildByName("shipping", GetFieldByName("payload")))
    #[test]
    fn nested_member_access() {
        let t = cel_to_log_expr("payload.shipping.country").unwrap();
        assert_eq!(t.expr, LogExpr::GetChildByName {
            child_name: "country".into(),
            operand: boxed(LogExpr::GetChildByName {
                child_name: "shipping".into(),
                operand: boxed(field("payload")),
            }),
        });
    }

    /// CEL cast functions (`int`, `double`, `string`, etc.) map to
    /// the corresponding Cast* LogExpr variant.
    #[test]
    fn cast_function() {
        let t = cel_to_log_expr("int(score)").unwrap();
        assert_eq!(t.expr, LogExpr::CastInt { operand: boxed(field("score")) });
        assert_eq!(t.status, TranslationStatus::FullyRaised);
    }

    /// CEL `in` operator with a list literal. The list becomes a
    /// Value::Array and the whole thing is a native In node.
    #[test]
    fn in_with_list() {
        let t = cel_to_log_expr("status in ['active', 'pending']").unwrap();
        assert_eq!(t.expr, LogExpr::In {
            lhs: boxed(field("status")),
            rhs: boxed(LogExpr::Literal(Value::Array(vec![
                Value::String("active".into()),
                Value::String("pending".into()),
            ]))),
        });
    }

    /// Arithmetic operators map directly. Type checking (int vs float
    /// widening, string concatenation overloading) happens later.
    #[test]
    fn arithmetic() {
        let t = cel_to_log_expr("price + tax").unwrap();
        assert_eq!(t.expr, LogExpr::Add {
            lhs: boxed(field("price")),
            rhs: boxed(field("tax")),
        });
    }

    /// Logical negation: `!` maps to LogicalNot.
    #[test]
    fn negation() {
        let t = cel_to_log_expr("!active").unwrap();
        assert_eq!(t.expr, LogExpr::LogicalNot { operand: boxed(field("active")) });
    }

    /// Unary minus maps to Negate (arithmetic negation, not logical).
    #[test]
    fn unary_minus() {
        let t = cel_to_log_expr("-amount").unwrap();
        assert_eq!(t.expr, LogExpr::Negate { operand: boxed(field("amount")) });
    }

    /// `size()` as a free function (not a method) with one argument.
    #[test]
    fn size_function() {
        let t = cel_to_log_expr("size(name)").unwrap();
        assert_eq!(t.expr, LogExpr::Size { operand: boxed(field("name")) });
    }

    /// Index access `items[0]` becomes an Index node with the collection
    /// as lhs and the index expression as rhs.
    #[test]
    fn index_access() {
        let t = cel_to_log_expr("items[0]").unwrap();
        assert_eq!(t.expr, LogExpr::Index {
            lhs: boxed(field("items")),
            rhs: boxed(lit_i64(0)),
        });
    }

    /// Equality comparison with a string literal.
    #[test]
    fn equality() {
        let t = cel_to_log_expr("status == 'active'").unwrap();
        assert_eq!(t.expr, LogExpr::Equal {
            lhs: boxed(field("status")),
            rhs: boxed(lit_str("active")),
        });
    }

    /// A compound expression mixing comparison and string methods.
    /// Everything is in the catalog, so the result is fully raised.
    #[test]
    fn complex_nested() {
        let t = cel_to_log_expr("age >= 18 && email.endsWith('@example.com')").unwrap();
        assert_eq!(t.status, TranslationStatus::FullyRaised);
    }

    /// A function not in our catalog becomes an opaque CelUdf node.
    /// The source text is reconstructed from the AST for debugging.
    /// The args are the variable references the CEL runtime will need.
    #[test]
    fn unknown_function_becomes_udf() {
        let t = cel_to_log_expr("frobnicate(x, y)").unwrap();
        assert_eq!(t.status, TranslationStatus::Opaque);
        match &t.expr {
            LogExpr::CelUdf { source, args } => {
                assert!(source.contains("frobnicate"));
                assert_eq!(args.len(), 2);
            }
            other => panic!("expected CelUdf, got {other:?}"),
        }
    }

    /// The key partial-raising test: one side of an && is fully raised
    /// (age >= 18 → GreaterOrEqual), the other side is an unknown
    /// function that becomes a CelUdf leaf. The conjunction itself is
    /// native IR, so the optimizer can still reason about short-circuit
    /// ordering — e.g. put the cheap field comparison first.
    #[test]
    fn partial_raising() {
        let t = cel_to_log_expr("age >= 18 && custom_score(payload)").unwrap();
        assert_eq!(t.status, TranslationStatus::PartiallyRaised);
        match &t.expr {
            LogExpr::LogicalAnd { lhs, rhs } => {
                assert!(matches!(lhs.as_ref(), LogExpr::GreaterOrEqual { .. }));
                assert!(matches!(rhs.as_ref(), LogExpr::CelUdf { .. }));
            }
            other => panic!("expected LogicalAnd, got {other:?}"),
        }
    }

    /// When every part of the expression maps to a catalog function,
    /// the status is FullyRaised — no CelUdf nodes anywhere in the tree.
    #[test]
    fn fully_raised_has_no_udfs() {
        let t = cel_to_log_expr("size(name) > 0").unwrap();
        assert_eq!(t.status, TranslationStatus::FullyRaised);
    }

    /// Nested ternaries are how CEL represents multi-way conditionals
    /// (like SQL CASE). Each level becomes a Conditional node. A later
    /// normalization pass could collapse these into a single Case node.
    #[test]
    fn nested_ternary() {
        let t = cel_to_log_expr(
            "score >= 90 ? 'A' : (score >= 80 ? 'B' : 'C')"
        ).unwrap();
        assert_eq!(t.status, TranslationStatus::FullyRaised);
        assert!(matches!(t.expr, LogExpr::Conditional { .. }));
    }

    /// A method call we don't recognize (toUpperCase is not in our
    /// catalog) becomes an opaque CelUdf. The CEL runtime handles it.
    #[test]
    fn unknown_method_becomes_udf() {
        let t = cel_to_log_expr("name.toUpperCase()").unwrap();
        assert_eq!(t.status, TranslationStatus::Opaque);
        assert!(matches!(t.expr, LogExpr::CelUdf { .. }));
    }

    /// Verify that `_` is a valid CEL identifier, so `_.enrichments.foo`
    /// parses as nested member access rooted at `_`.
    #[test]
    fn underscore_root_parses() {
        let t = cel_to_log_expr("_.enrichments.user_profile.tier").unwrap();
        assert_eq!(t.expr, LogExpr::GetChildByName {
            child_name: "tier".into(),
            operand: boxed(LogExpr::GetChildByName {
                child_name: "user_profile".into(),
                operand: boxed(LogExpr::GetChildByName {
                    child_name: "enrichments".into(),
                    operand: boxed(field("_")),
                }),
            }),
        });
        assert_eq!(t.status, TranslationStatus::FullyRaised);
    }

    /// A bare literal has no coeffects — it's pure and can be memoized
    /// forever.
    #[test]
    fn coeffects_pure_literal() {
        use crate::expr_gen::transitive_coeffects;
        let t = cel_to_log_expr("42").unwrap();
        let c = transitive_coeffects(&t.expr);
        assert!(c.is_pure());
    }

    /// Field access introduces reads_event_data. The comparison operator
    /// is pure, but its child reads a field, so the transitive closure
    /// inherits reads_event_data. No enrichment or time coeffects.
    #[test]
    fn coeffects_field_access_reads_event() {
        use crate::expr_gen::transitive_coeffects;
        let t = cel_to_log_expr("age > 18").unwrap();
        let c = transitive_coeffects(&t.expr);
        assert!(c.reads_event_data);
        assert!(!c.reads_enrichment);
        assert!(c.reads_current_time.is_none());
    }

    /// A CelUdf node conservatively assumes all coeffects — we don't
    /// know what the opaque CEL function does, so we assume the worst.
    /// One CelUdf anywhere in the tree poisons the whole expression.
    #[test]
    fn coeffects_udf_is_conservative() {
        use crate::expr_gen::transitive_coeffects;
        use crate::coeffects::Coeffects;
        let t = cel_to_log_expr("age >= 18 && custom_score(payload)").unwrap();
        let c = transitive_coeffects(&t.expr);
        assert_eq!(c, Coeffects::all());
    }

    #[test]
    fn cidr_contains_raises_fully() {
        let t = cel_to_log_expr("cidr_contains(src_ip, '10/8')").unwrap();
        assert_eq!(t.status, TranslationStatus::FullyRaised);
        assert!(matches!(t.expr, LogExpr::CidrContains { .. }));
    }

    #[test]
    fn ip_to_int_raises_fully() {
        let t = cel_to_log_expr("ip_to_int(src_ip)").unwrap();
        assert_eq!(t.status, TranslationStatus::FullyRaised);
        assert!(matches!(t.expr, LogExpr::IpToInt { .. }));
    }

    /// Coeffects propagate through pure operators: `size(name)` reads
    /// event data (via `name`), and `+` is pure, so the whole expression
    /// reads event data. No time coeffect because neither side reads time.
    #[test]
    fn coeffects_nested_propagates() {
        use crate::expr_gen::transitive_coeffects;
        let t = cel_to_log_expr("size(name) + size(email)").unwrap();
        let c = transitive_coeffects(&t.expr);
        assert!(c.reads_event_data);
        assert!(c.reads_current_time.is_none());
    }
}
