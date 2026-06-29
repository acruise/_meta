-- Control-plane persistence: core layer.
--
-- Split type/instance tables (entity types and entity instances are versioned
-- by the identical mechanism, but kept in separate, parallel tables rather than
-- a single discriminated set). The payload (content) is opaque to this layer;
-- the algebra owns versioning, referencing, and epochs, never interpretation.
--
-- ONE STORE PER PROJECT. There is no scope/tenant column: a store IS a project's
-- namespace. Do not point two unrelated projects at the same tables. Share a
-- database only when projects must coordinate mutations on each other's domain
-- model, and even then prefer a client SDK over a shared schema.
--
-- The whole-file design note lives in docs/control-plane-persistence-algebra.md.

-- This DDL is run idempotently by ControlPlaneStore::migrate(); every object is
-- created IF NOT EXISTS so re-running against an initialized store is a no-op.

-- ---------------------------------------------------------------------------
-- Entity types (versioned, append-only)
-- ---------------------------------------------------------------------------

create table if not exists entity_types (
  id text primary key
);

create table if not exists entity_type_versions (
  type_id    text        not null references entity_types(id),
  version_id bigint      not null,
  content    bytea       not null,                 -- opaque type descriptor payload
  type_tag   text        not null,                 -- consumer's discriminator for the payload encoding
  created_at timestamptz not null default now(),
  primary key (type_id, version_id)
);

-- type -> type references (e.g. a schema importing another schema).
-- Pinned (to_ver set) or floating (to_ver null = "latest", resolved via an epoch).
create table if not exists entity_type_edges (
  from_id  text   not null,
  from_ver bigint not null,
  to_id    text   not null references entity_types(id),   -- (1) target type always exists
  to_ver   bigint,                                        -- null = floating, set = pinned
  foreign key (from_id, from_ver) references entity_type_versions(type_id, version_id),
  -- (2) MATCH SIMPLE: a null to_ver skips the whole composite check (correct: "latest"
  -- names no row). A set to_ver is enforced against a concrete immutable type version.
  foreign key (to_id, to_ver) references entity_type_versions(type_id, version_id) match simple
);

-- ---------------------------------------------------------------------------
-- Entity instances (versioned, append-only)
-- ---------------------------------------------------------------------------

create table if not exists entities (
  id      text not null primary key,
  -- Header -> type ID: the stable identity of the type this instance is an
  -- instance of. Deliberately NOT pinned to a type revision here; the header
  -- binds the type identity, the revision (below) pins the type revision.
  type_id text not null references entity_types(id),
  -- redundant key so a revision can prove its pinned type matches the header's type
  unique (id, type_id)
);

create table if not exists entity_versions (
  entity_id       text        not null,
  version_id      bigint      not null,
  content         bytea       not null,            -- opaque instance payload
  -- Revision -> type revision: the type revision that was current (or explicitly
  -- selected) at the instant this instance revision was written. Pinned, always.
  -- type_id is denormalized from the header purely to anchor the two FKs below.
  type_id         text        not null,
  type_version_id bigint      not null,
  created_at      timestamptz not null default now(),
  primary key (entity_id, version_id),
  foreign key (entity_id) references entities(id),
  -- the pinned type revision must exist...
  foreign key (type_id, type_version_id) references entity_type_versions(type_id, version_id),
  -- ...and it must be a revision of the SAME type the header declares (no drift)
  foreign key (entity_id, type_id) references entities(id, type_id)
);

-- instance -> instance references. Pinned or floating, same encoding as entity_type_edges.
create table if not exists entity_edges (
  from_id  text   not null,
  from_ver bigint not null,
  to_id    text   not null references entities(id),   -- (1) target entity always exists
  to_ver   bigint,                                    -- null = floating, set = pinned
  foreign key (from_id, from_ver) references entity_versions(entity_id, version_id),
  foreign key (to_id, to_ver) references entity_versions(entity_id, version_id) match simple  -- (2)
);

-- ---------------------------------------------------------------------------
-- Epochs: append-only, fully-pinned snapshots.
-- An epoch records the selected version of every entity AND every type at the
-- instant it was cut, so that floating edges resolve to concrete versions and
-- everything reachable from the epoch is mutually consistent and immutable.
-- ---------------------------------------------------------------------------

create table if not exists epochs (
  epoch_id   bigint      not null primary key,
  created_at timestamptz not null default now()
);

create table if not exists epoch_entity_selections (
  epoch_id   bigint not null references epochs(epoch_id),
  entity_id  text   not null,
  version_id bigint not null,
  primary key (epoch_id, entity_id),
  foreign key (entity_id, version_id) references entity_versions(entity_id, version_id)
);

create table if not exists epoch_entity_type_selections (
  epoch_id   bigint not null references epochs(epoch_id),
  type_id    text   not null,
  version_id bigint not null,
  primary key (epoch_id, type_id),
  foreign key (type_id, version_id) references entity_type_versions(type_id, version_id)
);

-- Index the edge traversal direction used by closure reads.
create index if not exists entity_edges_from_idx      on entity_edges (from_id, from_ver);
create index if not exists entity_type_edges_from_idx on entity_type_edges (from_id, from_ver);
