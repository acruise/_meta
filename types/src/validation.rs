//! Entity-type validation: the two tiers above raw `Value` storage.
//!
//! There are two distinct ways a value can be wrong, and they correspond to
//! the two failure effects in [`crate::effects`]:
//!
//! - **Conformance (syntax tier, [`Effect::MayNotConform`]).** Does the
//!   `Value` tree match the declared `ValueType` tree -- right variant, right
//!   nesting, nullability honored, enum ordinal in range? This is structural
//!   and needs nothing but the type. [`conformance`] checks it.
//!
//! - **Validation rules (business-rule tier, [`Effect::MayViolateRule`]).**
//!   The value conforms, but is it *in contract* -- `age >= 0`, a non-empty
//!   name, an email that matches a pattern, `start < end`? These are the
//!   finer-grained predicates attached to an entity type, beyond what
//!   `ValueType` alone can say. [`validate`] checks them.
//!
//! The static side of each tier is an [`Effect`]; the runtime side is a
//! [`Violation`] -- the *effect-result datum* produced when the effect fires,
//! carrying a [`Path`] to the offending location plus a message. The two are
//! tied together by [`Tier::effect`] / [`Violation::effect`].
//!
//! Representation of the rule tier is deliberately **hybrid**: a small closed
//! vocabulary of [`Constraint`]s covers the common per-field cases cheaply and
//! portably, and a CEL/LogExpr [`Constraint::Expr`] escape hatch covers
//! everything else (cross-field, conditional). meta-types evaluates the closed
//! vocabulary itself; it cannot depend on the codegen crate, so it hands
//! `Expr` predicates back to the caller (see [`Outcome::deferred`]) to run with
//! the consumer's own expression evaluator.
//!
//! None of this lives in the control-plane store. The rules ride inside the
//! type-declaration payload, which the store treats as opaque bytes -- so the
//! existing versioning, revision-pinning, and epoch machinery carries them
//! with no changes to the algebra, and a consumer enforces them at ingest.

use std::fmt;

use crate::cluonflux::meta as pb;
use std::cmp::Ordering;

use crate::effects::Effect;
use crate::value::{decode_value, encode_value, Value, ValueType};

// ---------------------------------------------------------------------------
// Path -- the navigation payload
// ---------------------------------------------------------------------------

/// One step of a [`Path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSeg {
    /// A struct field, by declared name.
    Field(String),
    /// An array element or map entry, by position in iteration order.
    Index(usize),
}

/// A navigation path into a `Value` / `ValueType` tree: the location of a
/// field, element, or entry. This is the project's way of naming a spot in the
/// type tree; it is the payload on every [`Violation`] and the key on a
/// field-local [`FieldRule`]. Renders as `addr.lines[0]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Path(pub Vec<PathSeg>);

impl Path {
    /// The empty path -- the entity root itself.
    pub fn root() -> Self {
        Path(Vec::new())
    }

    /// A single-segment path naming one root-level field.
    pub fn field(name: impl Into<String>) -> Self {
        Path(vec![PathSeg::Field(name.into())])
    }

    /// A path from a sequence of field names (the common authoring case).
    pub fn fields<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Path(names.into_iter().map(|n| PathSeg::Field(n.into())).collect())
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn push_field(&mut self, name: impl Into<String>) {
        self.0.push(PathSeg::Field(name.into()));
    }

    pub fn push_index(&mut self, i: usize) {
        self.0.push(PathSeg::Index(i));
    }

    pub fn pop(&mut self) {
        self.0.pop();
    }

    /// This path with `prefix` prepended (`prefix` outer, `self` inner).
    pub fn prepend(&self, prefix: &Path) -> Path {
        let mut segs = Vec::with_capacity(prefix.0.len() + self.0.len());
        segs.extend(prefix.0.iter().cloned());
        segs.extend(self.0.iter().cloned());
        Path(segs)
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("(root)");
        }
        for (i, seg) in self.0.iter().enumerate() {
            match seg {
                PathSeg::Field(name) => {
                    if i > 0 {
                        f.write_str(".")?;
                    }
                    f.write_str(name)?;
                }
                PathSeg::Index(idx) => write!(f, "[{idx}]")?,
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Effect-result data
// ---------------------------------------------------------------------------

/// Which tier a [`Violation`] belongs to. Maps one-to-one onto the failure
/// effects so a caller can fold a run's violations back into an effect set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Structural `ValueType` mismatch. Pairs with [`Effect::MayNotConform`].
    Conformance,
    /// A higher-level predicate failed. Pairs with [`Effect::MayViolateRule`].
    Rule,
}

impl Tier {
    /// The static effect this tier discharges at runtime.
    pub fn effect(self) -> Effect {
        match self {
            Tier::Conformance => Effect::MayNotConform,
            Tier::Rule => Effect::MayViolateRule,
        }
    }
}

/// The datum produced when a failure effect fires: where it failed, which rule,
/// and a human-readable message. This is the runtime dual of the static
/// [`Effect`] -- construct one with [`Violation::not_conform`] (the
/// [`Tier::Conformance`] / [`Effect::MayNotConform`] case) or
/// [`Violation::rule`] (the [`Tier::Rule`] / [`Effect::MayViolateRule`] case).
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub tier: Tier,
    /// Where it failed; the root path means the entity itself.
    pub path: Path,
    /// The rule name (`"conformance"`, `"min"`, `"pattern"`, ...).
    pub rule: &'static str,
    pub message: String,
}

impl Violation {
    /// A structural-conformance failure (`MayNotConform`).
    pub fn not_conform(path: Path, message: impl Into<String>) -> Self {
        Violation { tier: Tier::Conformance, path, rule: "conformance", message: message.into() }
    }

