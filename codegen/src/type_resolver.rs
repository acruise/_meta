//! Resolution of declared type names in type literals.
//!
//! A type literal (`meta_types::type_syntax::TypeLiteral`) is syntax-checked
//! where it is written, but a declared name in it (`array<Order>`) needs the
//! plan's type catalog. This pass resolves every literal in an expression
//! against a caller-supplied resolver, typically a type catalog lookup. Run
//! it before type checking; the checker treats an unresolved literal as an
//! error.

use meta_types::type_syntax::TypeSyntaxError;
use meta_types::value::ValueType;

use crate::expr_gen::LogExpr;

/// Resolve every type literal in `expr`, in place. Stops at the first
/// unknown name.
pub fn resolve_type_literals(
    expr: &mut LogExpr,
    resolve: &dyn Fn(&str) -> Option<ValueType>,
) -> Result<(), TypeSyntaxError> {
    for t in expr.type_literals_mut() {
        t.resolve(resolve)?;
    }
    for child in expr.children_mut() {
        resolve_type_literals(child, resolve)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meta_types::type_syntax::{parse_type_literal, TypeLiteral};

    fn parse_as(operand: LogExpr, ty: &str) -> LogExpr {
        LogExpr::JsonParseAs { target_type: TypeLiteral::parse(ty).unwrap(), operand: Box::new(operand) }
    }

    #[test]
    fn resolves_nested_literals_through_any_parent() {
        let field = LogExpr::GetFieldByName { field_name: "s".into() };
        let inner = parse_as(field, "array<Order>");
        let mut e = LogExpr::Case {
            arms: vec![(Box::new(LogExpr::Literal(meta_types::value::Value::Bool(true))), Box::new(inner))],
            default: Box::new(LogExpr::Literal(meta_types::value::Value::Null)),
        };
        let order = parse_type_literal("struct{id: u64}").unwrap();
        resolve_type_literals(&mut e, &|n| (n == "Order").then(|| order.clone())).unwrap();
        let LogExpr::Case { arms, .. } = &e else { panic!() };
        let LogExpr::JsonParseAs { target_type, .. } = arms[0].1.as_ref() else { panic!() };
        assert_eq!(target_type.to_string(), "array<struct{id: u64}>");
    }

    #[test]
    fn unknown_name_is_an_error() {
        let mut e = parse_as(LogExpr::GetFieldByName { field_name: "s".into() }, "Missing");
        assert!(resolve_type_literals(&mut e, &|_| None).is_err());
    }
}
