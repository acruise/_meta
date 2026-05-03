use std::collections::HashSet;

use crate::value::ValueType;

/// What an expression may *do* at evaluation time. Dual of coeffects
/// (which describe what an expression reads). The empty set means
/// pure and total: always succeeds, always produces a value.
///
/// Effects compose upward: a parent's effects are the union of its
/// children's, unless the parent explicitly consumes an effect
/// (e.g. `Coalesce` consumes `MayNull`, `TryOrElse` consumes
/// `MayError` variants from its first operand).
///
/// `MayError(ValueType)` is parameterized: a site that can produce
/// errors of different types accumulates all of them. This gives
/// the type checker a compact view of every error type possible at
/// any given expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Effect {
    /// May produce null (nullable field access, empty aggregate window).
    MayNull,
    /// May fail at runtime with an error of the given type.
    /// Multiple `MayError` variants with distinct types can coexist
    /// in a single set -- e.g. `{MayError(String), MayError(I64)}`
    /// means the site can produce either kind of error.
    MayError(ValueType),
    /// May produce a result based on incomplete data (aggregate over
    /// a window that isn't full yet).
    MayPartial,
    /// May need to wait for external data before producing a result
    /// (enrichment with Defer policy).
    MayBlock,
}

/// A set of effects. Wraps a `HashSet<Effect>` with convenience
/// methods for construction and querying.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectSet(pub HashSet<Effect>);

impl EffectSet {
    pub fn new() -> Self {
        Self(HashSet::new())
    }

    pub fn pure() -> Self {
        Self::new()
    }

    pub fn is_pure(&self) -> bool {
        self.0.is_empty()
    }

    pub fn insert(&mut self, e: Effect) {
        self.0.insert(e);
    }

    pub fn contains(&self, e: &Effect) -> bool {
        self.0.contains(e)
    }

    pub fn may_null(&self) -> bool {
        self.0.contains(&Effect::MayNull)
    }

    pub fn may_error(&self) -> bool {
        self.0.iter().any(|e| matches!(e, Effect::MayError(_)))
    }

    pub fn may_partial(&self) -> bool {
        self.0.contains(&Effect::MayPartial)
    }

    pub fn may_block(&self) -> bool {
        self.0.contains(&Effect::MayBlock)
    }

    pub fn union(&self, other: &EffectSet) -> EffectSet {
        EffectSet(self.0.union(&other.0).cloned().collect())
    }

    /// Remove all `MayNull` from the set (e.g. after `Coalesce` consumes it).
    pub fn without_null(&self) -> EffectSet {
        EffectSet(self.0.iter().filter(|e| !matches!(e, Effect::MayNull)).cloned().collect())
    }

    /// Remove all `MayError(...)` from the set (e.g. after `TryOrElse` consumes them).
    pub fn without_errors(&self) -> EffectSet {
        EffectSet(self.0.iter().filter(|e| !matches!(e, Effect::MayError(_))).cloned().collect())
    }

    /// All distinct error types in this set.
    pub fn error_types(&self) -> Vec<&ValueType> {
        self.0.iter()
            .filter_map(|e| match e {
                Effect::MayError(vt) => Some(vt),
                _ => None,
            })
            .collect()
    }

    pub fn nullable() -> Self {
        let mut s = Self::new();
        s.insert(Effect::MayNull);
        s
    }

    pub fn fallible(error_type: ValueType) -> Self {
        let mut s = Self::new();
        s.insert(Effect::MayError(error_type));
        s
    }
}