    /// A business-rule failure (`MayViolateRule`), tagged with the rule name.
    pub fn rule(path: Path, rule: &'static str, message: impl Into<String>) -> Self {
        Violation { tier: Tier::Rule, path, rule, message: message.into() }
    }

    /// The static effect this violation realizes.
    pub fn effect(&self) -> Effect {
        self.tier.effect()
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.rule, self.path, self.message)
    }
}

// ---------------------------------------------------------------------------
// Constraints (the rule vocabulary)
// ---------------------------------------------------------------------------

/// A `Min`/`Max` bound: an ordered value plus whether the boundary value
/// itself is accepted. Exclusivity matters most for floating point and
/// timestamps, where "strictly before" / "strictly greater than" is the
/// natural constraint and an inclusive bound would wrongly admit the exact
/// boundary instant.
#[derive(Debug, Clone, PartialEq)]
pub struct Bound {
    pub value: Value,
    /// `true`: the boundary value passes (a closed bound, `>=` / `<=`).
    /// `false`: the boundary value fails (an open bound, `>` / `<`).
    pub inclusive: bool,
}

impl Bound {
    pub fn inclusive(value: Value) -> Self {
        Bound { value, inclusive: true }
    }

    pub fn exclusive(value: Value) -> Self {
        Bound { value, inclusive: false }
    }
}

/// One constraint from the closed vocabulary, or the expression escape hatch.
///
/// The bound variants (`Min`/`Max`) carry a [`Bound`] -- a full `Value`, not
/// an `f64` -- so they apply to any ordered type (integers, floats, decimals,
/// dates, timestamps, strings, byte strings) and never coerce through `f64`,
/// so no precision is lost (two distinct large `i64`s are never conflated).
/// Comparison is exact within a family; a bound whose family does not match
/// the value is ignored. Decimals compare by raw mantissa, assuming the bound
/// is authored at the field's scale.
#[derive(Debug, Clone, PartialEq)]
pub enum Constraint {
    /// Lower bound; see [`Bound`] for the inclusive/exclusive flag.
    Min(Bound),
    /// Upper bound; see [`Bound`].
    Max(Bound),
    /// Inclusive lower bound on length: characters for `String`, bytes for
    /// `Blob`/`Clob`, elements for `Array`, entries for `Map`.
    MinLen(u64),
    /// Inclusive upper bound on length.
    MaxLen(u64),
    /// Length must be at least 1.
    NonEmpty,
    /// A `String` must fully match this regular expression.
    Pattern(String),
    /// The value must equal one of these (compared loosely across integer
    /// widths, so a literal `5` matches an `I32` or `I64` field).
    OneOf(Vec<Value>),
    /// A CEL/LogExpr boolean predicate, carried as source and evaluated by the
    /// caller. Surfaced in [`Outcome::deferred`] rather than checked here.
    Expr(String),
}

impl Constraint {
    /// The short, stable rule name used in [`Violation::rule`].
    fn name(&self) -> &'static str {
        match self {
            Constraint::Min(_) => "min",
            Constraint::Max(_) => "max",
            Constraint::MinLen(_) => "min_len",
            Constraint::MaxLen(_) => "max_len",
            Constraint::NonEmpty => "non_empty",
            Constraint::Pattern(_) => "pattern",
            Constraint::OneOf(_) => "one_of",
            Constraint::Expr(_) => "expr",
        }
    }
}

/// The constraints attached to one field path.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldRule {
    pub path: Path,
    pub constraints: Vec<Constraint>,
}

/// The full set of validation rules for an entity type: field-local
/// constraints plus whole-entity (typically cross-field) predicates.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntityValidation {
    pub fields: Vec<FieldRule>,
    pub entity: Vec<Constraint>,
}

impl EntityValidation {
    /// Conjunction: a value must satisfy both rule sets. Because [`validate`]
    /// already fails on *any* violation, this is just the union of all rules
    /// -- and adding rules can only ever *narrow* the accepted set, never
    /// widen it. That monotonicity is what makes the use-site predicates on a
    /// `ValueTypeRef` safe to compose with the referenced declaration's
    /// baseline: a reference may demand a stricter subset, never a looser one.
    pub fn and(&self, other: &EntityValidation) -> EntityValidation {
        EntityValidation {
            fields: self.fields.iter().chain(&other.fields).cloned().collect(),
            entity: self.entity.iter().chain(&other.entity).cloned().collect(),
        }
    }

