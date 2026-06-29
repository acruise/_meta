# Control-plane persistence algebra

**Status:** Draft / design musing, 2026-06-28. Not yet built. Captured here because it is a domain-agnostic building block intended to be shared across the cluonflux projects (the declarative actor platform was the first consumer to need it), the same way the value type system and function catalog are shared.

## What this is

A small, reusable algebra for storing a control plane's *domain model*: versioned entities that reference each other, snapshotted into consistent epochs, with structural sharing of the heavyweight transitive closures and cheap identity for the references between them. It deliberately knows nothing about what an entity *is* — the entity payload is opaque to the algebra (bytes plus a type tag). Consumers plug in their own domain types; the algebra owns versioning, referencing, epochs, and closure materialization. The moment the persistence layer needs to understand what an entity means, the abstraction has leaked and stopped being reusable, so keeping the payload opaque is load-bearing.

## Core model

- **Entity** — a thing with a stable identity, independent of any particular revision.
- **Version** — an immutable, append-only revision of an entity's content. A write is always the *insert of a new (entityId, versionId)*, never a mutation of an existing row. Identified cheaply by the `(ID, versionId)` tuple.
- **Edge** — a reference from one version to another entity, either *pinned* (names a concrete `versionId`) or *floating* (means "latest", resolved against an epoch). Edges are the cheap currency you pass around freely.
- **Epoch** — a content-addressed, append-only, *fully-pinned* snapshot of the selected version of every entity. An epoch resolves all floating references to concrete versions at the instant it is cut, so everything reachable from an epoch is mutually consistent and immutable.
- **Closure** — the materialized transitive closure of everything a given entity revision needs in order to run/operate. Heavyweight to build, so it admits structural sharing: two parents referencing the same sub-closure share the identical materialized node.

## Identity layers

Three representations of "which thing", each at the layer where it pays:

- `(ID, versionId)` tuples — the cheap, human-meaningful edges in the dependency graph. Small, copyable, what you store and pass.
- Content hash (Merkle) — for cross-process, cross-restart, and dedup identity, where pointer identity is unavailable or untrustworthy. Lets nodes diff/sync epochs by root hash (ship only the changed subtrees), dedup identical artifacts cluster-wide, survive restarts, and use a coordination-free self-verifying id (no central allocator handing out dense integers).
- Interned local index (e.g. `u32`) — resolve a hash or tuple to a dense index once at ingress, then carry the integer on any hot path. Hashes are large (32 bytes); you do not want them riding every in-flight item.

Content addressing is *usually not worth it* — within one process, plain `Arc` pointer-sharing already gives structural sharing plus O(1) subtree equality. It earns its keep here precisely because a control plane is distributed, persisted, deduplicated, and needs a coordination-free revision id — the rare quadrant where the hashing pays. The Merkle tree *is* the persistent config DAG you already wanted, annotated with hashes, so you keep pointer-sharing for in-process speed and gain hashes for cross-boundary identity; and the rehash cost piggybacks on the copy-on-write path you are already walking.

## Facets keep the closure shallow and acyclic

Decompose an entity into independently-versioned **facets** rather than versioning the whole entity as one blob. (In the actor platform's case: an actor type's event-schema, state-schema, and handler-table are separate facets.) This matters for two reasons:

