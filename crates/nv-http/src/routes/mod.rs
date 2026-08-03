//! Rutas HTTP de NitroVerde.
//!
//! - `fake_meta`: endpoint al que pega el cliente WhatsApp de akiveo-api (§C-3).
//! - `media`: descarga de adjuntos en dos pasos Meta-real (T19).
//! - `api`: API de control §C-1 (conversaciones, transcript, messages, faults, health).
//! - `sse`: tail de deltas §C-5 (snapshot+tail del lado del cliente).
//!
//! [`router`] es el punto de ensamblado público de nv-http (T11): recibe el
//! estado compartido, el orchestrator y el token de shutdown, y deja un
//! `Router` listo para que T12 monte la UI y T13 solo haga wiring.

pub mod api;
pub mod fake_meta;
pub mod media;
pub mod sse;

use std::sync::Arc;

use axum::Router;
use nv_engine::orchestrator::Orchestrator;
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

/// Ensambla el router público de NitroVerde (§C-1):
///
/// - `POST /graph/{version}/{phone_number_id}/messages` (fake-Meta, T10).
/// - `GET /graph/{version}/{media_id}` y `GET /media/{media_id}` (media, T19).
/// - `/api/*` (T11): conversations, transcript, messages, faults, health.
/// - `POST /api/conversations/{id}/media` (multipart, T19).
/// - `GET /api/conversations/{id}/events` (SSE §C-5).
///
/// `shutdown` (riesgo ADR #3) cancela los streams SSE vivos para que
/// `with_graceful_shutdown` (T13) no se bloquee. Cada sub-router aplica su
/// estado UNA vez acá (`fake_meta`, `media` y `sse` con `AppState`; `api`
/// con `ApiState`, que envuelve `AppState` + orchestrator).
pub fn router(
    state: AppState,
    orchestrator: Arc<Orchestrator>,
    shutdown: CancellationToken,
) -> Router {
    let api_state = api::ApiState {
        app: state.clone(),
        orchestrator,
    };
    Router::new()
        .merge(fake_meta::router().with_state(state.clone()))
        .merge(media::router().with_state(state.clone()))
        .merge(sse::router(shutdown).with_state(state))
        .merge(api::router().with_state(api_state))
        .merge(crate::ui_assets::router())
}
