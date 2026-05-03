//! Expression normalizer on the logical IR.
//!
//! Rewrites a `LogExpr` tree so that structurally equivalent
//! expressions have identical structure, enabling downstream
//! deduplication when the tree is lowered to a physical DAG.
//!
//! Transformations:
//! - **Commutative canonicalization**: operands of commutative operators
//!   are sorted by structural hash so `a + b` and `b + a` produce the
//!   same tree.
//! - **Double negation elimination**: `!!x` → `x`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::expr_gen::LogExpr;
use crate::value::Value;

pub fn normalize(expr: &LogExpr) -> LogExpr {
    let normalized = match expr {
        LogExpr::Literal(_)
        | LogExpr::GetFieldByName { .. }
        | LogExpr::GetFieldByIndex { .. }
        | LogExpr::CurrentTimestamp => expr.clone(),

        LogExpr::LogicalNot { operand } => {
            let inner = normalize(operand);
            if let LogExpr::LogicalNot { operand: inner_inner } = &inner {
                return (**inner_inner).clone();
            }
            LogExpr::LogicalNot { operand: Box::new(inner) }
        }

        // Additive identity: x + 0 → x, 0 + x → x
        LogExpr::Add { lhs, rhs } => {
            let l = normalize(lhs);
            let r = normalize(rhs);
            if let LogExpr::Literal(v) = &r { if v.is_zero() { return l; } }
            if let LogExpr::Literal(v) = &l { if v.is_zero() { return r; } }
            sort_binary(l, r, |l, r| LogExpr::Add { lhs: l, rhs: r })
        }

        // Multiplicative identity: x * 1 → x, 1 * x → x
        // Multiplicative zero: x * 0 → 0
        LogExpr::Multiply { lhs, rhs } => {
            let l = normalize(lhs);
            let r = normalize(rhs);
            if let LogExpr::Literal(v) = &r { if v.is_one() { return l; } }
            if let LogExpr::Literal(v) = &l { if v.is_one() { return r; } }
            if let LogExpr::Literal(v) = &r { if v.is_zero() { return r; } }
            if let LogExpr::Literal(v) = &l { if v.is_zero() { return l; } }
            sort_binary(l, r, |l, r| LogExpr::Multiply { lhs: l, rhs: r })
        }

        // Subtractive identity: x - 0 → x
        LogExpr::Subtract { lhs, rhs } => {
            let l = normalize(lhs);
            let r = normalize(rhs);
            if let LogExpr::Literal(v) = &r { if v.is_zero() { return l; } }
            LogExpr::Subtract { lhs: Box::new(l), rhs: Box::new(r) }
        }

        // Boolean identities: x && true → x, x || false → x
        LogExpr::LogicalAnd { lhs, rhs } => {
            let l = normalize(lhs);
            let r = normalize(rhs);
            if let LogExpr::Literal(Value::Bool(true)) = &r { return l; }
            if let LogExpr::Literal(Value::Bool(true)) = &l { return r; }
            if let LogExpr::Literal(Value::Bool(false)) = &r { return r; }
            if let LogExpr::Literal(Value::Bool(false)) = &l { return l; }
            sort_binary(l, r, |l, r| LogExpr::LogicalAnd { lhs: l, rhs: r })
        }
        LogExpr::LogicalOr { lhs, rhs } => {
            let l = normalize(lhs);
            let r = normalize(rhs);
            if let LogExpr::Literal(Value::Bool(false)) = &r { return l; }
            if let LogExpr::Literal(Value::Bool(false)) = &l { return r; }
            if let LogExpr::Literal(Value::Bool(true)) = &r { return r; }
            if let LogExpr::Literal(Value::Bool(true)) = &l { return l; }
            sort_binary(l, r, |l, r| LogExpr::LogicalOr { lhs: l, rhs: r })
        }

        _ if expr.is_commutative() => normalize_commutative(expr),

        _ => normalize_children(expr),
    };
    normalized
}

