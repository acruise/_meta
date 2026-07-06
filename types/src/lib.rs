pub mod value;
pub mod coeffects;
pub mod effects;
pub mod external_fn;
pub mod validation;

#[cfg(feature = "yaml-catalog")]
pub mod udf_catalog;

#[cfg(feature = "yaml-catalog")]
pub mod value_type_yaml;

pub mod cluonflux {
    pub mod meta {
        include!(concat!(env!("OUT_DIR"), "/cluonflux.meta.rs"));
    }
}