- **Shallow closures.** To reference another entity, you usually need only one facet of it, not all of it. Versioning and sharing at facet granularity tightens each closure (import the one facet you need) and maximizes sharing (every parent that references that facet shares the identical node). The genuinely transitive part is then internal to a facet (e.g. a schema's nested-type imports), which is already a self-contained, content-shareable unit.
- **Acyclicity.** The domain reference graph between entities can be cyclic (mutual references are normal). Content addressing requires a DAG. Facet decomposition breaks the cycle: the cross-entity edge points at a *data* facet (a schema), which is a sink with no outgoing operational edges. So the closure DAG over facets stays acyclic even when the entity graph is not. Facets are not only a sharing-granularity choice; they are what makes the content-addressed scheme well-founded.

## Consistency model

The defining property: **writes need no consistency constraint other than referential integrity.** Because versions are immutable and append-only, a write is an insert with no write-write conflict to serialize. The single invariant is "a reference must point at something that exists" — exactly a foreign key. This is a monotone, coordination-free system in the CALM sense: concurrent writers creating disjoint version sets never coordinate, and the one non-monotone bit ("a ref requires its referent to exist first") is what the FK orders for you. Referential integrity does double duty: it gates creates (no dangling new reference) and gates deletes/GC (cannot reclaim a version while anything live references it).

Reads of a transitive closure should be sequentially consistent (or stronger), but under the model this is nearly free rather than expensive:

- The immutable, FK-closed content cannot tear under *any* isolation level — you cannot observe a half-written version (it is a single atomically-committed row) and cannot follow a dangling edge (FK guarantees the referent committed first).
- The only thing that genuinely needs a consistent snapshot is the mutable "latest" selection — i.e. resolving the epoch. Model epochs as append-only pinned rows, and a closure read collapses to: read one epoch row (a single-object read, trivially sequentially consistent) then traverse immutable content. The only truly mutable cell left is a tiny "current epoch head" pointer, which you can also model as `max(epochId)` over the append-only epoch log.

**Staleness is not inconsistency.** A stale `latest` read is just an *older valid epoch* — fully pinned, internally coherent, a snapshot that genuinely existed. So the two knobs decouple: epoch-pinning buys coherence for free, and recency dials independently. Carry "transitive-closure reads are sequentially consistent (or stronger)" as the conservative default since it costs almost nothing, and relax only if shown to hurt. Things to watch:

- Resolve `latest` at *epoch* granularity, never per-entity. Per-entity `max(versionId)` resolved independently can straddle a multi-entity publish and tear; "latest" must mean "latest epoch".
- Monotonic reads per consumer — never observe an older epoch than one you have already acted on (no config time-travel backward).
- The one place to *strengthen* to linearizable is control-plane read-your-writes immediately after a publish; the data plane tolerates stale-but-monotone-and-coherent epochs.

## Postgres encoding

Versions are immutable append-only rows; references are foreign keys; epochs are append-only pinned snapshots. The pinned/floating distinction is encoded natively with a **nullable column in a composite foreign key**: a non-null `versionId` is pinned (FK enforced), a null `versionId` is floating ("latest").

```sql
create table entities (id text primary key);

create table versions (
  entity_id   text   not null references entities(id),
  version_id  bigint not null,
  content     bytea  not null,            -- opaque payload; algebra does not interpret it
  primary key (entity_id, version_id)
);

create table edges (
  from_id  text   not null,
  from_ver bigint not null,
  to_id    text   not null references entities(id),   -- (1) entity must ALWAYS exist
  to_ver   bigint,                                     -- null = latest, set = pinned
  foreign key (from_id, from_ver) references versions(entity_id, version_id),
  foreign key (to_id,   to_ver)   references versions(entity_id, version_id) match simple  -- (2)
);
```

The mechanism is `MATCH SIMPLE` (Postgres's default): if any column of a composite FK is null, the whole composite check is skipped. So a null `to_ver` is structurally exempt from referential integrity — correct, because "latest" names no row to point at — while a set `to_ver` enforces the FK against a concrete immutable version. `MATCH FULL` rejects mixed null/non-null rows (so it cannot express this), and `MATCH PARTIAL` — which would enforce just the non-null subset, the ideal here — is unimplemented in Postgres.

The one gotcha: because `MATCH SIMPLE` disables the *entire* composite check on a null, a floating edge otherwise gets *zero* referential integrity, including no guarantee the entity even exists. So keep a separate, always-enforced plain FK `to_id -> entities(id)` (marker (1) above). Net invariants: every edge's target entity exists; revision pinning is guaranteed exactly when pinned. This pair is the standard workaround for the absence of `MATCH PARTIAL`.

A null `to_ver` encodes only *that the edge is floating* — it does not resolve it. Resolution still goes through the epoch: at materialization, substitute null with the epoch's chosen version, yielding a fully-pinned closure. Keep that discipline (null means "ask the epoch", not "ask `max()`") or the torn-closure hazard returns.

## Reusability boundary

- `_meta` owns the algebra: entity/version/edge/epoch model, the Postgres schema, closure materialization and structural sharing, the Merkle identity layer. Generic over an opaque entity payload (bytes plus a type tag) and the reference relation.
- A consumer owns its domain content and how it uses materialized closures (e.g. the actor platform binds an active actor to its materialized closure, interns the closure hash to a hot-path index, and decodes payloads against the closure's descriptors only at dispatch). The wire/storage identity — content hashes and `(ID, versionId)` tuples — is the contract between the two.

## Entity types are versioned the same way

Entity types are themselves versioned, append-only entities, modeled with the
*identical* mechanism as instances but kept in their own parallel tables
(`entity_types` / `entity_type_versions` / `entity_type_edges` alongside
`entities` / `entity_versions` / `entity_edges`) rather than one discriminated
set. Typed foreign keys
throughout, no discriminator column and no generated-column FK tricks; the cost
is a separate edge table for type -> type references (e.g. a schema importing
another schema), which is acceptable.

The instance -> type pointer is split across the two identity layers, and this
split is the load-bearing decision:

- The instance **header** points to the type **ID** (`entities.type_id`). This
  binds the stable type identity, independent of any type revision -- "this
  instance is, in general, of type X".
- The instance **revision** points to the type **revision** that was current (or
  explicitly selected) when the revision was written (`versions.type_version_id`,
  pinned). Pinning at the revision means a later type revision never retroactively
  re-types an existing instance revision.

Two foreign keys keep these honest: `(type_id, type_version_id) -> type_versions`
(the pinned type revision exists) and `(entity_id, type_id) -> entities(id,
type_id)` (the revision's type matches the header's declared type, so the two
layers cannot drift). The second is defense-in-depth against raw writers: the
store API derives `type_id` from the header and only takes a version *number*, so
it structurally cannot express a cross-type pin in the first place.

## One store per project, no scope column

There is deliberately no scope/tenant column. A store *is* a project's
namespace. Two unrelated projects must not point at the same tables. Share a
database between projects only when they must coordinate mutations on each
other's domain model -- and even then, prefer a client SDK in front of a single
owner over two writers on a shared schema. This keeps every key at its natural
columns and avoids threading a tag through every PK and FK.

## Open questions

- Epoch GC: reclaiming versions no longer referenced by any live epoch, coordinated with the FK delete-gate and with content-addressed dedup (a version may be unreferenced in one epoch but identical-by-hash to one still live).
- Whether the epoch head is a single mutable row or strictly `max(epochId)` over the log, and the read-your-writes story for control-plane operators across replicas.
- Content-addressing / Merkle identity, structural-shared closure materialization, and the interned local-index hot path are all deferred; the first cut (the `meta-control-plane` crate) implements the core layer -- versioned types/instances, edges, epochs, and closure read by traversal -- only.
