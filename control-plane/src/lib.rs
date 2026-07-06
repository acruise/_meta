//! Postgres-backed control-plane store.
//!
//! A small, reusable persistence layer for a control plane's domain model:
//! versioned, append-only **entity types** and **entity instances** that
//! reference each other, snapshotted into consistent **epochs**. The payload of
//! every type and instance is opaque to this layer (`bytea` plus a consumer's
//! `type_tag`); the algebra owns versioning, referencing, and epochs, never
//! interpretation. See `docs/control-plane-persistence-algebra.md`.
//!
//! ## One store per project
//!
//! There is no scope/tenant column. A [`ControlPlaneStore`] *is* a single
//! project's namespace. Do not point two unrelated projects at the same tables.
//! Share a database between projects only when they must coordinate mutations on
//! each other's domain model -- and even then, prefer a client SDK in front of a
//! single owner over two writers on a shared schema.
//!
//! ## Type binding
//!
//! An instance's identity is bound to a type's identity at the **header**
//! ([`ControlPlaneStore::create_entity`]), while each instance **revision** pins
//! the specific type revision that was current/selected when it was written
//! ([`ControlPlaneStore::put_version`]). The schema enforces that a revision's
//! pinned type revision belongs to the same type the header declares.

pub mod error;
pub mod model;
mod store;

pub use error::{Result, StoreError};
pub use model::{
    Closure, EntityId, EntityRevision, EpochId, Ref, TypeId, TypeRevision, VersionId,
};
pub use store::ControlPlaneStore;
