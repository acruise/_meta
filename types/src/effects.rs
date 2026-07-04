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
///
/// Effect *kinds* (the variant tags here) are meant to be declared in
/// the function catalog -- the single source of truth, the same as the
/// IR node set -- as plain string tags; codegen validates that every
/// per-entry use names a declared kind. A kind's parameter space is not
/// declared and is open: it is supplied at the use-site, not enumerated
/// in the catalog. `MayNull` / `MayPartial` / `MayBlock` /
/// `MayNotConform` / `MayViolateRule` are nullary; `MayError` carries a
/// `ValueType` that only a concrete site knows. So a kind can be named
/// up-front while its inhabitants cannot. (Not yet wired through codegen;
/// see `docs/entity-validation-predicates.md`.)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Effect {
    /// May produce null (nullable field access, empty aggregate window).
    MayNull,
    /// May fail at runtime with an error. The `ValueType` is the error's
    /// type when known; `None` means "may error, type not yet determined"
    /// -- which is the honest answer when an effect kind is parsed from a
    /// bare catalog tag (`may_error`) whose parameter space is open and
    /// supplied only at the use-site. Multiple `MayError` variants with
    /// distinct types can coexist in a single set -- e.g.
    /// `{MayError(Some(String)), MayError(Some(I64))}` means the site can
    /// produce either kind of error.
    MayError(Option<ValueType>),
    /// May produce a result based on incomplete data (aggregate over
    /// a window that isn't full yet).
    MayPartial,
    /// May need to wait for external data before producing a result
    /// (enrichment with Defer policy).
    MayBlock,

    /// May fail *structural conformance*: a value that does not match its
    /// declared `ValueType` -- wrong variant, missing/extra struct field,
    /// null in a non-nullable slot, enum ordinal out of range. This is the
    /// "syntax error" tier of validation: the shape is wrong, independent of
    /// any business meaning. Kept distinct from `MayError` because
    /// a conformance failure is a fixed, type-system-level category, not a
    /// user-defined error value of some result type.
    MayNotConform,

    /// May fail a *higher-level validation predicate* attached to an entity
    /// type -- a numeric bound, length, pattern, enum membership, or a
    /// cross-field rule (see `crate::validation`). The "business rule" tier:
    /// the value conforms structurally but is out of contract. Separate from
    /// `MayNotConform` so a consumer can treat "malformed" and "well-formed
    /// but invalid" differently (e.g. reject at the wire vs quarantine for
    /// review).
    MayViolateRule,
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

    pub fn may_not_conform(&self) -> bool {
        self.0.contains(&Effect::MayNotConform)
    }

    pub fn may_violate_rule(&self) -> bool {
        self.0.contains(&Effect::MayViolateRule)
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

    /// All distinct *known* error types in this set. An untyped
    /// `MayError(None)` contributes nothing here; use [`may_error`] to
    /// test for the presence of any error effect, typed or not.
    ///
    /// [`may_error`]: EffectSet::may_error
    pub fn error_types(&self) -> Vec<&ValueType> {
        self.0.iter()
            .filter_map(|e| match e {
                Effect::MayError(Some(vt)) => Some(vt),
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
        s.insert(Effect::MayError(Some(error_type)));
        s
    }

    /// A site that may error with a type not yet determined (e.g. parsed
    /// from a bare `may_error` catalog tag).
    pub fn fallible_untyped() -> Self {
        let mut s = Self::new();
        s.insert(Effect::MayError(None));
        s
    }

    /// A site that may fail structural conformance ("syntax" tier).
    pub fn nonconforming() -> Self {
        let mut s = Self::new();
        s.insert(Effect::MayNotConform);
        s
    }

    /// A site that may fail a higher-level validation predicate ("business
    /// rule" tier).
    pub fn rule_checked() -> Self {
        let mut s = Self::new();
        s.insert(Effect::MayViolateRule);
        s
    }
}