fn normalize_commutative(expr: &LogExpr) -> LogExpr {
    match expr {
        LogExpr::LogicalOr { lhs, rhs } => sort_binary(normalize(lhs), normalize(rhs), |l, r| LogExpr::LogicalOr { lhs: l, rhs: r }),
        LogExpr::LogicalAnd { lhs, rhs } => sort_binary(normalize(lhs), normalize(rhs), |l, r| LogExpr::LogicalAnd { lhs: l, rhs: r }),
        LogExpr::Equal { lhs, rhs } => sort_binary(normalize(lhs), normalize(rhs), |l, r| LogExpr::Equal { lhs: l, rhs: r }),
        LogExpr::NotEqual { lhs, rhs } => sort_binary(normalize(lhs), normalize(rhs), |l, r| LogExpr::NotEqual { lhs: l, rhs: r }),
        LogExpr::NullSafeEqual { lhs, rhs } => sort_binary(normalize(lhs), normalize(rhs), |l, r| LogExpr::NullSafeEqual { lhs: l, rhs: r }),
        LogExpr::NullSafeNotEqual { lhs, rhs } => sort_binary(normalize(lhs), normalize(rhs), |l, r| LogExpr::NullSafeNotEqual { lhs: l, rhs: r }),
        LogExpr::Add { lhs, rhs } => sort_binary(normalize(lhs), normalize(rhs), |l, r| LogExpr::Add { lhs: l, rhs: r }),
        LogExpr::Multiply { lhs, rhs } => sort_binary(normalize(lhs), normalize(rhs), |l, r| LogExpr::Multiply { lhs: l, rhs: r }),
        _ => normalize_children(expr),
    }
}

fn sort_binary(
    lhs: LogExpr,
    rhs: LogExpr,
    build: impl Fn(Box<LogExpr>, Box<LogExpr>) -> LogExpr,
) -> LogExpr {
    let lh = structural_hash(&lhs);
    let rh = structural_hash(&rhs);
    if lh <= rh {
        build(Box::new(lhs), Box::new(rhs))
    } else {
        build(Box::new(rhs), Box::new(lhs))
    }
}

fn structural_hash(expr: &LogExpr) -> u64 {
    let mut hasher = DefaultHasher::new();
    expr.hash(&mut hasher);
    hasher.finish()
}

