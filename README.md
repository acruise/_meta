# _meta

The shared substrate for the cluonflux projects. `_meta` is the single source of truth for the value type system, the function/operator catalog, the proto wire model, and the codegen that turns those catalogs into Rust IR. It is consumed as a git submodule (`github.com/acruise/_meta`) by several downstream projects (a real-time columnar event buffer, a declarative actor platform, and others), so that an expression written against one means exactly the same thing in another.

If you are looking for the "rosetta stone" that maps between our internal IR, CEL, and Substrait, you are in the right place.

## What lives here

```
_meta/
├── function-catalog.yaml   # 193 function/operator entries — the rosetta stone for expressions
├── type-catalog.yaml       # 31 value-type entries — the rosetta stone for types
├── proto/
│   ├── value.proto         # the cluonflux.meta wire model (ValueType, EncodedValue)
│   └── vendor/cel/expr/    # vendored CEL syntax.proto / checked.proto
├── rust-types/             # meta-types crate: hand-written runtime types
└── rust/                   # meta-codegen crate: the build-time IR generator
```

### The catalogs

`function-catalog.yaml` is the heart of the repo. Each entry has a unique `id` that lives in no external namespace, plus up to three mapping columns:

- `internal` — our IR node name. Drives codegen of the `LogExpr` enum variants.
- `cel` — the CEL function name (operator or method). How users actually write it.
- `substrait` — the Substrait extension + name. For interop reference.

Any column may be absent: an entry with no `internal` generates no IR node; an entry with no `cel` has no user-facing syntax yet; an entry with no `substrait` is ours alone. Entries also carry algebraic metadata (`commutative`, `short_circuits`, `aggregate`, `hof`, `lambda`, `coeffects`, `null_semantics`, parameter/return types) that the codegen and the consumers use to reason about expressions.

`type-catalog.yaml` does the same for value types, mapping our `ValueType` to CEL runtime types and Substrait types. A deliberate design choice across both catalogs: nullability is a property of the container (column, struct field, array element, map value), never of the type itself.

### `meta-types` (`rust-types/`)

Hand-written runtime types that both consumers depend on. `Value` never appears on the hot path — it is the cold-path / interpreted-evaluator carrier only.

- `value.rs` — the `Value` tagged union and `ValueType`, plus proto serde.
- `coeffects.rs` — `Coeffect` / `CoeffectSet`: what an expression *reads* (event data, current time, aggregates, enrichment, external UDFs). Drives memoization and evaluation-wave ordering.
- `effects.rs` — `Effect` / `EffectSet`: what an expression *does* (may null, may error of a given type, may be partial, may block). The dual of coeffects.
- `external_fn.rs` — the external UDF boundary: the `ProtoSerde` bridge (Rust types ↔ proto `EncodedValue`, with `Value` deliberately absent), `ExternalFn` traits, and the `UdfImport` / `ResolvedUdfRef` machinery for query-level UDF imports.
- `udf_catalog.rs` — YAML parser for external UDF module catalogs (behind the `yaml-catalog` feature).
- generated `cluonflux::meta` proto module (from `proto/value.proto`).

### `meta-codegen` (`rust/`)

The build-time generator. It reads `function-catalog.yaml` and emits the logical IR (`LogExpr`) enum plus its coeffect machinery, and provides the translation/checking passes that operate on that IR.

- `codegen.rs` — parses the catalog and emits `LogExpr`, intrinsic + transitive coeffect impls, and an `EXPR_GEN_HASH` guard so hand-written passes know when the generated shape changed. Runs from `build.rs`.
- `cel_to_ir.rs` — CEL source → `LogExpr` with *partial raising*: catalog-known constructs become physical IR nodes, unrecognized ones become `CelFallback` leaves the runtime delegates to CEL. Always succeeds if the CEL parses; the only variable is how much got raised.
- `type_check.rs` — type checking over the IR, including auto-cast modes and external UDF metadata.
- `normalizer.rs` — canonicalizes structurally equivalent trees (commutative operand sorting, double-negation and identity elimination) so downstream DAG lowering can dedup.
- `udf_resolver.rs` — resolves `UnresolvedCall` nodes against a query's UDF imports into `ExternalCall` nodes (erroring on no-match or ambiguity).
- `event_proto_codegen.rs` — generates schema-specific `.proto` messages and JSON→proto conversion for events at the ingest boundary.

The `LogExpr` enum is the genuinely shared artifact: every consumer generates it from the *same* catalog, then extends it with its own physical layer (e.g. a consumer's `PhysExpr` adds nodes like `GetColumn` / `GetAggregate` / `GetAccumulator`).

## Building

Each crate is a standalone Cargo package (there is no workspace manifest at the repo root), so build from inside its own directory:

```sh
( cd rust-types && cargo build )
( cd rust && cargo build && cargo test )   # cargo test includes the codegen snapshot test
```

Codegen runs automatically as part of the `rust` crate's `build.rs` (it rerun-triggers on changes to `function-catalog.yaml` and `codegen.rs`). To see the generated IR for a catalog directly:

```sh
( cd rust && cargo run -q -- ../function-catalog.yaml )
```

This prints the generated Rust to stdout and a summary of generated counts to stderr. Consumers feed their own additional catalogs alongside this one — for example, a consumer regenerates its physical IR by passing this catalog plus its own extension catalog to its codegen binary:

```sh
cargo run -q -p <consumer>-codegen -- _meta/function-catalog.yaml meta/<consumer>-catalog.yaml > ir/src/_gen_expr.rs
```

## Working in this repo

Because `_meta` is a submodule, a change here is a change to a contract two projects depend on. A few consequences:

- Editing the catalogs changes the generated IR shape. If `EXPR_GEN_HASH` changes, the hand-written passes (`type_check.rs` and the consumers' evaluators) need review — that is exactly what the compile-time hash assertion is there to force.
- After committing here, bump the submodule pointer in each consuming project to pick up the change.
- House style (shared across the consuming projects): Markdown with soft wraps (one physical line per paragraph), ASCII only in source comments and notes — no emoji, no unicode box-drawing. Propose design before large changes.
