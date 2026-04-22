pub mod value;
pub mod coeffects;
pub mod codegen;
pub mod type_check;
pub mod cel_to_ir;
pub mod normalizer;

pub mod expr_gen {
    include!(concat!(env!("OUT_DIR"), "/expr_gen.rs"));
}

pub mod cluonflux {
    pub mod meta {
        include!(concat!(env!("OUT_DIR"), "/cluonflux.meta.rs"));
    }
}

pub mod cel {
    pub mod expr {
        include!(concat!(env!("OUT_DIR"), "/cel.expr.rs"));
    }
}
