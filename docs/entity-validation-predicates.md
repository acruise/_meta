# Entity-type validation predicates

**Status:** Draft + first cut, 2026-06-29. The closed-vocabulary tier is implemented in `meta-types` (`rust-types/src/validation.rs`); the expression escape hatch and the consumer wiring are designed here but not yet built.

## What this is

Finer-grained validation attached to entity types, beyond the structural `Value`/`ValueType` conformance that is the type system's baseline. Conformance answers "is this the right shape"; these predicates answer "is this in contract" -- `age >= 0`, a non-empty name, an email matching a pattern, `start_date < end_date`. The slot was reserved long before it was filled: `value.proto`'s `ValueTypeRef` already promised a declaration that "resolves to a full ValueType (plus validation rules that are enforced at ingest time)."

## Two tiers, two effects

A value can be wrong in two qualitatively different ways, and the distinction is worth making first-class because consumers act on the two differently (reject malformed input at the wire; quarantine well-formed-but-invalid input for review). Each tier is a static `Effect` (what a site *may* do) paired with a runtime result datum (what actually went wrong).

- **Conformance -- the syntax tier.** The `Value` tree does not match the declared `ValueType` tree: wrong variant, wrong struct arity, a null in a non-nullable slot, an enum ordinal out of range. Structural, needs nothing but the type. Static effect: `Effect::MayNotConform`. Checked by `validation::conformance`.
- **Validation rules -- the business-rule tier.** The value conforms, but a predicate attached to the entity type rejects it. Static effect: `Effect::MayViolateRule`. Checked by `validation::validate`.

These are deliberately *not* folded into the existing `Effect::MayError(ValueType)`. `MayError` is parameterized by a user-defined error *type* -- it models an expression that can produce an error value of some result type. Conformance and rule failures are a fixed, system-level category, not a user error value, so they get their own markers. The effects stay bare markers (the effect *set* is meant to be compact); the payload lives on the runtime datum, see below.

## The effect-result datum and the navigation path

When a failure effect fires, the runtime side is a `Violation`: the dual of the static effect, carrying where it failed, which rule, and a message. Two constructors mint them, one per tier, mapping back to the effect via `Violation::effect` / `Tier::effect`:

- `Violation::not_conform(path, message)` -> `Tier::Conformance` -> `Effect::MayNotConform`
- `Violation::rule(path, rule_name, message)` -> `Tier::Rule` -> `Effect::MayViolateRule`

The "where" is a first-class `Path` -- the project had no navigation-path type before this, so one is introduced here and is reusable anywhere a spot in a `Value`/`ValueType` tree must be named (accessor IR, decode diagnostics, future quantified rules). A `Path` is a sequence of `PathSeg`:

- `PathSeg::Field(name)` -- a struct field by declared name.
- `PathSeg::Index(i)` -- an array element or map entry by iteration position.

It renders the obvious way: `addr.lines[0]`. The conformance walker threads a `Path` as it descends, so a violation deep in a nested struct/array/map points exactly at the offending leaf. `Path` is on the proto wire too (`Path` / `PathSegment`), so it can key a stored rule and travel in a serialized failure report.

## Representation: hybrid (closed vocabulary + expression escape hatch)

A pure expression language would be maximally expressive but drags the whole LogExpr evaluator (and its coeffects) into every schema check; a pure closed vocabulary is cheap and portable but cannot say `start < end`. So the representation is hybrid, the protovalidate/buf model:

- A closed `Constraint` vocabulary for the common per-field cases: `Min`/`Max` (ordered bounds), `MinLen`/`MaxLen`/`NonEmpty` (length over string/blob/array/map), `Pattern` (regex over strings), `OneOf` (membership). These are serializable, portable, and evaluated directly in `meta-types`. The bound variants carry a `Bound { value: Value, inclusive: bool }`: a full `Value` (not an `f64`) so they apply to any ordered type -- integers, floats, decimals, dates, timestamps, strings, byte strings -- and compare exactly (integer widths interoperate through `i128`, never through `f64`, so large values are never conflated); and an explicit inclusive/exclusive flag, which matters for floats and timestamps where a strict "before"/"after" bound is the natural constraint. `OneOf` likewise compares exactly with integer-width interoperation.
- An `Expr(String)` escape hatch carrying CEL/LogExpr source for everything else (cross-field, conditional). `meta-types` cannot depend on the codegen crate, so it does not interpret expressions -- `validate` collects them into `Outcome::deferred`, each resolved to the `Path` scope it applies to, for the caller to run with its own evaluator. A false result there becomes a `Tier::Rule` violation.

