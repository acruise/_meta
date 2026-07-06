//! Wire-format IO for [`meta_types`] values.
//!
//! `meta-types` owns the pure data model -- [`Value`](meta_types::value::Value),
//! [`ValueType`](meta_types::value::ValueType), conformance, validation --
//! and stays dependency-light. This crate owns the *syntax frontiers*: code
//! that moves `Value`s across wire formats under the direction of a
//! `ValueType`. JSON is the first resident ([`json`]); other formats land
//! here as they are needed.

pub mod json;
