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
├── rust/                   # meta-codegen crate: the build-time IR generator
└── rust-store/             # meta-control-plane crate: Postgres control-plane store
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

### `meta-control-plane` (`rust-store/`)

The Postgres implementation of the [control-plane persistence algebra](docs/control-plane-persistence-algebra.md): versioned, append-only **entity types** and **entity instances** that reference each other, snapshotted into consistent **epochs**, with closures materialized by traversal. The payload of every type and instance is opaque to this layer (`bytea` plus a consumer-defined `type_tag`); the algebra owns versioning, referencing, and epochs, never interpretation.

- `migrations/0001_init.sql` — the schema. Types and instances are versioned by the identical mechanism but kept in separate parallel tables (`entity_types`/`entity_type_versions`/`entity_type_edges` and `entities`/`entity_versions`/`entity_edges`). The instance->type pointer is split: the **header** binds a type **ID**, each **revision** pins the type **revision** current/selected when it was written. Pinned vs floating edges are encoded with a nullable composite FK under `MATCH SIMPLE`.
- `src/store.rs` — `ControlPlaneStore`, built on `tokio-postgres` + `deadpool` (async, no build-time database). `put`/`get`/`cut_epoch`/`read_closure`, plus `migrate()` which runs the idempotent DDL.
- `src/model.rs` — newtyped identifiers (`EntityId` vs `TypeId` vs `VersionId` are not interchangeable) and the `Closure` result.

**One store per project.** There is no scope/tenant column; a store *is* a project's namespace. Do not point two unrelated projects at the same tables. Share a database between projects only when they must coordinate mutations on each other's domain model, and even then prefer a client SDK in front of a single owner over two writers on a shared schema.

This first cut is the core layer only — content-addressing/Merkle identity, structural-shared closure sharing, the interned hot-path index, and epoch GC are deferred (see the design doc's open questions).

## Building

Each crate is a standalone Cargo package (there is no workspace manifest at the repo root), so build from inside its own directory:

```sh
( cd rust-types && cargo build )
( cd rust && cargo build && cargo test )   # cargo test includes the codegen snapshot test
( cd rust-store && cargo build )           # integration tests need a throwaway Postgres:
                                           # TEST_DATABASE_URL=postgres://localhost/cp_test cargo test
```

Codegen runs automatically as part of the `rust` crate's `build.rs` (it rerun-triggers on changes to `function-catalog.yaml` and `codegen.rs`). To see the generated IR for a catalog directly:

```sh
( cd rust && cargo run -q -- ../function-catalog.yaml )
```

This prints the generated Rust to stdout and a summary of generated counts to stderr. Consumers feed their own additional catalogs alongside this one — for example, a consumer regenerates its physical IR by passing this catalog plus its own extension catalog to its codegen binary:

```sh
cargo run -q -p <consumer>-codegen -- _meta/function-catalog.yaml meta/<consumer>-catalog.yaml > ir/src/_gen_expr.rs
```

## Consuming `_meta`, and co-developing it from a downstream project

A consumer may pull `_meta` in as a git **dependency** rather than a submodule (the actor platform does this: `meta-types = { git = "https://github.com/acruise/_meta.git" }` in its `Cargo.toml`, with `Cargo.lock` pinning the rev). This decouples the consumer's git history from a pinned `_meta` commit — picking up a change is `cargo update -p meta-types`, not a submodule pointer bump.

To hack on `_meta` and a consumer together in one edit-build loop without re-submoduling and without disturbing the consumer's committed (reproducible) build, use a Cargo **path override** in the consumer. Check `_meta` out as a sibling of the consumer, then add a local-only `.cargo/config.toml` in the consumer:

```toml
# consumer/.cargo/config.toml  (gitignore this file)
paths = ["../_meta/rust-types"]
```

This redirects the `meta-types` dependency to your local `_meta` working tree regardless of how it is declared, so edits flow through immediately. Because the file is gitignored, the consumer's committed `Cargo.toml`/`Cargo.lock` still reference the pinned git rev, so anyone else (and CI) gets the reproducible pinned version. Verify it took with `cargo metadata` (the `meta-types` id should read `path+file://.../_meta/rust-types`).

When done: commit and push here, then in the consumer run `cargo update -p meta-types` to bump the pinned rev to the new commit; delete the override to build against the published rev. Caveat: a `paths` override requires the local crate to keep the same name and a compatible version and cannot add/remove the crate's own dependencies while active — if you restructure `meta-types`' dependency graph and Cargo complains, switch to a committed `[patch."https://github.com/acruise/_meta.git"]` entry in the consumer's root `Cargo.toml` (more robust, but committed, so it requires the sibling checkout to exist for every build).

## Design notes

- [`docs/control-plane-persistence-algebra.md`](docs/control-plane-persistence-algebra.md) — draft design for a domain-agnostic control-plane persistence algebra (versioned append-only entities, referential-integrity as the sole write invariant, pinned/floating edges via nullable composite FK, content-addressed immutable epochs, structural-shared transitive closures). Intended as a shared building block, first needed by the actor platform.
- [`docs/entity-validation-predicates.md`](docs/entity-validation-predicates.md) — finer-grained validation attached to entity types beyond `Value`/`ValueType` conformance: a two-tier model (structural conformance vs. business-rule predicates) with a matching pair of effects, a hybrid closed-vocabulary + CEL-expression representation, and a first-class navigation `Path`. First cut implemented in `meta-types` (`rust-types/src/validation.rs`).
- [`docs/codegen-content-hashing.md`](docs/codegen-content-hashing.md) — the paranoid `EXPR_GEN_HASH` guard for multi-layered codegen: why the generated IR carries a content hash of the catalog *and* the generator's own source, how the compile-time assert in `type_check.rs` forces a human re-read on semantic-only catalog drift, the three-guard split (compiler exhaustiveness / snapshot / hash assert), the per-file convention, and how a consumer layer should fold the substrate hash into its own.

## Working in this repo

Because `_meta` is a submodule, a change here is a change to a contract two projects depend on. A few consequences:

- Editing the catalogs regenerates the IR. `EXPR_GEN_HASH` is a content hash of the whole catalog plus the codegen, not of the emitted enum shape — so it also changes for catalog edits that leave `LogExpr` byte-identical (e.g. tweaking `null_semantics`, `return`/`params` types, `coeffects`). When it changes, the compile-time assertion in `type_check.rs` fails on purpose, forcing a human to re-read that pass against the catalog diff before re-pinning the constant. Treat a re-pin as "I reviewed it," not a reflex.
- After committing here, bump the submodule pointer in each consuming project to pick up the change.
- House style (shared across the consuming projects): Markdown with soft wraps (one physical line per paragraph), ASCII only in source comments and notes — no emoji, no unicode box-drawing. Propose design before large changes.

### TODO

- The `EXPR_GEN_HASH` guard in `type_check.rs` currently carries two jobs: catching *structural* catalog changes (variant added/removed/re-fielded) and *semantic-only* changes (shape-identical, metadata moved). The structural half is redundant for any pass that matches `LogExpr` exhaustively — `udf_resolver.rs` already relies on the compiler's non-exhaustive-match error instead. Consider dropping the 8 wildcard arms in `type_check.rs` to make its match exhaustive too; that would offload structural detection to the compiler and leave the hash guard responsible only for the residual semantic-only class, making each "do I actually need to review?" decision sharper when it fires. The guard is per-file by convention — if another pass ever grows semantic coupling to catalog metadata, it needs its own assert; nothing extends type_check's coverage automatically.
