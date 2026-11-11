//! `audigma-configurator`: the core behind `audigma configure`.
//!
//! Today this crate holds the schema pipeline ([`schema`]); the draft, writer and assists land in
//! later sessions of the configurator plan. It re-exports the metadata types so frontends
//! depend on one crate.

pub mod schema;

pub use audigma_settings::meta;
