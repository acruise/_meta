//! Canonical names for variables bound by higher-order functions.
//!
//! Hash-consing gives a node the identity of its whole subtree, so a path
//! step is distinct from the same step under a different parent. A bound
//! variable breaks that: in `xs.map(x, x.a)` and `ys.map(x, x.a)` the bodies
//! `x.a` are identical, yet one ranges over `xs` and the other over `ys`.
//! Merged, they would look like one expression to everything keyed by node
//! identity (materialization, memoization, provenance, coeffect closure).
//!
//! This pass renames every binder after what it ranges over -- a structural
//! hash of its (already canonical) collection and its nesting depth -- and
//! rewrites every reference to a bound variable from `GetFieldByName` into
//! `GetBinding`. Two references are then the same node exactly when they
//! denote the same variable: same collection, same nesting. The depth term
//! keeps `xs.map(a, xs.map(b, a + b))` from naming `a` and `b` alike. A
//! collection that reads an enclosing binder already carries that binder's
//! canonical name, so names encode the full path.
//!
//! Frontends (`cel_to_ir`, `jsonpath`) run this on their output, so user
//! spellings of binder names never reach the IR.

use std::hash::{Hash, Hasher};

use crate::expr_gen::LogExpr;

/// Rename binders canonically and turn bound references into `GetBinding`.
/// Idempotent.
pub fn canonicalize_bindings(expr: &LogExpr) -> LogExpr {
    canon(expr, &[])
}

/// `scope` maps a binder's spelling to its canonical name, innermost last.
fn canon(expr: &LogExpr, scope: &[(String, String)]) -> LogExpr {
    match expr {
        LogExpr::GetFieldByName { field_name } => {
            match scope.iter().rev().find(|(spelled, _)| spelled == field_name) {
                Some((_, canonical)) => LogExpr::GetBinding { binding_name: canonical.clone() },
                None => expr.clone(),
            }
        }
        LogExpr::All { collection, binding, body } => {
            let (collection, binding, body) = binder(collection, binding, body, scope);
            LogExpr::All { collection, binding, body }
        }
        LogExpr::Exists { collection, binding, body } => {
            let (collection, binding, body) = binder(collection, binding, body, scope);
            LogExpr::Exists { collection, binding, body }
        }
        LogExpr::ExistsOne { collection, binding, body } => {
            let (collection, binding, body) = binder(collection, binding, body, scope);
            LogExpr::ExistsOne { collection, binding, body }
        }
        LogExpr::Filter { collection, binding, body } => {
            let (collection, binding, body) = binder(collection, binding, body, scope);
            LogExpr::Filter { collection, binding, body }
        }
        LogExpr::MapTransform { collection, binding, body } => {
            let (collection, binding, body) = binder(collection, binding, body, scope);
            LogExpr::MapTransform { collection, binding, body }
        }
        _ => {
            let mut out = expr.clone();
            for child in out.children_mut() {
                let replaced = canon(child, scope);
                *child = replaced;
            }
            out
        }
    }
}

fn binder(
    collection: &LogExpr,
    binding: &str,
    body: &LogExpr,
    scope: &[(String, String)],
) -> (Box<LogExpr>, String, Box<LogExpr>) {
    let collection = canon(collection, scope);
    let name = canonical_name(&collection, scope.len());
    let mut inner = scope.to_vec();
    inner.push((binding.to_string(), name.clone()));
    let body = canon(body, &inner);
    (Box::new(collection), name, Box::new(body))
}

/// `$` plus a hash of what the variable ranges over and how deeply it is
/// nested. Cannot collide with an identifier, so never with an event field
/// or a parameter. The hash only needs to be consistent within one compiled
/// plan, where names are carried as data.
fn canonical_name(collection: &LogExpr, depth: usize) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    collection.hash(&mut h);
    depth.hash(&mut h);
    format!("${:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cel_to_ir::cel_to_log_expr;

    fn cel(src: &str) -> LogExpr {
        cel_to_log_expr(src).unwrap().expr
    }

    fn body(e: &LogExpr) -> (&str, &LogExpr) {
        match e {
            LogExpr::MapTransform { binding, body, .. } | LogExpr::Filter { binding, body, .. } => (binding, body),
            other => panic!("expected a binder, got {other:?}"),
        }
    }

    #[test]
    fn same_body_over_different_collections_differs() {
        let (ex, ey) = (cel("xs.map(x, x.a)"), cel("ys.map(x, x.a)"));
        let ((bx, body_x), (by, body_y)) = (body(&ex), body(&ey));
        assert_ne!(bx, by);
        assert_ne!(body_x, body_y);
    }

    #[test]
    fn same_collection_merges_regardless_of_spelling() {
        assert_eq!(cel("xs.map(x, x + 1)"), cel("xs.map(y, y + 1)"));
    }

    #[test]
    fn bound_references_become_get_binding() {
        let e = cel("xs.filter(x, x > limit)");
        let (b, body) = body(&e);
        let LogExpr::GreaterThan { lhs, rhs } = body else { panic!() };
        assert_eq!(**lhs, LogExpr::GetBinding { binding_name: b.to_string() });
        // `limit` is free: still an event field.
        assert_eq!(**rhs, LogExpr::GetFieldByName { field_name: "limit".into() });
    }

    #[test]
    fn nesting_over_the_same_collection_keeps_variables_apart() {
        let e = cel("xs.map(a, xs.map(b, a + b))");
        let (outer, body_outer) = body(&e);
        let (inner, body_inner) = body(body_outer);
        assert_ne!(outer, inner);
        let LogExpr::Add { lhs, rhs } = body_inner else { panic!() };
        assert_eq!(**lhs, LogExpr::GetBinding { binding_name: outer.to_string() });
        assert_eq!(**rhs, LogExpr::GetBinding { binding_name: inner.to_string() });
    }

    #[test]
    fn shadowing_resolves_to_the_innermost_binder() {
        let e = cel("xs.map(x, ys.map(x, x))");
        let (_, body_outer) = body(&e);
        let (inner, body_inner) = body(body_outer);
        assert_eq!(*body_inner, LogExpr::GetBinding { binding_name: inner.to_string() });
    }

    #[test]
    fn binding_reads_are_discharged_by_their_binder() {
        use crate::expr_gen::transitive_coeffects;
        let e = cel("xs.map(x, x + 1)");
        assert!(transitive_coeffects(body(&e).1).reads_any_binding());
        assert!(!transitive_coeffects(&e).reads_any_binding());
        // The inner binder discharges `b` but not `a`, so the outer body is
        // still open; the whole expression is closed.
        let e = cel("xs.map(a, ys.map(b, a + b))");
        assert!(transitive_coeffects(body(&e).1).reads_any_binding());
        assert!(!transitive_coeffects(&e).reads_any_binding());
    }

    #[test]
    fn idempotent() {
        let e = cel("xs.map(a, a.items.filter(i, i.n > a.min))");
        assert_eq!(canonicalize_bindings(&e), e);
    }
}
