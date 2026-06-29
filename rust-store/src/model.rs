//! Value types for the control-plane algebra.
//!
//! The identifiers are newtypes on purpose: the entire point of the type/instance
//! split is that an entity id and a type id are not interchangeable, and a version
//! id is meaningless without knowing which entity it belongs to. The newtypes make
//! "passed a type id where an entity id was wanted" a compile error rather than a
//! silent foreign-key failure at runtime.

use std::fmt;

/// Stable identity of an entity instance, independent of any revision.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntityId(pub String);

/// Stable identity of an entity type, independent of any revision.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeId(pub String);

/// An append-only revision number, scoped to a single entity or type. Monotonic
/// per parent, starting at 1; meaningless on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VersionId(pub i64);

/// A content-consistent snapshot identifier, monotonic across the whole store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EpochId(pub i64);

macro_rules! string_id {
    ($t:ty) => {
        impl $t {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl From<&str> for $t {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }
        impl From<String> for $t {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}
string_id!(EntityId);
string_id!(TypeId);

impl fmt::Display for VersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl fmt::Display for EpochId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An edge target: either pinned to a concrete revision, or floating ("latest",
/// resolved against an epoch at materialization time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ref {
    Pinned(VersionId),
    Floating,
}

impl Ref {
    /// The concrete `to_ver` column value: `Some` when pinned, `None` (SQL null)
    /// when floating. A null `to_ver` is what makes the composite FK MATCH SIMPLE
    /// skip enforcement, which is exactly "this names no row yet".
    pub fn to_ver(self) -> Option<i64> {
        match self {
            Ref::Pinned(v) => Some(v.0),
            Ref::Floating => None,
        }
    }

    pub fn from_opt(v: Option<i64>) -> Self {
        match v {
            Some(v) => Ref::Pinned(VersionId(v)),
            None => Ref::Floating,
        }
    }
}

/// A revision of an entity type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRevision {
    pub type_id: TypeId,
    pub version_id: VersionId,
    pub content: Vec<u8>,
    /// The consumer's tag for how `content` is encoded. Opaque to this layer.
    pub type_tag: String,
}

/// A revision of an entity instance, carrying its pinned type revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityRevision {
    pub entity_id: EntityId,
    pub version_id: VersionId,
    pub content: Vec<u8>,
    /// The type identity this revision's `type_version` belongs to (== the
    /// header's `type_id`).
    pub type_id: TypeId,
    /// The type revision current/selected when this instance revision was written.
    pub type_version_id: VersionId,
}

/// The materialized transitive closure of an entity revision at a given epoch:
/// every instance revision reachable through (pinned + epoch-resolved) edges,
/// plus every type revision those instances pin and every type revision reachable
/// from there through type edges. Everything here is concrete and immutable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Closure {
    pub epoch: Option<EpochId>,
    pub entities: Vec<EntityRevision>,
    pub types: Vec<TypeRevision>,
}
