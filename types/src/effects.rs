use std::collections::HashSet;
use std::convert::Infallible;
use std::fmt::Debug;
use std::hash::Hash;

use crate::value::ValueType;

/// A host-defined effect kind, carried in [`Effect::Ext`]. The effect-side
/// counterpart of [`crate::coeffects::CoeffectExt`]; `Infallible` is the
/// "no extensions" domain and the default.
pub trait EffectExt: Debug + Clone + Eq + Hash {}

impl EffectExt for Infallible {}

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
/// The kinds here are the ones this crate itself produces (the catalog,
/// codegen, `crate::validation`). Kinds specific to a consumer go in
/// `Ext(X)` with a host-defined `X`; a core-only set lifts into a host
/// domain with [`EffectSet::widen`]. A kind's parameter space is open and
/// supplied at the use-site: `MayNull` / `MayNotConform` /
/// `MayViolateRule` are nullary; `MayError` carries a `ValueType` that
/// only a concrete site knows. So a kind can be named up-front while its
/// inhabitants cannot. (Not yet wired through codegen; see
/// `docs/entity-validation-predicates.md`.)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Effect<X: EffectExt = Infallible> {
    /// May produce null (nullable field access, empty window).
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

    /// A host-defined kind.
    Ext(X),
}

impl<X: EffectExt> Effect<X> {
    /// Re-key the extension domain, leaving core kinds unchanged.
    pub fn map_ext<Y: EffectExt>(self, f: impl FnOnce(X) -> Y) -> Effect<Y> {
        match self {
            Effect::MayNull => Effect::MayNull,
            Effect::MayError(t) => Effect::MayError(t),
            Effect::MayNotConform => Effect::MayNotConform,
            Effect::MayViolateRule => Effect::MayViolateRule,
            Effect::Ext(x) => Effect::Ext(f(x)),
        }
    }
}

/// A set of effects. Wraps a `HashSet<Effect<X>>` with convenience
/// methods for construction and querying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectSet<X: EffectExt = Infallible>(pub HashSet<Effect<X>>);

impl<X: EffectExt> Default for EffectSet<X> {
    fn default() -> Self {
        Self(HashSet::new())
    }
}

impl<X: EffectExt> EffectSet<X> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pure() -> Self {
        Self::new()
    }

    pub fn is_pure(&self) -> bool {
        self.0.is_empty()
    }

    pub fn insert(&mut self, e: Effect<X>) {
        self.0.insert(e);
    }

    pub fn contains(&self, e: &Effect<X>) -> bool {
        self.0.contains(e)
    }

    pub fn may_null(&self) -> bool {
        self.0.contains(&Effect::MayNull)
    }

    pub fn may_error(&self) -> bool {
        self.0.iter().any(|e| matches!(e, Effect::MayError(_)))
    }

    pub fn may_not_conform(&self) -> bool {
        self.0.contains(&Effect::MayNotConform)
    }

    pub fn may_violate_rule(&self) -> bool {
        self.0.contains(&Effect::MayViolateRule)
    }

    /// The extension kinds in this set.
    pub fn ext(&self) -> impl Iterator<Item = &X> {
        self.0.iter().filter_map(|e| match e {
            Effect::Ext(x) => Some(x),
            _ => None,
        })
    }

    pub fn union(&self, other: &EffectSet<X>) -> EffectSet<X> {
        EffectSet(self.0.union(&other.0).cloned().collect())
    }

    /// Re-key the extension domain of every entry.
    pub fn map_ext<Y: EffectExt>(self, f: impl Fn(X) -> Y) -> EffectSet<Y> {
        EffectSet(self.0.into_iter().map(|e| e.map_ext(&f)).collect())
    }

    pub fn ext_kind(x: X) -> Self {
        let mut s = Self::new();
        s.insert(Effect::Ext(x));
        s
    }

    /// Remove all `MayNull` from the set (e.g. after `Coalesce` consumes it).
    pub fn without_null(&self) -> EffectSet<X> {
        EffectSet(self.0.iter().filter(|e| !matches!(e, Effect::MayNull)).cloned().collect())
    }

    /// Remove all `MayError(...)` from the set (e.g. after `TryOrElse` consumes them).
    pub fn without_errors(&self) -> EffectSet<X> {
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

impl EffectSet<Infallible> {
    /// Lift a core-only set into a host's extension domain. Total:
    /// `Infallible` has no values.
    pub fn widen<Y: EffectExt>(self) -> EffectSet<Y> {
        self.map_ext(|n| match n {})
    }
}
