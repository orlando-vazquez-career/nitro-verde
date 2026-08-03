//! Rutas de descarga de media (T19): el camino Meta-real de DOS pasos que
//! hace `download_media(media_id)` del cliente WhatsApp de akiveo-api:
//!
//! 1. `GET /graph/{version}/{media_id}` → info JSON Meta-like:
//!    `{"messaging_product":"whatsapp","url":"<public_url>/media/{id}",
//!      "mime_type":...,"sha256":...,"file_size":N}`.
//!    La `url` es ABSOLUTA (`public_url` de §C-7): el target la usa tal cual
//!    para el paso 2 (`info_data["url"]` es obligatorio en akiveo-api).
//! 2. `GET /media/{media_id}` → los BYTES crudos con `Content-Type` del mime
//!    guardado en el `put`.
//!
//! Colisión de rutas (verificada por test): `/graph/{version}/{media_id}`
//! tiene 2 segmentos tras `/graph` y `/graph/{version}/{phone_number_id}/messages`
//! tiene 3 — axum las distingue por aridad, no hay conflicto de params.
//!
//! Handlers finos: la guarda es `MediaStore` (T19, nv-engine); acá solo se
//! cablea HTTP ↔ store. Todo sync, ningún guard cruza `.await` (ADR #4).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{Router, get};

use crate::state::AppState;

/// Router de media, sin estado aplicado: el composition root
/// (`routes::router`) lo mergea y aplica `.with_state(...)` una sola vez.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/graph/{version}/{media_id}", get(media_info))
        .route("/media/{media_id}", get(media_bytes))
}

/// Paso 1 del download: info Meta-like del adjunto. `{version}` es opaco
/// (§C-3). 404 Meta-like si el `media_id` no existe.
async fn media_info(
    State(state): State<AppState>,
    Path((version, media_id)): Path<(String, String)>,
) -> Response {
    tracing::debug!(%version, %media_id, "fake-meta: info de media pedida");
    match state.media.get(&media_id) {
        Some(entry) => Json(serde_json::json!({
            "messaging_product": "whatsapp",
            "url": state.media_download_url(&media_id),
            "mime_type": entry.mime,
            "sha256": entry.sha256,
            "file_size": entry.file_size(),
        }))
        .into_response(),
        None => media_not_found(&media_id),
    }
}

/// Paso 2 del download: los bytes crudos con el `Content-Type` guardado.
/// 404 Meta-like si el `media_id` no existe.
async fn media_bytes(State(state): State<AppState>, Path(media_id): Path<String>) -> Response {
    match state.media.get(&media_id) {
        Some(entry) => ([(header::CONTENT_TYPE, entry.mime)], entry.bytes).into_response(),
        None => media_not_found(&media_id),
    }
}

/// Body de error Meta-like de media inexistente (misma forma §C-3).
fn media_not_found(media_id: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": {
                "message": format!("media no encontrado: {media_id}"),
                "type": "NVMediaNotFound",
                "code": 404,
            }
        })),
    )
        .into_response()
}
