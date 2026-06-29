# The codegen content-hashing guard

**Status:** Documentation of an implemented mechanism, 2026-06-29. The single-layer guard described here lives in `rust/src/codegen.rs` and `rust/src/type_check.rs`; the multi-layer composition in the last section is the discipline a consumer's own codegen should follow, stated here because nothing in `_meta` can enforce it across a repo boundary.

## The problem it solves

`_meta` generates Rust from the catalogs, and then hand-written passes operate on that generated code. The generator emits the `LogExpr` enum from `function-catalog.yaml`; `type_check.rs`, `normalizer.rs`, `udf_resolver.rs`, and consumers' own passes are written by hand against that enum and against catalog *metadata* (null semantics, coeffects, param/return types, the algebraic flags). Two failure modes follow, and ordinary tooling catches neither well:

- **Structural drift.** A catalog edit adds, removes, or re-fields a `LogExpr` variant. A pass that matches the enum non-exhaustively keeps compiling and silently mishandles the new shape.
- **Semantic-only drift.** A catalog edit leaves the emitted enum *byte-identical* but changes metadata a hand-written pass depends on -- e.g. flipping a `null_semantics` from `propagate` to `consume`, or changing a `return` type. The generated file does not change, every test that pins the generated text still passes, and a pass whose correctness silently depended on the old metadata is now wrong with no signal at all.

The hash guard exists for the second mode especially: it makes an invisible change visible by forcing a human to re-read the coupled pass. It is deliberately paranoid -- it would rather fire on a change that turns out to be harmless than stay silent on one that is not.

## The hash chain

Two *volatile inputs* are hashed (`content_hash`, a `DefaultHasher` over a string):

- `catalog_hash = content_hash(function-catalog.yaml)` -- the data.
- `codegen_hash = content_hash(src/codegen.rs)` -- the generator's *own source*, pulled in via `include_str!`. Hashing the generator itself means a change to how code is emitted (not just what data feeds it) also trips the guard: a refactor of the emission logic is as reviewable as a catalog edit.

These combine into one constant (`combined_hash` hashes the two hashes together):

```
EXPR_GEN_HASH = combined_hash(catalog_hash, codegen_hash)
```

The generator writes all three into the generated file (`rust/src/codegen.rs:202`):

```rust
// CATALOG_HASH: <hex>
// CODEGEN_HASH: <hex>
pub const EXPR_GEN_HASH: u64 = 0x....;
```

`EXPR_GEN_HASH` is the load-bearing one; the two component comments are there so that when the combined value moves you can see *which* input moved (catalog vs generator).

## The three guards, and what each is responsible for

The hash is one of three layers, each catching a different class, and the design intent is that they stay separate rather than one trying to do every job.

1. **Compiler exhaustiveness -- structural drift, for free.** A pass that matches `LogExpr` *exhaustively* (no wildcard arm) fails to compile the moment a variant is added or re-fielded. `udf_resolver.rs` already relies on this and needs no hash assert for the structural class. This is the cheapest guard and the preferred one for structural changes.

2. **The snapshot test -- the exact emitted shape.** `rust/tests/codegen_snapshot.rs` runs the codegen binary and diffs its output against `expected_codegen_output.txt`. Crucially it *strips the three hash lines first* (`strip_hash_lines`), so the snapshot pins the structural text and the hash guard pins the semantics, independently -- a pure metadata edit moves `EXPR_GEN_HASH` but leaves the stripped snapshot identical, and a shape change moves the snapshot. Update this file when a shape change is intentional.

3. **The `EXPR_GEN_HASH` compile-time assert -- semantic-only drift.** `type_check.rs:25` carries:

   ```rust
   const _: () = assert!(
       crate::expr_gen::EXPR_GEN_HASH == 0x4c5dfe8c3da1b6cd,
       "type_check.rs needs review — EXPR_GEN_HASH changed"
   );
   ```

   When either volatile input changes, the constant moves, this `const` assertion fails *at compile time*, and the build stops until a human re-pins it. The failure is the point: it forces a re-read of `type_check.rs` against the catalog/codegen diff before the pinned value is updated.

