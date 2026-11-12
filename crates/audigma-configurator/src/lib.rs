//! `audigma-configurator`: the core behind `audigma configure`.
//!
//! Today this crate holds the schema pipeline ([`schema`]) and the assists ([`assist`]: probes,
//! capability check, benchmark), the [`draft`] being edited and the config [`writer`]. It
//! re-exports the metadata types so frontends
//! depend on one crate.

pub mod assist;
pub mod draft;
pub mod schema;
pub mod writer;

pub use audigma_settings::meta;