fn normalize_children(expr: &LogExpr) -> LogExpr {
    match expr {
        LogExpr::Literal(_)
        | LogExpr::GetFieldByName { .. }
        | LogExpr::GetFieldByIndex { .. }
        | LogExpr::CurrentTimestamp => expr.clone(),

        // Unary
        LogExpr::LogicalNot { operand } => LogExpr::LogicalNot { operand: n(operand) },
        LogExpr::Negate { operand } => LogExpr::Negate { operand: n(operand) },
        LogExpr::IsNull { operand } => LogExpr::IsNull { operand: n(operand) },
        LogExpr::IsNotNull { operand } => LogExpr::IsNotNull { operand: n(operand) },
        LogExpr::IsNan { operand } => LogExpr::IsNan { operand: n(operand) },
        LogExpr::IsFinite { operand } => LogExpr::IsFinite { operand: n(operand) },
        LogExpr::IsInfinite { operand } => LogExpr::IsInfinite { operand: n(operand) },
        LogExpr::Abs { operand } => LogExpr::Abs { operand: n(operand) },
        LogExpr::Sqrt { operand } => LogExpr::Sqrt { operand: n(operand) },
        LogExpr::Exp { operand } => LogExpr::Exp { operand: n(operand) },
        LogExpr::Sign { operand } => LogExpr::Sign { operand: n(operand) },
        LogExpr::Size { operand } => LogExpr::Size { operand: n(operand) },
        LogExpr::Lower { operand } => LogExpr::Lower { operand: n(operand) },
        LogExpr::Upper { operand } => LogExpr::Upper { operand: n(operand) },
        LogExpr::Trim { operand } => LogExpr::Trim { operand: n(operand) },
        LogExpr::TimestampExtract { operand } => LogExpr::TimestampExtract { operand: n(operand) },
        LogExpr::RoundTemporal { operand } => LogExpr::RoundTemporal { operand: n(operand) },
        LogExpr::RoundCalendar { operand } => LogExpr::RoundCalendar { operand: n(operand) },
        LogExpr::CastBool { operand } => LogExpr::CastBool { operand: n(operand) },
        LogExpr::CastInt { operand } => LogExpr::CastInt { operand: n(operand) },
        LogExpr::CastUint { operand } => LogExpr::CastUint { operand: n(operand) },
        LogExpr::CastDouble { operand } => LogExpr::CastDouble { operand: n(operand) },
        LogExpr::CastString { operand } => LogExpr::CastString { operand: n(operand) },
        LogExpr::CastBytes { operand } => LogExpr::CastBytes { operand: n(operand) },
        LogExpr::CastDuration { operand } => LogExpr::CastDuration { operand: n(operand) },
        LogExpr::CastTimestamp { operand } => LogExpr::CastTimestamp { operand: n(operand) },
        LogExpr::TypeOf { operand } => LogExpr::TypeOf { operand: n(operand) },
        LogExpr::Dyn { operand } => LogExpr::Dyn { operand: n(operand) },
        LogExpr::Ln { operand } => LogExpr::Ln { operand: n(operand) },
        LogExpr::Log10 { operand } => LogExpr::Log10 { operand: n(operand) },
        LogExpr::Ceil { operand } => LogExpr::Ceil { operand: n(operand) },
        LogExpr::Floor { operand } => LogExpr::Floor { operand: n(operand) },
        LogExpr::Round { operand } => LogExpr::Round { operand: n(operand) },
        LogExpr::JsonParse { operand } => LogExpr::JsonParse { operand: n(operand) },
        LogExpr::JsonParseStruct { operand } => LogExpr::JsonParseStruct { operand: n(operand) },
        LogExpr::JsonStringify { operand } => LogExpr::JsonStringify { operand: n(operand) },
        LogExpr::IpToInt { operand } => LogExpr::IpToInt { operand: n(operand) },
        LogExpr::IntToIp { operand } => LogExpr::IntToIp { operand: n(operand) },
        LogExpr::Has { operand } => LogExpr::Has { operand: n(operand) },
        LogExpr::RaiseError { operand } => LogExpr::RaiseError { operand: n(operand) },

        // Binary (non-commutative handled here; commutative handled in normalize_commutative)
        LogExpr::LessThan { lhs, rhs } => LogExpr::LessThan { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::LessOrEqual { lhs, rhs } => LogExpr::LessOrEqual { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::GreaterThan { lhs, rhs } => LogExpr::GreaterThan { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::GreaterOrEqual { lhs, rhs } => LogExpr::GreaterOrEqual { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Subtract { lhs, rhs } => LogExpr::Subtract { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Divide { lhs, rhs } => LogExpr::Divide { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Modulus { lhs, rhs } => LogExpr::Modulus { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Power { lhs, rhs } => LogExpr::Power { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Coalesce { lhs, rhs } => LogExpr::Coalesce { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::TryOrElse { lhs, rhs } => LogExpr::TryOrElse { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Least { lhs, rhs } => LogExpr::Least { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Greatest { lhs, rhs } => LogExpr::Greatest { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::StringSplit { lhs, rhs } => LogExpr::StringSplit { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::StringPosition { lhs, rhs } => LogExpr::StringPosition { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Concat { lhs, rhs } => LogExpr::Concat { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Index { lhs, rhs } => LogExpr::Index { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::In { lhs, rhs } => LogExpr::In { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::JsonExtract { lhs, rhs } => LogExpr::JsonExtract { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::JsonExtractString { lhs, rhs } => LogExpr::JsonExtractString { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::CidrContains { lhs, rhs } => LogExpr::CidrContains { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::CidrMatch { lhs, rhs } => LogExpr::CidrMatch { lhs: n(lhs), rhs: n(rhs) },
        // Commutative binary ops are handled by normalize_commutative, but if
        // we reach here (shouldn't happen), just normalize children.
        LogExpr::LogicalOr { lhs, rhs } => LogExpr::LogicalOr { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::LogicalAnd { lhs, rhs } => LogExpr::LogicalAnd { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Equal { lhs, rhs } => LogExpr::Equal { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::NotEqual { lhs, rhs } => LogExpr::NotEqual { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::NullSafeEqual { lhs, rhs } => LogExpr::NullSafeEqual { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::NullSafeNotEqual { lhs, rhs } => LogExpr::NullSafeNotEqual { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Add { lhs, rhs } => LogExpr::Add { lhs: n(lhs), rhs: n(rhs) },
        LogExpr::Multiply { lhs, rhs } => LogExpr::Multiply { lhs: n(lhs), rhs: n(rhs) },

        // Receiver + arg
        LogExpr::Contains { receiver, arg } => LogExpr::Contains { receiver: n(receiver), arg: n(arg) },
        LogExpr::StartsWith { receiver, arg } => LogExpr::StartsWith { receiver: n(receiver), arg: n(arg) },
        LogExpr::EndsWith { receiver, arg } => LogExpr::EndsWith { receiver: n(receiver), arg: n(arg) },
        LogExpr::RegexMatch { receiver, arg } => LogExpr::RegexMatch { receiver: n(receiver), arg: n(arg) },

        // Ternary
        LogExpr::Between { arg0, arg1, arg2 } => LogExpr::Between { arg0: n(arg0), arg1: n(arg1), arg2: n(arg2) },
        LogExpr::Substring { arg0, arg1, arg2 } => LogExpr::Substring { arg0: n(arg0), arg1: n(arg1), arg2: n(arg2) },
        LogExpr::Replace { arg0, arg1, arg2 } => LogExpr::Replace { arg0: n(arg0), arg1: n(arg1), arg2: n(arg2) },

        LogExpr::Conditional { condition, then_expr, else_expr } => LogExpr::Conditional {
            condition: n(condition), then_expr: n(then_expr), else_expr: n(else_expr),
        },

        // Navigation
        LogExpr::GetChildByName { child_name, operand } => LogExpr::GetChildByName {
            child_name: child_name.clone(), operand: n(operand),
        },
        LogExpr::GetChildByIndex { child_index, lhs, rhs } => LogExpr::GetChildByIndex {
            child_index: *child_index, lhs: n(lhs), rhs: n(rhs),
        },

        // HOFs
        LogExpr::All { collection, binding, body } => LogExpr::All { collection: n(collection), binding: binding.clone(), body: n(body) },
        LogExpr::Exists { collection, binding, body } => LogExpr::Exists { collection: n(collection), binding: binding.clone(), body: n(body) },
        LogExpr::ExistsOne { collection, binding, body } => LogExpr::ExistsOne { collection: n(collection), binding: binding.clone(), body: n(body) },
        LogExpr::Filter { collection, binding, body } => LogExpr::Filter { collection: n(collection), binding: binding.clone(), body: n(body) },
        LogExpr::MapTransform { collection, binding, body } => LogExpr::MapTransform { collection: n(collection), binding: binding.clone(), body: n(body) },

        // CelUdf
        LogExpr::CelUdf { source, args } => LogExpr::CelUdf {
            source: source.clone(),
            args: args.iter().map(|a| Box::new(normalize(a))).collect(),
        },

        // Case
        LogExpr::Case { arms, default } => LogExpr::Case {
            arms: arms.iter().map(|(c, r)| (n(c), n(r))).collect(),
            default: n(default),
        },
    }
}

fn n(expr: &LogExpr) -> Box<LogExpr> {
    Box::new(normalize(expr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str) -> LogExpr {
        LogExpr::GetFieldByName { field_name: name.into() }
    }

    /// `a + b` and `b + a` normalize to the same tree.
    #[test]
    fn commutative_canonicalization() {
        let ab = LogExpr::Add {
            lhs: Box::new(field("a")),
            rhs: Box::new(field("b")),
        };
        let ba = LogExpr::Add {
            lhs: Box::new(field("b")),
            rhs: Box::new(field("a")),
        };
        assert_eq!(normalize(&ab), normalize(&ba));
    }

    /// `a - b` and `b - a` are NOT equal after normalization.
    #[test]
    fn non_commutative_preserves_order() {
        let ab = LogExpr::Subtract {
            lhs: Box::new(field("a")),
            rhs: Box::new(field("b")),
        };
        let ba = LogExpr::Subtract {
            lhs: Box::new(field("b")),
            rhs: Box::new(field("a")),
        };
        assert_ne!(normalize(&ab), normalize(&ba));
    }

    /// `!!flag` normalizes to just `flag`.
    #[test]
    fn double_negation_elimination() {
        let expr = LogExpr::LogicalNot {
            operand: Box::new(LogExpr::LogicalNot {
                operand: Box::new(field("flag")),
            }),
        };
        assert_eq!(normalize(&expr), field("flag"));
    }

    /// `field == "x"` and `"x" == field` normalize to the same tree
    /// (equality is commutative).
    #[test]
    fn equality_commutativity() {
        let a = LogExpr::Equal {
            lhs: Box::new(field("status")),
            rhs: Box::new(LogExpr::Literal(Value::String("active".into()))),
        };
        let b = LogExpr::Equal {
            lhs: Box::new(LogExpr::Literal(Value::String("active".into()))),
            rhs: Box::new(field("status")),
        };
        assert_eq!(normalize(&a), normalize(&b));
    }

    /// Normalization is idempotent: normalizing an already-normalized
    /// expression produces the same result.
    #[test]
    fn idempotent() {
        let expr = LogExpr::LogicalAnd {
            lhs: Box::new(LogExpr::GreaterOrEqual {
                lhs: Box::new(field("age")),
                rhs: Box::new(LogExpr::Literal(Value::I64(18))),
            }),
            rhs: Box::new(LogExpr::Contains {
                receiver: Box::new(field("email")),
                arg: Box::new(LogExpr::Literal(Value::String("@example.com".into()))),
            }),
        };
        let once = normalize(&expr);
        let twice = normalize(&once);
        assert_eq!(once, twice);
    }

    /// Normalization recurses into children: commutative sorting
    /// happens at every level, not just the root.
    #[test]
    fn deep_normalization() {
        let expr = LogExpr::LogicalAnd {
            lhs: Box::new(LogExpr::Add {
                lhs: Box::new(field("b")),
                rhs: Box::new(field("a")),
            }),
            rhs: Box::new(LogExpr::Multiply {
                lhs: Box::new(field("d")),
                rhs: Box::new(field("c")),
            }),
        };
        let norm = normalize(&expr);
        // Both Add and Multiply children should be sorted
        match &norm {
            LogExpr::LogicalAnd { lhs, rhs } => {
                // The inner ops should have sorted their children
                match (lhs.as_ref(), rhs.as_ref()) {
                    (LogExpr::Add { lhs: al, rhs: ar }, LogExpr::Multiply { lhs: ml, rhs: mr }) |
                    (LogExpr::Multiply { lhs: ml, rhs: mr }, LogExpr::Add { lhs: al, rhs: ar }) => {
                        let add_sorted = structural_hash(al) <= structural_hash(ar);
                        let mul_sorted = structural_hash(ml) <= structural_hash(mr);
                        assert!(add_sorted, "Add children should be sorted");
                        assert!(mul_sorted, "Multiply children should be sorted");
                    }
                    other => panic!("unexpected shape: {other:?}"),
                }
            }
            other => panic!("expected LogicalAnd, got {other:?}"),
        }
    }

    /// x + 0 → x
    #[test]
    fn additive_identity() {
        let expr = LogExpr::Add {
            lhs: Box::new(field("x")),
            rhs: Box::new(LogExpr::Literal(Value::I64(0))),
        };
        assert_eq!(normalize(&expr), field("x"));
    }

    /// 0 + x → x (commutative identity)
    #[test]
    fn additive_identity_reversed() {
        let expr = LogExpr::Add {
            lhs: Box::new(LogExpr::Literal(Value::I64(0))),
            rhs: Box::new(field("x")),
        };
        assert_eq!(normalize(&expr), field("x"));
    }

    /// x * 1 → x
    #[test]
    fn multiplicative_identity() {
        let expr = LogExpr::Multiply {
            lhs: Box::new(field("x")),
            rhs: Box::new(LogExpr::Literal(Value::I64(1))),
        };
        assert_eq!(normalize(&expr), field("x"));
    }

    /// x * 0 → 0
    #[test]
    fn multiplicative_zero() {
        let expr = LogExpr::Multiply {
            lhs: Box::new(field("x")),
            rhs: Box::new(LogExpr::Literal(Value::I64(0))),
        };
        assert_eq!(normalize(&expr), LogExpr::Literal(Value::I64(0)));
    }

    /// x - 0 → x
    #[test]
    fn subtractive_identity() {
        let expr = LogExpr::Subtract {
            lhs: Box::new(field("x")),
            rhs: Box::new(LogExpr::Literal(Value::I64(0))),
        };
        assert_eq!(normalize(&expr), field("x"));
    }

    /// x && true → x
    #[test]
    fn logical_and_true_identity() {
        let expr = LogExpr::LogicalAnd {
            lhs: Box::new(field("flag")),
            rhs: Box::new(LogExpr::Literal(Value::Bool(true))),
        };
        assert_eq!(normalize(&expr), field("flag"));
    }

    /// x && false → false
    #[test]
    fn logical_and_false_annihilator() {
        let expr = LogExpr::LogicalAnd {
            lhs: Box::new(field("flag")),
            rhs: Box::new(LogExpr::Literal(Value::Bool(false))),
        };
        assert_eq!(normalize(&expr), LogExpr::Literal(Value::Bool(false)));
    }

    /// x || false → x
    #[test]
    fn logical_or_false_identity() {
        let expr = LogExpr::LogicalOr {
            lhs: Box::new(field("flag")),
            rhs: Box::new(LogExpr::Literal(Value::Bool(false))),
        };
        assert_eq!(normalize(&expr), field("flag"));
    }

    /// x || true → true
    #[test]
    fn logical_or_true_annihilator() {
        let expr = LogExpr::LogicalOr {
            lhs: Box::new(field("flag")),
            rhs: Box::new(LogExpr::Literal(Value::Bool(true))),
        };
        assert_eq!(normalize(&expr), LogExpr::Literal(Value::Bool(true)));
    }

    /// f64 zero works too
    #[test]
    fn additive_identity_f64() {
        let expr = LogExpr::Add {
            lhs: Box::new(field("x")),
            rhs: Box::new(LogExpr::Literal(Value::F64(0.0))),
        };
        assert_eq!(normalize(&expr), field("x"));
    }
}
