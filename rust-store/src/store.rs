//! The Postgres-backed control-plane store.
//!
//! Every write is an append (an `INSERT` of a new immutable row); nothing here
//! mutates an existing version. The only write invariant is referential
//! integrity, which the schema enforces with foreign keys, so concurrent writers
//! creating disjoint version sets never need to coordinate.

use std::collections::{HashSet, VecDeque};

use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use tokio_postgres::types::ToSql;
use tokio_postgres::{GenericClient, NoTls};

use crate::error::{Result, StoreError};
use crate::model::{Closure, EntityId, EntityRevision, EpochId, Ref, TypeId, TypeRevision, VersionId};

/// The DDL run by [`ControlPlaneStore::migrate`]. Idempotent (`create ... if not
/// exists`), so re-running against an initialized store is a no-op.
const SCHEMA_SQL: &str = include_str!("../migrations/0001_init.sql");

/// A handle to one project's control plane. A store *is* a project's namespace;
/// do not point two unrelated projects at the same tables (see the module docs
/// and `docs/control-plane-persistence-algebra.md`).
#[derive(Clone)]
pub struct ControlPlaneStore {
    pool: Pool,
}

impl ControlPlaneStore {
    /// Build a store from a libpq/tokio-postgres connection string, managing its
    /// own connection pool. Uses `NoTls`; for TLS, build a [`Pool`] yourself and
    /// pass it to [`ControlPlaneStore::from_pool`].
    pub fn connect(conn_str: &str) -> Result<Self> {
        let pg_cfg: tokio_postgres::Config = conn_str.parse()?;
        let mgr = Manager::from_config(
            pg_cfg,
            NoTls,
            ManagerConfig {
                recycling_method: RecyclingMethod::Fast,
            },
        );
        let pool = Pool::builder(mgr)
            .build()
            .map_err(|e| StoreError::PoolBuild(e.to_string()))?;
        Ok(Self { pool })
    }

    /// Wrap a caller-supplied pool (e.g. one configured with TLS or custom sizing).
    pub fn from_pool(pool: Pool) -> Self {
        Self { pool }
    }

