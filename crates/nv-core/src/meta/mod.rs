//! Contrato wire de Meta (WhatsApp).
//!
//! Estos tipos SON el contrato wire (regla ADR: sin DTOs espejo).
//! - `cloud_api`: payloads OUTBOUND (akiveo-api → fake-Meta), C-3.
//! - `webhook`: payload INBOUND (NitroVerde → webhook del bot), C-2.

pub mod cloud_api;
pub mod webhook;