    /// Rebase every rule under `prefix`, for embedding a referenced type's
    /// rules at the location where it is used. Field rules get `prefix`
    /// prepended to their path. Whole-entity rules -- which were scoped to the
    /// referenced type's own root -- become field rules at `prefix`, since
    /// that root now lives at `prefix` in the embedding type. (A `prefix` of
    /// the root is the identity.)
    pub fn prefixed(&self, prefix: &Path) -> EntityValidation {
        if prefix.is_root() {
            return self.clone();
        }
        let mut fields: Vec<FieldRule> = self
            .fields
            .iter()
            .map(|r| FieldRule { path: r.path.prepend(prefix), constraints: r.constraints.clone() })
            .collect();
        if !self.entity.is_empty() {
            fields.push(FieldRule { path: prefix.clone(), constraints: self.entity.clone() });
        }
        EntityValidation { fields, entity: Vec::new() }
    }
}

/// An `Expr` predicate that meta-types could not evaluate, resolved to the
/// scope it applies to. The caller runs it with its LogExpr evaluator; a
/// false result becomes a [`Tier::Rule`] [`Violation`] on `path`.
#[derive(Debug, Clone, PartialEq)]
pub struct DeferredExpr {
    pub path: Path,
    pub source: String,
}

/// The result of a validation pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    pub violations: Vec<Violation>,
    pub deferred: Vec<DeferredExpr>,
}

