pub mod value;
pub mod coeffects;
pub mod effects;
pub mod external_fn;

#[cfg(feature = "yaml-catalog")]
pub mod udf_catalog;

pub mod cluonflux {
    pub mod meta {
        include!(concat!(env!("OUT_DIR"), "/cluonflux.meta.rs"));
    }
}