    /// The underlying pool, for callers that need to run their own statements.
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Create the schema if it does not already exist.
    pub async fn migrate(&self) -> Result<()> {
        let client = self.pool.get().await?;
        client.batch_execute(SCHEMA_SQL).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Entity types
    // -----------------------------------------------------------------------

    /// Register a type identity. Idempotent on `id`.
    pub async fn create_type(&self, id: &TypeId) -> Result<()> {
        let client = self.pool.get().await?;
        client
            .execute(
                "insert into entity_types (id) values ($1) on conflict (id) do nothing",
                &[&id.0],
            )
            .await?;
        Ok(())
    }

    /// Append a new type revision, allocating the next per-type version number.
    /// Errors if the type identity does not exist.
    pub async fn put_type_version(
        &self,
        id: &TypeId,
        content: &[u8],
        type_tag: &str,
    ) -> Result<VersionId> {
        let client = self.pool.get().await?;
        let row = client
            .query_opt(
                "insert into entity_type_versions (type_id, version_id, content, type_tag) \
                 select $1, \
                        coalesce((select max(version_id) from entity_type_versions where type_id = $1), 0) + 1, \
                        $2, $3 \
                 from entity_types where id = $1 \
                 returning version_id",
                &[&id.0, &content, &type_tag],
            )
            .await?;
        match row {
            Some(r) => Ok(VersionId(r.get(0))),
            None => Err(StoreError::TypeNotFound(id.0.clone())),
        }
    }

    /// Load a concrete type revision.
    pub async fn get_type_version(&self, id: &TypeId, ver: VersionId) -> Result<TypeRevision> {
        let client = self.pool.get().await?;
        load_type_revision(&**client, id, ver).await
    }

    /// The highest version number of a type, or `None` if it has no revisions.
    pub async fn latest_type_version(&self, id: &TypeId) -> Result<Option<VersionId>> {
        let client = self.pool.get().await?;
        let row = client
            .query_one(
                "select max(version_id) from entity_type_versions where type_id = $1",
                &[&id.0],
            )
            .await?;
        Ok(row.get::<_, Option<i64>>(0).map(VersionId))
    }

    /// Add a type -> type reference (pinned or floating).
    pub async fn add_type_edge(
        &self,
        from: (&TypeId, VersionId),
        to_id: &TypeId,
        to: Ref,
    ) -> Result<()> {
        let client = self.pool.get().await?;
        client
            .execute(
                "insert into entity_type_edges (from_id, from_ver, to_id, to_ver) values ($1, $2, $3, $4)",
                &[&from.0 .0, &from.1 .0, &to_id.0, &to.to_ver()],
            )
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Entity instances
    // -----------------------------------------------------------------------

    /// Register an instance identity bound to a type identity (the header ->
    /// type-ID pointer). Idempotent on `id`; errors if the type does not exist.
    pub async fn create_entity(&self, id: &EntityId, type_id: &TypeId) -> Result<()> {
        let client = self.pool.get().await?;
        client
            .execute(
                "insert into entities (id, type_id) values ($1, $2) on conflict (id) do nothing",
                &[&id.0, &type_id.0],
            )
            .await?;
        Ok(())
    }

    /// Append a new instance revision, pinning it to `type_version` -- the type
    /// revision current/selected at write time (the revision -> type-revision
    /// pointer). The owning type id is taken from the header, and the schema
    /// enforces that `type_version` is a revision of that same type.
    ///
    /// Errors if the entity does not exist; a `type_version` that does not exist
    /// or belongs to the wrong type surfaces as a foreign-key [`StoreError::Db`].
    pub async fn put_version(
        &self,
        entity_id: &EntityId,
        content: &[u8],
        type_version: VersionId,
    ) -> Result<VersionId> {
        let client = self.pool.get().await?;
        let row = client
            .query_opt(
                "insert into entity_versions (entity_id, version_id, content, type_id, type_version_id) \
                 select $1, \
                        coalesce((select max(version_id) from entity_versions where entity_id = $1), 0) + 1, \
                        $2, e.type_id, $3 \
                 from entities e where e.id = $1 \
                 returning version_id",
                &[&entity_id.0, &content, &type_version.0],
            )
            .await?;
        match row {
            Some(r) => Ok(VersionId(r.get(0))),
            None => Err(StoreError::EntityNotFound(entity_id.0.clone())),
        }
    }

    /// Append a new instance revision pinned to the type's *current* (latest)
    /// revision at this instant -- the "current at the time" reading of the
    /// revision -> type-revision pointer, for callers that are not explicitly
    /// selecting an older type revision. The pin is still materialized as a
    /// concrete version, so the binding stays stable as the type evolves later.
    ///
    /// Errors if the entity does not exist or its type has no revisions yet.
    pub async fn put_version_current_type(
        &self,
        entity_id: &EntityId,
        content: &[u8],
    ) -> Result<VersionId> {
        let client = self.pool.get().await?;
        let row = client
            .query_opt(
                "insert into entity_versions (entity_id, version_id, content, type_id, type_version_id) \
                 select $1, \
                        coalesce((select max(version_id) from entity_versions where entity_id = $1), 0) + 1, \
                        $2, e.type_id, \
                        (select max(tv.version_id) from entity_type_versions tv where tv.type_id = e.type_id) \
                 from entities e where e.id = $1 \
                 returning version_id",
                &[&entity_id.0, &content],
            )
            .await;
        match row {
            Ok(Some(r)) => Ok(VersionId(r.get(0))),
            Ok(None) => Err(StoreError::EntityNotFound(entity_id.0.clone())),
            // A not-null violation on type_version_id means the type had no revisions.
            Err(e) => Err(StoreError::Db(e)),
        }
    }

    /// Load a concrete instance revision.
    pub async fn get_version(&self, entity_id: &EntityId, ver: VersionId) -> Result<EntityRevision> {
        let client = self.pool.get().await?;
        load_entity_revision(&**client, entity_id, ver).await
    }

    /// The highest version number of an entity, or `None` if it has no revisions.
    pub async fn latest_version(&self, entity_id: &EntityId) -> Result<Option<VersionId>> {
        let client = self.pool.get().await?;
        let row = client
            .query_one(
                "select max(version_id) from entity_versions where entity_id = $1",
                &[&entity_id.0],
            )
            .await?;
        Ok(row.get::<_, Option<i64>>(0).map(VersionId))
    }

    /// Add an instance -> instance reference (pinned or floating).
    pub async fn add_edge(
        &self,
        from: (&EntityId, VersionId),
        to_id: &EntityId,
        to: Ref,
    ) -> Result<()> {
        let client = self.pool.get().await?;
        client
            .execute(
                "insert into entity_edges (from_id, from_ver, to_id, to_ver) values ($1, $2, $3, $4)",
                &[&from.0 .0, &from.1 .0, &to_id.0, &to.to_ver()],
            )
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Epochs
    // -----------------------------------------------------------------------

    /// Cut a new epoch: an append-only, fully-pinned snapshot selecting the
    /// latest revision of every entity and every type at this instant. Floating
    /// edges resolve against these selections, so everything reachable from the
    /// epoch is mutually consistent and immutable.
    pub async fn cut_epoch(&self) -> Result<EpochId> {
        let mut client = self.pool.get().await?;
        let tx = client.transaction().await?;
        let epoch_id: i64 = tx
            .query_one(
                "insert into epochs (epoch_id) \
                 values (coalesce((select max(epoch_id) from epochs), 0) + 1) \
                 returning epoch_id",
                &[],
            )
            .await?
            .get(0);
        tx.execute(
            "insert into epoch_entity_selections (epoch_id, entity_id, version_id) \
             select $1, entity_id, max(version_id) from entity_versions group by entity_id",
            &[&epoch_id],
        )
        .await?;
        tx.execute(
            "insert into epoch_entity_type_selections (epoch_id, type_id, version_id) \
             select $1, type_id, max(version_id) from entity_type_versions group by type_id",
            &[&epoch_id],
        )
        .await?;
        tx.commit().await?;
        Ok(EpochId(epoch_id))
    }

    /// Materialize the transitive closure of an entity at an epoch, by traversal.
    /// Pinned edges are followed directly; floating edges resolve through the
    /// epoch's selections. The result holds every reachable instance revision and
    /// every type revision they (transitively) pin -- all concrete, all immutable.
    pub async fn read_closure(&self, epoch: EpochId, root: &EntityId) -> Result<Closure> {
        let client = self.pool.get().await?;

        let root_ver = resolve_entity_in_epoch(&**client, epoch, &root.0)
            .await?
            .ok_or_else(|| StoreError::NotInEpoch {
                id: root.0.clone(),
                epoch: epoch.0,
            })?;

        let mut closure = Closure {
            epoch: Some(epoch),
            ..Default::default()
        };

        let mut inst_seen: HashSet<(String, i64)> = HashSet::new();
        let mut type_seen: HashSet<(String, i64)> = HashSet::new();
        let mut inst_queue: VecDeque<(EntityId, VersionId)> = VecDeque::new();
        let mut type_queue: VecDeque<(TypeId, VersionId)> = VecDeque::new();
        inst_queue.push_back((root.clone(), root_ver));

        // Instance frontier: collect revisions and resolve their edges.
        while let Some((eid, ver)) = inst_queue.pop_front() {
            if !inst_seen.insert((eid.0.clone(), ver.0)) {
                continue;
            }
            let rev = load_entity_revision(&**client, &eid, ver).await?;
            // The revision's pinned type revision seeds the type frontier.
            type_queue.push_back((rev.type_id.clone(), rev.type_version_id));

            for edge in client
                .query(
                    "select to_id, to_ver from entity_edges where from_id = $1 and from_ver = $2",
                    &[&eid.0, &ver.0],
                )
                .await?
            {
                let to_id: String = edge.get(0);
                let to_ver: Option<i64> = edge.get(1);
                let resolved = match to_ver {
                    Some(v) => v,
                    None => resolve_entity_in_epoch(&**client, epoch, &to_id)
                        .await?
                        .ok_or_else(|| StoreError::NotInEpoch {
                            id: to_id.clone(),
                            epoch: epoch.0,
                        })?
                        .0,
                };
                inst_queue.push_back((EntityId(to_id), VersionId(resolved)));
            }
            closure.entities.push(rev);
        }

        // Type frontier: collect type revisions and resolve their type edges.
        while let Some((tid, ver)) = type_queue.pop_front() {
            if !type_seen.insert((tid.0.clone(), ver.0)) {
                continue;
            }
            let trev = load_type_revision(&**client, &tid, ver).await?;

            for edge in client
                .query(
                    "select to_id, to_ver from entity_type_edges where from_id = $1 and from_ver = $2",
                    &[&tid.0, &ver.0],
                )
                .await?
            {
                let to_id: String = edge.get(0);
                let to_ver: Option<i64> = edge.get(1);
                let resolved = match to_ver {
                    Some(v) => v,
                    None => resolve_type_in_epoch(&**client, epoch, &to_id)
                        .await?
                        .ok_or_else(|| StoreError::NotInEpoch {
                            id: to_id.clone(),
                            epoch: epoch.0,
                        })?
                        .0,
                };
                type_queue.push_back((TypeId(to_id), VersionId(resolved)));
            }
            closure.types.push(trev);
        }

        Ok(closure)
    }
}

// ---------------------------------------------------------------------------
// Row loaders, generic over any client (pooled connection or transaction) so
// the closure traversal reuses one connection instead of one per node.
// ---------------------------------------------------------------------------

async fn load_entity_revision<C: GenericClient>(
    client: &C,
    entity_id: &EntityId,
    ver: VersionId,
) -> Result<EntityRevision> {
    let row = client
        .query_opt(
            "select content, type_id, type_version_id from entity_versions \
             where entity_id = $1 and version_id = $2",
            &[&entity_id.0, &ver.0],
        )
        .await?
        .ok_or_else(|| StoreError::VersionNotFound {
            id: entity_id.0.clone(),
            ver: ver.0,
        })?;
    Ok(EntityRevision {
        entity_id: entity_id.clone(),
        version_id: ver,
        content: row.get(0),
        type_id: TypeId(row.get(1)),
        type_version_id: VersionId(row.get(2)),
    })
}

async fn load_type_revision<C: GenericClient>(
    client: &C,
    id: &TypeId,
    ver: VersionId,
) -> Result<TypeRevision> {
    let row = client
        .query_opt(
            "select content, type_tag from entity_type_versions where type_id = $1 and version_id = $2",
            &[&id.0, &ver.0],
        )
        .await?
        .ok_or_else(|| StoreError::TypeVersionNotFound {
            id: id.0.clone(),
            ver: ver.0,
        })?;
    Ok(TypeRevision {
        type_id: id.clone(),
        version_id: ver,
        content: row.get(0),
        type_tag: row.get(1),
    })
}

async fn resolve_entity_in_epoch<C: GenericClient>(
    client: &C,
    epoch: EpochId,
    entity_id: &str,
) -> Result<Option<VersionId>> {
    let params: [&(dyn ToSql + Sync); 2] = [&epoch.0, &entity_id];
    let row = client
        .query_opt(
            "select version_id from epoch_entity_selections where epoch_id = $1 and entity_id = $2",
            &params,
        )
        .await?;
    Ok(row.map(|r| VersionId(r.get(0))))
}

async fn resolve_type_in_epoch<C: GenericClient>(
    client: &C,
    epoch: EpochId,
    type_id: &str,
) -> Result<Option<VersionId>> {
    let params: [&(dyn ToSql + Sync); 2] = [&epoch.0, &type_id];
    let row = client
        .query_opt(
            "select version_id from epoch_entity_type_selections where epoch_id = $1 and type_id = $2",
            &params,
        )
        .await?;
    Ok(row.map(|r| VersionId(r.get(0))))
}
