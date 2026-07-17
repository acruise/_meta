//! Wire-format IO for [`meta_types`] values.
//!
//! `meta-types` owns the pure data model -- [`Value`](meta_types::value::Value),
//! [`ValueType`](meta_types::value::ValueType), validation -- and stays
//! dependency-light. This crate owns the *syntax frontiers*: code that moves
//! `Value`s across wire formats under the direction of a `ValueType`, and
//! the structural [`conformance`] check applied at those frontiers (a
//! proto-decoded value is checked after the fact; values parsed by [`json`]
//! or [`yaml`] conform by construction, and the two frontiers share their
//! scalar canonicalization code so they cannot disagree). Other formats land
//! here as they are needed.

pub mod conformance;
pub mod json;
pub mod yaml;
