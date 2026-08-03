//! nv-engine: motor de NitroVerde (fake-Meta, orchestrator, registry, faults).
//!
//! Conoce tokio/reqwest, NO axum.

pub mod fake_meta;
pub mod fault;
pub mod media;
pub mod registry;
pub mod http_client;
pub mod orchestrator;
