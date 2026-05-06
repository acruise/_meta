pub use meta_types::value;
pub use meta_types::coeffects;
pub use meta_types::effects;
pub use meta_types::external_fn;
pub use meta_types::cluonflux;

pub mod codegen;
pub mod type_check;
pub mod cel_to_ir;
pub mod event_proto_codegen;
pub mod normalizer;
pub mod udf_resolver;

pub mod expr_gen {
    include!(concat!(env!("OUT_DIR"), "/expr_gen.rs"));
}

pub mod cel {
    pub mod expr {
        include!(concat!(env!("OUT_DIR"), "/cel.expr.rs"));
    }
}