impl Outcome {
    /// True when nothing failed the checks performed here. Note this does not
    /// account for [`Outcome::deferred`] expressions, which the caller must
    /// still evaluate.
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    /// The set of failure effects realized by this outcome. A clean outcome
    /// over a fully-checked value realizes none.
    pub fn effects(&self) -> Vec<Effect> {
        let mut conform = false;
        let mut rule = false;
        for v in &self.violations {
            match v.tier {
                Tier::Conformance => conform = true,
                Tier::Rule => rule = true,
            }
        }
        let mut out = Vec::new();
        if conform {
            out.push(Effect::MayNotConform);
        }
        if rule {
            out.push(Effect::MayViolateRule);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Conformance (syntax tier)
// ---------------------------------------------------------------------------

/// Structurally check a value against its declared type. Returns one violation
/// per mismatch (it does not stop at the first). The root is treated as
/// non-nullable; nullability of nested slots is read from their containers
/// (`StructField::nullable`, `elements_nullable`, `values_nullable`).
pub fn conformance(value: &Value, vt: &ValueType) -> Vec<Violation> {
    let mut out = Vec::new();
    check_conformance(value, vt, false, &mut Path::root(), &mut out);
    out
}

fn check_conformance(
    value: &Value,
    vt: &ValueType,
    nullable: bool,
    path: &mut Path,
    out: &mut Vec<Violation>,
) {
    if value.is_null() {
        if !nullable {
            out.push(Violation::not_conform(path.clone(), "null in a non-nullable position"));
        }
        return;
    }

    match vt {
        ValueType::Struct { fields } => {
            let Value::Struct(vals) = value else {
                out.push(Violation::not_conform(path.clone(), format!("expected struct, found {}", kind(value))));
                return;
            };
            if vals.len() != fields.len() {
                out.push(Violation::not_conform(
                    path.clone(),
                    format!("struct arity {} != declared {}", vals.len(), fields.len()),
                ));
            }
            for (f, v) in fields.iter().zip(vals.iter()) {
                path.push_field(f.name.clone());
                check_conformance(v, &f.value_type, f.nullable, path, out);
                path.pop();
            }
        }
        ValueType::Array { element_type, elements_nullable } => {
            let Value::Array(elems) = value else {
                out.push(Violation::not_conform(path.clone(), format!("expected array, found {}", kind(value))));
                return;
            };
            for (i, e) in elems.iter().enumerate() {
                path.push_index(i);
                check_conformance(e, element_type, *elements_nullable, path, out);
                path.pop();
            }
        }
        ValueType::Map { value_type, values_nullable, .. } => {
            let Value::Map(entries) = value else {
                out.push(Violation::not_conform(path.clone(), format!("expected map, found {}", kind(value))));
                return;
            };
            for (i, v) in entries.values().enumerate() {
                path.push_index(i);
                check_conformance(v, value_type, *values_nullable, path, out);
                path.pop();
            }
        }
        ValueType::Enum { values } => match value {
            Value::Enum(ord) => {
                if !values.is_empty() && (*ord as usize) >= values.len() {
                    out.push(Violation::not_conform(path.clone(), format!("enum ordinal {ord} out of range")));
                }
            }
            _ => out.push(Violation::not_conform(path.clone(), format!("expected enum, found {}", kind(value)))),
        },
        // Scalar leaves: the variant must line up.
        _ => {
            if !scalar_conforms(value, vt) {
                out.push(Violation::not_conform(path.clone(), format!("{} is not a {:?}", kind(value), vt)));
            }
        }
    }
}

/// Whether a non-null scalar value matches a scalar `ValueType`. Compound and
/// enum types are handled by the caller.
fn scalar_conforms(value: &Value, vt: &ValueType) -> bool {
    matches!(
        (value, vt),
        (Value::Bool(_), ValueType::Bool)
            | (Value::I8(_), ValueType::I8)
            | (Value::I16(_), ValueType::I16)
            | (Value::I32(_), ValueType::I32)
            | (Value::I64(_), ValueType::I64)
            | (Value::U8(_), ValueType::U8)
            | (Value::U16(_), ValueType::U16)
            | (Value::U32(_), ValueType::U32)
            | (Value::U64(_), ValueType::U64)
            | (Value::F32(_), ValueType::F32)
            | (Value::F64(_), ValueType::F64)
            | (Value::Date(_), ValueType::Date)
            | (Value::Uuid(_), ValueType::Uuid)
            | (Value::Ipv4(_), ValueType::Ipv4)
            | (Value::Ipv6(_), ValueType::Ipv6)
            | (Value::Blob(_), ValueType::Blob)
            | (Value::Clob(_), ValueType::Clob)
            | (Value::String(_), ValueType::String)
            | (Value::DecimalI64(_), ValueType::Decimal { .. })
            | (Value::DecimalI128(_), ValueType::Decimal { .. })
            | (Value::Timestamp(_), ValueType::Timestamp { .. })
            | (Value::TimestampTz(_, _), ValueType::Timestamp { .. })
    )
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::I8(_) | Value::I16(_) | Value::I32(_) | Value::I64(_) => "int",
        Value::U8(_) | Value::U16(_) | Value::U32(_) | Value::U64(_) => "uint",
        Value::F32(_) | Value::F64(_) => "float",
        Value::Date(_) => "date",
        Value::Uuid(_) => "uuid",
        Value::Ipv4(_) => "ipv4",
        Value::Ipv6(_) => "ipv6",
        Value::Blob(_) => "blob",
        Value::Clob(_) => "clob",
        Value::String(_) => "string",
        Value::DecimalI64(_) | Value::DecimalI128(_) => "decimal",
        Value::Timestamp(_) | Value::TimestampTz(_, _) => "timestamp",
        Value::Enum(_) => "enum",
        Value::Array(_) => "array",
        Value::Map(_) => "map",
        Value::Struct(_) => "struct",
    }
}

// ---------------------------------------------------------------------------
// Validation rules (business-rule tier)
// ---------------------------------------------------------------------------

/// Evaluate the closed-vocabulary rules of `rules` against `value` (typed by
/// `vt`), collecting violations and any `Expr` predicates that must be
/// evaluated by the caller. Assumes `value` already conforms (run
/// [`conformance`] first); a rule whose field path does not resolve is
/// skipped, since an absent or null ancestor is a conformance/nullability
/// concern, not a rule concern.
pub fn validate(value: &Value, vt: &ValueType, rules: &EntityValidation) -> Outcome {
    let mut out = Outcome::default();

    for fr in &rules.fields {
        if let Some(target) = resolve_path(value, vt, &fr.path) {
            for c in &fr.constraints {
                check_rule(c, target, &fr.path, &mut out);
            }
        }
    }

    for c in &rules.entity {
        check_rule(c, value, &Path::root(), &mut out);
    }

    out
}

fn check_rule(c: &Constraint, value: &Value, path: &Path, out: &mut Outcome) {
    // Null is governed by nullability (the conformance tier), not by rules.
    // The expression escape hatch is the one thing that still wants to see it.
    if value.is_null() && !matches!(c, Constraint::Expr(_)) {
        return;
    }

    let mut fail = |msg: String| out.violations.push(Violation::rule(path.clone(), c.name(), msg));

    match c {
        Constraint::Min(b) => {
            if let Some(ord) = compare(value, &b.value) {
                if bound_violated(ord, true, b.inclusive) {
                    let cmp = if b.inclusive { ">=" } else { ">" };
                    fail(format!("{value:?} fails min (must be {cmp} {:?})", b.value));
                }
            }
        }
        Constraint::Max(b) => {
            if let Some(ord) = compare(value, &b.value) {
                if bound_violated(ord, false, b.inclusive) {
                    let cmp = if b.inclusive { "<=" } else { "<" };
                    fail(format!("{value:?} fails max (must be {cmp} {:?})", b.value));
                }
            }
        }
        Constraint::MinLen(n) => {
            if let Some(len) = len_of(value) {
                if (len as u64) < *n {
                    fail(format!("length {len} < min_len {n}"));
                }
            }
        }
        Constraint::MaxLen(n) => {
            if let Some(len) = len_of(value) {
                if (len as u64) > *n {
                    fail(format!("length {len} > max_len {n}"));
                }
            }
        }
        Constraint::NonEmpty => {
            if let Some(0) = len_of(value) {
                fail("must not be empty".into());
            }
        }
        Constraint::Pattern(p) => {
            if let Value::String(s) = value {
                match regex::Regex::new(p) {
                    Ok(re) => {
                        if !re.is_match(s) {
                            fail(format!("{s:?} does not match /{p}/"));
                        }
                    }
                    Err(_) => fail(format!("invalid pattern /{p}/")),
                }
            }
        }
        Constraint::OneOf(opts) => {
            if !opts.iter().any(|o| values_match(o, value)) {
                fail("not one of the permitted values".into());
            }
        }
        Constraint::Expr(src) => {
            out.deferred.push(DeferredExpr { path: path.clone(), source: src.clone() });
        }
    }
}

/// Resolve a navigation path against a (value, type) pair, walking both in
/// lockstep. Returns `None` if any segment is missing or an ancestor is null.
fn resolve_path<'a>(value: &'a Value, vt: &'a ValueType, path: &Path) -> Option<&'a Value> {
    let mut v = value;
    let mut t = vt;
    for seg in &path.0 {
        match seg {
            PathSeg::Field(name) => {
                let ValueType::Struct { fields } = t else {
                    return None;
                };
                let idx = fields.iter().position(|f| &f.name == name)?;
                let Value::Struct(vals) = v else {
                    return None;
                };
                v = vals.get(idx)?;
                t = &fields[idx].value_type;
            }
            PathSeg::Index(i) => match (v, t) {
                (Value::Array(elems), ValueType::Array { element_type, .. }) => {
                    v = elems.get(*i)?;
                    t = element_type;
                }
                (Value::Map(entries), ValueType::Map { value_type, .. }) => {
                    v = entries.values().nth(*i)?;
                    t = value_type;
                }
                _ => return None,
            },
        }
    }
    Some(v)
}

fn len_of(v: &Value) -> Option<usize> {
    Some(match v {
        Value::String(s) => s.chars().count(),
        Value::Blob(b) | Value::Clob(b) => b.len(),
        Value::Array(a) => a.len(),
        Value::Map(m) => m.len(),
        _ => return None,
    })
}

/// Whether `ord` (the value compared against the bound) violates a min/max.
fn bound_violated(ord: Ordering, is_min: bool, inclusive: bool) -> bool {
    match (is_min, ord) {
        (true, Ordering::Less) => true,
        (false, Ordering::Greater) => true,
        // On the boundary, an exclusive bound rejects, an inclusive one accepts.
        (_, Ordering::Equal) => !inclusive,
        _ => false,
    }
}

/// Exact comparison within an ordered family. `None` when the two values are
/// not comparable -- different families, or a NaN float -- in which case a
/// mismatched bound is simply ignored. Integer widths are compared through
/// `i128` (every `I*`/`U*` value fits), never through `f64`, so no precision is
/// lost; floats compare only to floats, strings to strings, and so on.
fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    if let (Some(x), Some(y)) = (as_i128(a), as_i128(b)) {
        return Some(x.cmp(&y));
    }
    match (a, b) {
        (Value::F32(_) | Value::F64(_), Value::F32(_) | Value::F64(_)) => {
            as_float(a).partial_cmp(&as_float(b))
        }
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        (Value::Blob(x), Value::Blob(y)) | (Value::Clob(x), Value::Clob(y)) => Some(x.cmp(y)),
        (Value::Date(x), Value::Date(y)) => Some(x.cmp(y)),
        (Value::Timestamp(x), Value::Timestamp(y)) => Some(x.cmp(y)),
        (Value::DecimalI64(x), Value::DecimalI64(y)) => Some(x.cmp(y)),
        (Value::DecimalI128(x), Value::DecimalI128(y)) => Some(x.cmp(y)),
        (Value::DecimalI64(x), Value::DecimalI128(y)) => Some((*x as i128).cmp(y)),
        (Value::DecimalI128(x), Value::DecimalI64(y)) => Some(x.cmp(&(*y as i128))),
        _ => None,
    }
}

/// Every integer `Value` widened to `i128` (all `I*`/`U*` fit losslessly).
fn as_i128(v: &Value) -> Option<i128> {
    Some(match v {
        Value::I8(n) => *n as i128,
        Value::I16(n) => *n as i128,
        Value::I32(n) => *n as i128,
        Value::I64(n) => *n as i128,
        Value::U8(n) => *n as i128,
        Value::U16(n) => *n as i128,
        Value::U32(n) => *n as i128,
        Value::U64(n) => *n as i128,
        _ => return None,
    })
}

fn as_float(v: &Value) -> f64 {
    match v {
        Value::F32(n) => *n as f64,
        Value::F64(n) => *n,
        _ => unreachable!("as_float called on a non-float value"),
    }
}

/// Equality for `OneOf`: exact, but with integer widths interoperating (a
/// literal `5` matches an `I32` or `I64` field) -- still without going through
/// `f64`, so large integers are never conflated.
fn values_match(a: &Value, b: &Value) -> bool {
    compare(a, b) == Some(Ordering::Equal) || a == b
}

// ---------------------------------------------------------------------------
// Proto serde
// ---------------------------------------------------------------------------

impl From<&Path> for pb::Path {
    fn from(p: &Path) -> Self {
        use pb::path_segment::Seg;
        pb::Path {
            segments: p
                .0
                .iter()
                .map(|s| pb::PathSegment {
                    seg: Some(match s {
                        PathSeg::Field(name) => Seg::Field(name.clone()),
                        PathSeg::Index(i) => Seg::Index(*i as u64),
                    }),
                })
                .collect(),
        }
    }
}

impl From<&pb::Path> for Path {
    fn from(p: &pb::Path) -> Self {
        use pb::path_segment::Seg;
        Path(
            p.segments
                .iter()
                .filter_map(|s| match &s.seg {
                    Some(Seg::Field(name)) => Some(PathSeg::Field(name.clone())),
                    Some(Seg::Index(i)) => Some(PathSeg::Index(*i as usize)),
                    None => None,
                })
                .collect(),
        )
    }
}

impl From<&EntityValidation> for pb::EntityValidation {
    fn from(v: &EntityValidation) -> Self {
        pb::EntityValidation {
            fields: v.fields.iter().map(pb::FieldRule::from).collect(),
            entity: v.entity.iter().map(pb::Constraint::from).collect(),
        }
    }
}

impl From<&pb::EntityValidation> for EntityValidation {
    fn from(p: &pb::EntityValidation) -> Self {
        EntityValidation {
            fields: p.fields.iter().map(FieldRule::from).collect(),
            entity: p.entity.iter().map(Constraint::from).collect(),
        }
    }
}

impl From<&FieldRule> for pb::FieldRule {
    fn from(r: &FieldRule) -> Self {
        pb::FieldRule {
            path: Some(pb::Path::from(&r.path)),
            constraints: r.constraints.iter().map(pb::Constraint::from).collect(),
        }
    }
}

impl From<&pb::FieldRule> for FieldRule {
    fn from(p: &pb::FieldRule) -> Self {
        FieldRule {
            path: p.path.as_ref().map(Path::from).unwrap_or_default(),
            constraints: p.constraints.iter().map(Constraint::from).collect(),
        }
    }
}

impl From<&Bound> for pb::Bound {
    fn from(b: &Bound) -> Self {
        pb::Bound {
            value: Some(encode_value(&b.value, &ValueType::Null)),
            inclusive: b.inclusive,
        }
    }
}

impl From<&pb::Bound> for Bound {
    fn from(p: &pb::Bound) -> Self {
        Bound {
            value: p.value.as_ref().map(|v| decode_value(v, &ValueType::Null)).unwrap_or(Value::Null),
            inclusive: p.inclusive,
        }
    }
}

impl From<&Constraint> for pb::Constraint {
    fn from(c: &Constraint) -> Self {
        use pb::constraint::Kind;
        // Bound and OneOf values are encoded with a `Null` companion type:
        // they are scalars, and comparison re-widens integers exactly (via
        // `compare`), so the wire's width normalization is harmless.
        let kind = match c {
            Constraint::Min(b) => Kind::Min(pb::Bound::from(b)),
            Constraint::Max(b) => Kind::Max(pb::Bound::from(b)),
            Constraint::MinLen(n) => Kind::MinLen(*n),
            Constraint::MaxLen(n) => Kind::MaxLen(*n),
            Constraint::NonEmpty => Kind::NonEmpty(true),
            Constraint::Pattern(p) => Kind::Pattern(p.clone()),
            Constraint::OneOf(vals) => Kind::OneOf(pb::OneOfConstraint {
                values: vals.iter().map(|v| encode_value(v, &ValueType::Null)).collect(),
            }),
            Constraint::Expr(s) => Kind::Expr(s.clone()),
        };
        pb::Constraint { kind: Some(kind) }
    }
}

impl From<&pb::Constraint> for Constraint {
    fn from(p: &pb::Constraint) -> Self {
        use pb::constraint::Kind;
        match &p.kind {
            Some(Kind::Min(b)) => Constraint::Min(Bound::from(b)),
            Some(Kind::Max(b)) => Constraint::Max(Bound::from(b)),
            Some(Kind::MinLen(n)) => Constraint::MinLen(*n),
            Some(Kind::MaxLen(n)) => Constraint::MaxLen(*n),
            Some(Kind::NonEmpty(_)) => Constraint::NonEmpty,
            Some(Kind::Pattern(p)) => Constraint::Pattern(p.clone()),
            Some(Kind::OneOf(o)) => Constraint::OneOf(
                o.values.iter().map(|v| decode_value(v, &ValueType::Null)).collect(),
            ),
            Some(Kind::Expr(s)) => Constraint::Expr(s.clone()),
            // An empty oneof is meaningless; model it as a vacuous expr.
            None => Constraint::Expr(String::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::StructField;

    fn person_type() -> ValueType {
        ValueType::Struct {
            fields: vec![
                StructField { name: "name".into(), human_name: "".into(), value_type: ValueType::String, nullable: false },
                StructField { name: "age".into(), human_name: "".into(), value_type: ValueType::I64, nullable: true },
            ],
        }
    }

    fn person(name: &str, age: Option<i64>) -> Value {
        Value::Struct(vec![
            Value::String(name.into()),
            age.map(Value::I64).unwrap_or(Value::Null),
        ])
    }

    #[test]
    fn path_display() {
        assert_eq!(Path::root().to_string(), "(root)");
        assert_eq!(Path::field("age").to_string(), "age");
        let mut p = Path::fields(["addr", "lines"]);
        p.push_index(0);
        assert_eq!(p.to_string(), "addr.lines[0]");
    }

    #[test]
    fn conformance_accepts_well_shaped() {
        assert!(conformance(&person("alice", Some(30)), &person_type()).is_empty());
        // null in the nullable age slot is fine.
        assert!(conformance(&person("bob", None), &person_type()).is_empty());
    }

    #[test]
    fn conformance_rejects_null_in_non_nullable() {
        let v = Value::Struct(vec![Value::Null, Value::I64(30)]);
        let viol = conformance(&v, &person_type());
        assert_eq!(viol.len(), 1);
        assert_eq!(viol[0].tier, Tier::Conformance);
        assert_eq!(viol[0].effect(), Effect::MayNotConform);
        assert_eq!(viol[0].path, Path::field("name"));
    }

    #[test]
    fn conformance_rejects_wrong_variant() {
        let v = Value::Struct(vec![Value::String("x".into()), Value::String("not-an-int".into())]);
        let viol = conformance(&v, &person_type());
        assert_eq!(viol.len(), 1);
        assert_eq!(viol[0].path, Path::field("age"));
    }

    #[test]
    fn conformance_path_into_nested_array() {
        let vt = ValueType::Struct {
            fields: vec![StructField {
                name: "tags".into(),
                human_name: "".into(),
                value_type: ValueType::Array { element_type: Box::new(ValueType::String), elements_nullable: false },
                nullable: false,
            }],
        };
        let v = Value::Struct(vec![Value::Array(vec![Value::String("ok".into()), Value::I64(7)])]);
        let viol = conformance(&v, &vt);
        assert_eq!(viol.len(), 1);
        assert_eq!(viol[0].path.to_string(), "tags[1]");
    }

    #[test]
    fn rule_min_bound() {
        let rules = EntityValidation {
            fields: vec![FieldRule { path: Path::field("age"), constraints: vec![Constraint::Min(Bound::inclusive(Value::I64(0)))] }],
            entity: vec![],
        };
        assert!(validate(&person("alice", Some(30)), &person_type(), &rules).is_clean());

        let out = validate(&person("alice", Some(-5)), &person_type(), &rules);
        assert_eq!(out.violations.len(), 1);
        assert_eq!(out.violations[0].rule, "min");
        assert_eq!(out.effects(), vec![Effect::MayViolateRule]);
    }

    #[test]
    fn rule_non_empty_and_pattern() {
        let rules = EntityValidation {
            fields: vec![FieldRule {
                path: Path::field("name"),
                constraints: vec![Constraint::NonEmpty, Constraint::Pattern("^[a-z]+$".into())],
            }],
            entity: vec![],
        };
        assert!(validate(&person("alice", Some(1)), &person_type(), &rules).is_clean());

        let empty = validate(&person("", Some(1)), &person_type(), &rules);
        // Empty fails both non_empty and the pattern.
        assert_eq!(empty.violations.len(), 2);

        let bad = validate(&person("Alice1", Some(1)), &person_type(), &rules);
        assert_eq!(bad.violations.len(), 1);
        assert_eq!(bad.violations[0].rule, "pattern");
    }

    #[test]
    fn bounds_on_strings() {
        // Lexicographic min on a string field -- f64 bounds could never do this.
        let rules = EntityValidation {
            fields: vec![FieldRule {
                path: Path::field("name"),
                constraints: vec![Constraint::Min(Bound::inclusive(Value::String("m".into())))],
            }],
            entity: vec![],
        };
        assert!(validate(&person("nora", Some(1)), &person_type(), &rules).is_clean());
        assert_eq!(validate(&person("alice", Some(1)), &person_type(), &rules).violations.len(), 1);
    }

    #[test]
    fn bounds_are_exact_not_f64() {
        // 2^53 and 2^53 + 1 are distinct i64 but collapse to the same f64.
        let lo = 9_007_199_254_740_992_i64; // 2^53
        let hi = 9_007_199_254_740_993_i64; // 2^53 + 1
        let ty = ValueType::Struct {
            fields: vec![StructField { name: "n".into(), human_name: "".into(), value_type: ValueType::I64, nullable: false }],
        };
        let val = Value::Struct(vec![Value::I64(hi)]);
        let rules = EntityValidation {
            fields: vec![FieldRule { path: Path::field("n"), constraints: vec![Constraint::Max(Bound::inclusive(Value::I64(lo)))] }],
            entity: vec![],
        };
        // hi > lo, so max is violated -- an f64 comparison would miss this.
        assert_eq!(validate(&val, &ty, &rules).violations.len(), 1);
    }

    #[test]
    fn exclusive_bound_rejects_the_boundary() {
        let ty = ValueType::Struct {
            fields: vec![StructField { name: "x".into(), human_name: "".into(), value_type: ValueType::F64, nullable: false }],
        };
        let at_zero = Value::Struct(vec![Value::F64(0.0)]);

        let inclusive = EntityValidation {
            fields: vec![FieldRule { path: Path::field("x"), constraints: vec![Constraint::Min(Bound::inclusive(Value::F64(0.0)))] }],
            entity: vec![],
        };
        let exclusive = EntityValidation {
            fields: vec![FieldRule { path: Path::field("x"), constraints: vec![Constraint::Min(Bound::exclusive(Value::F64(0.0)))] }],
            entity: vec![],
        };
        // The boundary value passes an inclusive min, fails an exclusive one.
        assert!(validate(&at_zero, &ty, &inclusive).is_clean());
        assert_eq!(validate(&at_zero, &ty, &exclusive).violations.len(), 1);
    }

    #[test]
    fn rule_one_of_loose_int_width() {
        let rules = EntityValidation {
            fields: vec![FieldRule {
                path: Path::field("age"),
                // Authored as I32 literals; the field is I64.
                constraints: vec![Constraint::OneOf(vec![Value::I32(18), Value::I32(21)])],
            }],
            entity: vec![],
        };
        assert!(validate(&person("a", Some(21)), &person_type(), &rules).is_clean());
        assert_eq!(validate(&person("a", Some(20)), &person_type(), &rules).violations.len(), 1);
    }

    #[test]
    fn null_field_skips_rules() {
        let rules = EntityValidation {
            fields: vec![FieldRule { path: Path::field("age"), constraints: vec![Constraint::Min(Bound::inclusive(Value::I64(0)))] }],
            entity: vec![],
        };
        // age is null -> the bound is not a rule concern, so no violation.
        assert!(validate(&person("a", None), &person_type(), &rules).is_clean());
    }

    #[test]
    fn expr_is_deferred_not_evaluated() {
        let rules = EntityValidation {
            fields: vec![],
            entity: vec![Constraint::Expr("age >= 0 && size(name) > 0".into())],
        };
        let out = validate(&person("a", Some(1)), &person_type(), &rules);
        assert!(out.is_clean());
        assert_eq!(out.deferred.len(), 1);
        assert_eq!(out.deferred[0].source, "age >= 0 && size(name) > 0");
        assert_eq!(out.deferred[0].path, Path::root());
    }

    #[test]
    fn and_only_narrows() {
        // Declaration baseline: age >= 0. Use-site adds: age <= 65.
        let decl = EntityValidation {
            fields: vec![FieldRule { path: Path::field("age"), constraints: vec![Constraint::Min(Bound::inclusive(Value::I64(0)))] }],
            entity: vec![],
        };
        let use_site = EntityValidation {
            fields: vec![FieldRule { path: Path::field("age"), constraints: vec![Constraint::Max(Bound::inclusive(Value::I64(65)))] }],
            entity: vec![],
        };
        let combined = decl.and(&use_site);

        // Accepted by the declaration alone, rejected by the narrowed combo.
        assert!(validate(&person("a", Some(40)), &person_type(), &decl).is_clean());
        assert!(validate(&person("a", Some(80)), &person_type(), &decl).is_clean());
        assert!(validate(&person("a", Some(40)), &person_type(), &combined).is_clean());
        assert_eq!(validate(&person("a", Some(80)), &person_type(), &combined).violations.len(), 1);
    }

    #[test]
    fn prefixed_rebases_a_referenced_types_rules() {
        // A referenced type with a field rule (age >= 0) and a cross-field
        // entity rule, embedded at "owner" in the outer type.
        let referenced = EntityValidation {
            fields: vec![FieldRule { path: Path::field("age"), constraints: vec![Constraint::Min(Bound::inclusive(Value::I64(0)))] }],
            entity: vec![Constraint::Expr("name != ''".into())],
        };
        let at_owner = referenced.prefixed(&Path::field("owner"));

        // Field rule moved to owner.age; the entity rule became a field rule at owner.
        assert_eq!(at_owner.fields[0].path, Path::fields(["owner", "age"]));
        assert_eq!(at_owner.entity.len(), 0);
        let expr_rule = at_owner.fields.iter().find(|r| r.path == Path::field("owner")).unwrap();
        assert!(matches!(expr_rule.constraints[0], Constraint::Expr(_)));
    }

    #[test]
    fn validation_proto_round_trip() {
        let rules = EntityValidation {
            fields: vec![FieldRule {
                path: Path::fields(["addr", "zip"]),
                constraints: vec![
                    Constraint::Min(Bound::inclusive(Value::I64(0))),
                    Constraint::Max(Bound::exclusive(Value::I64(150))),
                    Constraint::OneOf(vec![Value::I64(18), Value::I64(21)]),
                ],
            }],
            entity: vec![Constraint::Expr("start < end".into())],
        };
        let proto = pb::EntityValidation::from(&rules);
        let back = EntityValidation::from(&proto);
        assert_eq!(back, rules);
    }
}