## Why it is "paranoid", and the re-pin discipline

The guard over-fires on purpose. Because it hashes whole-file content, it trips on changes that cannot possibly affect `type_check.rs` -- a comment in the catalog, whitespace in the generator, a `notes:` field. That is the accepted cost of never missing the changes that *can* affect it. The rule that makes this sustainable:

**Re-pinning the constant means "I reviewed this," not a reflex.** When the assert fires, read the diff to the catalog and `codegen.rs`, decide whether the hand-written pass is still correct under it, and only then update the pinned hex to the new `EXPR_GEN_HASH`. Treating the re-pin as a mechanical "make it compile" step throws away the entire value of the mechanism.

When it fires, the workflow is:

1. Look at the `CATALOG_HASH` / `CODEGEN_HASH` comments (or the diff) to see which input moved.
2. Re-read the guarded pass against that diff.
3. If the emitted shape changed, also refresh `expected_codegen_output.txt`.
4. Update the pinned constant to the new value.

## The per-file convention, and its non-composability

The assert is **per-file by convention**: `type_check.rs` carries one because it is semantically coupled to catalog metadata, and that assert speaks *only* for `type_check.rs`. Nothing extends its coverage automatically. If another pass grows a semantic dependency on catalog metadata, it must plant *its own* assert against `EXPR_GEN_HASH`; the existing one will not protect it. This is a known sharp edge -- coverage is opt-in per coupled file, and the discipline is to add an assert whenever you write a pass that reads catalog semantics the generated shape does not capture.

A related cleanup is open (see the README TODO): `type_check.rs` currently also keeps wildcard arms, so it leans on the hash for *structural* detection too. Making its match exhaustive would offload structural detection to the compiler (guard 1) and leave the hash assert responsible only for the residual semantic-only class -- sharpening every "do I actually need to review?" decision when it fires.

## Multi-layered codegen

`_meta` is the bottom layer: catalog plus generator produce `LogExpr` and the `EXPR_GEN_HASH` that covers them. Consumers add a layer on top -- a consumer feeds `_meta/function-catalog.yaml` *plus its own extension catalog* to its own codegen binary to generate a physical IR (e.g. a `PhysExpr` that adds `GetColumn` / `GetAggregate` nodes). That consumer has the same two failure modes against its own hand-written passes, over a larger input set.

The composition principle: **each layer should fold the layer below it into its own hash.** A consumer's guard constant should be a hash of its own volatile inputs *and* the `EXPR_GEN_HASH` it built against:

```
CONSUMER_GEN_HASH = hash(EXPR_GEN_HASH,
                         content_hash(consumer-catalog.yaml),
                         content_hash(consumer-codegen source))
```

So a change in the `_meta` substrate propagates upward: it moves `EXPR_GEN_HASH`, which moves `CONSUMER_GEN_HASH`, which trips the consumer's own per-file asserts and forces review of the consumer's passes against the substrate change -- exactly the same paranoia, one layer up. What is implemented in this repo is the bottom layer only (`combined_hash` of catalog + generator). The upward composition is a contract each consumer's codegen must honor in its own `generate`, because the chain is only as strong as the layer that forgets to include the one beneath it.

## Caveat: it is a tripwire, not a cross-machine identity

`content_hash` uses the standard-library `DefaultHasher`. It is deterministic within a toolchain, but `std` does not guarantee the algorithm is stable across Rust releases, and these values are not a Merkle/content-addressed identity (that is a separate concern; see the control-plane persistence algebra). Treat `EXPR_GEN_HASH` as a local change-detection tripwire whose pinned value belongs to a checkout, not as a portable fingerprint to compare across machines or versions. If it ever flaps purely from a toolchain bump, that is a recognized limitation of using `DefaultHasher` here, not a real catalog change -- re-pin and move on.
