use thiserror::Error;

/// Errors surfaced by the control-plane store.
///
/// Referential-integrity violations from Postgres (a dangling edge, a revision
/// pinned to a type that disagrees with the header) arrive as [`StoreError::Db`];
/// they are the database doing its job, not bugs in this layer. The dedicated
/// variants cover conditions this layer checks or interprets itself.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("entity {0:?} does not exist")]
    EntityNotFound(String),

    #[error("entity type {0:?} does not exist")]
    TypeNotFound(String),

    #[error("version {ver} of entity {id:?} does not exist")]
    VersionNotFound { id: String, ver: i64 },

    #[error("version {ver} of type {id:?} does not exist")]
    TypeVersionNotFound { id: String, ver: i64 },

    #[error("epoch {0} does not exist")]
    EpochNotFound(i64),

    #[error("entity {id:?} has no version selected in epoch {epoch}")]
    NotInEpoch { id: String, epoch: i64 },

    #[error("postgres error: {0}")]
    Db(#[from] tokio_postgres::Error),

    #[error("connection pool error: {0}")]
    Pool(#[from] deadpool_postgres::PoolError),

    #[error("connection pool build error: {0}")]
    PoolBuild(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;