Rules attach at two granularities, both serialized into the type-declaration payload:

- Field-local, keyed by `Path` (`age >= 0`): `EntityValidation::fields`.
- Whole-entity, for cross-field predicates (`start < end`), typically `Expr`: `EntityValidation::entity`.

## Where it lives, and what stays untouched

Validation is a `meta-types` concern enforced by the consumer at ingest. It is emphatically *not* a control-plane-store concern: the persistence algebra's load-bearing invariant is that the entity payload is opaque bytes plus a type tag, and "the moment the persistence layer needs to understand what an entity means, the abstraction has leaked." So:

- `EntityValidation` serializes *inside* the type-declaration content -- which, from the store's view, is just opaque payload. The existing versioning, revision-pinning, and epoch machinery therefore carries the rules with **zero** changes to `rust-store`. A given instance revision pins a type revision (per the persistence algebra), and that type revision's payload carries exactly the rules in force when the instance was written; a later rule change never retroactively invalidates an existing revision.
- Rules are pure structural overlay on top of `ValueType`, not embedded in it. Two types that differ only in their validation rules are still the same structural `ValueType`. This keeps `ValueType` the single source of truth for type identity (the proto's stated principle) and validation a separable facet.

## Use-site narrowing: a reference can be stricter than its declaration

A named type declaration carries its own baseline rules -- they are the contract of that type *everywhere* it is used. But a particular use site often has extra, spicier opinions about which subset of that type's values it will accept: `Address` in general permits any country, but the shipping-address field here accepts only domestic ones; `Quantity` is any non-negative decimal, but this line item caps it at the pallet size. So a `ValueTypeRef` -- the use site of a declaration -- carries its *own* `EntityValidation` (proto `ValueTypeRef.validation`), on top of whatever the declaration already enforces.

The composition rule is the whole point, and it is deliberately one-directional: **use-site predicates compose with the declaration's by conjunction, so a reference can only narrow the accepted set, never widen it.** `EntityValidation::and` is exactly this -- because `validate` already fails on any violation, conjunction is just the union of both rule sets, and adding rules is monotone: it can only shrink what passes. A use site can demand a stricter subset; it has no way to re-admit a value the declaration rejected. That monotonicity is what makes carrying predicates on a reference safe -- a reference cannot quietly loosen a shared type's contract out from under everyone else who depends on it.

When the referenced type is embedded inside a larger type (its values live at some nested path, not the entity root), its rules have to move with it. `EntityValidation::prefixed(&path)` rebases a referenced type's rules to the location of use: field rules get the path prepended, and the referenced type's whole-entity (cross-field) rules become field rules at that path, since the referenced type's root now lives there. So resolving a struct whose `owner` field references `Person` folds `Person`'s `age >= 0` into `owner.age >= 0` and `Person`'s cross-field `start < end` into a rule scoped at `owner`.

The YAML surface for this is the natural extension of the `{ ref: Name }` form -- `{ ref: Name, where: [ ... ] }`, where the `where` clause is a list of use-site constraints -- and resolving such a reference yields the declaration's `ValueType` together with `declaration_rules.and(use_site_rules.prefixed(here))`. Wiring that through `value_type_yaml` (a constraint sub-parser, and a resolution path that returns type-plus-validation rather than bare `ValueType`, rebasing nested-reference rules as it descends) is the next implementation step; the composition primitives it needs (`and`, `prefixed`) are in place and tested.

## Ingest flow (intended consumer wiring)

1. Decode the incoming payload to a `Value` against the type revision's `ValueType`.
2. `conformance(&value, &vt)` -> any `Tier::Conformance` violations reject at the wire (`MayNotConform`).
3. `validate(&value, &vt, &rules)` -> `Tier::Rule` violations, plus `Outcome::deferred` expression predicates.
4. Evaluate each `DeferredExpr` with the consumer's LogExpr evaluator against the same `Value`; false -> a `Tier::Rule` violation on its path.
5. A clean run (no violations across both tiers) admits the write; otherwise return the collected `Violation`s.

## Open questions / deferred

- **Decimal bound scale.** `Min`/`Max` now carry a `Value` and compare exactly (integers through `i128`, strings lexicographically, floats to floats, etc.), with an explicit inclusive/exclusive flag. The one rough edge left is `Decimal`: it compares by raw mantissa, which is correct only when the bound is authored at the field's scale. Carrying the scale alongside the bound (or normalizing at schema-compile time) would close that.
- **Quantified rules.** "every element of `tags` is non-empty" needs a per-element quantifier the `Path`/`Constraint` model does not yet have. `Path` already distinguishes `Index`, so a `ForEach` wrapper is a natural addition.
- **Expr type-checking at schema-compile time.** The escape hatch is currently opaque source; ideally a consumer type-checks it against the entity `ValueType` (and confirms it is a pure boolean) when the type declaration is written, not at every ingest.
- **Effect propagation.** `MayNotConform` / `MayViolateRule` are defined and have runtime counterparts, but nothing in the catalog yet *produces* them on an expression site. Wiring a validation-bearing IR node's effect set is future work, gated the same way as the other effects. This is part of the larger move described next.

## Aside: a YAML surface syntax for ValueType

Entity types, struct fields, error types, and the like all need to be *written* somewhere, and the only YAML-to-`ValueType` path that existed (`udf_catalog::parse_simple_type`) was flat-scalar-only. `meta-types` now carries a single recursive codec, `value_type_yaml` (behind the `yaml-catalog` feature), where a type is either a string -- a scalar keyword (`string`, `i64`, `timestamp`) or the name of a declared type -- or a one-key mapping naming a constructor (`{ array: { element: ... } }`, `{ struct: [ ... ] }`, `{ decimal: { precision, scale } }`, ...). Two ways to name a type, as asked: **inline** (a structural literal) or **by name** against a `TypeCatalog` (a `types:` mapping whose declarations may reference each other; cycles are rejected, since a `ValueType` is a finite tree). `parse_type` / `TypeCatalog::from_yaml` load it; `to_yaml` emits it inline (names are not recovered -- a `ValueType` does not remember which declaration it came from).

This is the surface for the rest of the design: an entity type's `ValueType` and its `EntityValidation` are written in this syntax, a named `EntityRef`/`type_ref` target resolves through the catalog, and a typed `MayError` error type can be named rather than spelled out inline. Folding `udf_catalog`'s shallow parse onto this codec is the obvious follow-up.

## Aside: catalog-declared effect/coeffect kinds

Adding `MayNotConform` / `MayViolateRule` exposed a seam worth naming, because it generalizes past validation. Effect and coeffect *kinds* are currently sourced from three hand-maintained places that must agree: the `Effect` / `Coeffect` enums in `meta-types`, a hardcoded string-to-constructor match in `codegen.rs`, and the per-entry `coeffects: [...]` uses in the function catalog. The catalog records *uses* but never declares the *kinds*, so the kind set is de-facto owned by the runtime enum and duplicated in codegen.

The kinds belong in the catalog -- the same single-source-of-truth argument that puts the IR node set there. They are just **string tags**; codegen validates that every per-entry use names a declared kind when it generates from the catalog. The reason a bare string is the right representation, and not a richer typed declaration, is that **a kind's parameter space is open and not knowable up-front**. You can name the kinds; you cannot enumerate their inhabitants:

- Nullary kinds -- `may_null`, `may_partial`, `may_block`, `may_not_conform`, `may_violate_rule`, `reads_event_data`, `reads_aggregates`, `reads_enrichment` -- have no parameter; the tag is the whole story.
- Open-parameterized kinds -- `may_error` (a `ValueType`), `reads_current_time` (a `TimeGranularity`), `calls_external_udf` (a `UdfLanguage`) -- carry a value that only a concrete site knows. The catalog does not try to describe that parameter; the value is supplied where the effect is realized (per expression, per config). The string tag is all the catalog declares.

So the declaration is just a list of kind strings, e.g.:

```yaml
effect_kinds: [may_null, may_error, may_partial, may_block, may_not_conform, may_violate_rule]
coeffect_kinds: [reads_event_data, reads_current_time, reads_aggregates, reads_enrichment, calls_external_udf]
```

Wiring is deferred. When codegen grows to consume these, it validates the per-entry `coeffects: [...]` (and a future `effects: [...]`) against the declared kind strings, replacing the hardcoded string match in `codegen.rs`. Where these lists physically live -- a top-level section of `function-catalog.yaml` (today a bare list) versus a sibling kinds catalog fed alongside it -- is a catalog-format call to make at that point, not now.
