//! Ruta fake-Meta (§C-3): `POST /graph/{version}/{phone_number_id}/messages`.
//!
//! Recibe el outbound del bot (akiveo-api → NitroVerde). Handler fino: la
//! lógica de parseo/eventos vive en `nv_engine::fake_meta` (T8), la de
//! conversaciones en `nv_engine::registry` (T7) y la de fallos en
//! `nv_engine::fault` (T5). Acá solo se cablea:
//!
//! 0. Guard anti-CSRF: Content-Type DEBE ser `application/json` (se acepta
//!    variante con charset); si no, 415 `NVUnsupportedMediaType`. Sin este
//!    guard, cualquier pestaña del browser podría hacer POSTs de escritura
//!    con `text/plain` (sin preflight CORS).
//! 1. Leer body bytes.
//! 2. `fault.decide()` (§C-4: UNA vez por request, ANTES de parsear).
//!    - `Respond{status}` → status + body `NVInjected` (§C-3).
//!    - `Delay{ms}` → `sleep` y se sigue como `Pass`.
//!    - `Pass` → seguir.
//! 3. `process_outbound` → evento + respuesta Meta-like + `to` (UN solo
//!    parseo: el `to` sale del payload ya deserializado, sin re-parseo).
//! 4. Conversación por `to` (`get_or_create_by_phone`, §C-3 auto-creación:
//!    el outbound del bot NUNCA se pierde) → `append_event`.
//! 5. Responder la `MetaResponse`.
//!
//! Riesgo ADR #4: el guard del `RwLock<FaultPolicy>` se suelta ANTES del
//! `sleep` — se clona la decisión (`FaultDecision` es `Copy`) en una
//! sección crítica corta y síncrona.

use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{Router, post};
use nv_core::error::NvError;
use nv_engine::fake_meta::process_outbound;
use nv_engine::fault::FaultDecision;

use crate::state::AppState;

/// Router de la ruta fake-Meta, sin estado aplicado: T11/T13 lo mergean y
/// aplican `.with_state(...)` una sola vez en el composition root.
pub fn router() -> Router<AppState> {
    Router::new().route("/graph/{version}/{phone_number_id}/messages", post(post_message))
}

/// Handler del outbound del bot. `{version}` y `{phone_number_id}` son
/// path params opacos (§C-3): se aceptan todos y solo se eco-loguean.
async fn post_message(
    State(state): State<AppState>,
    Path((version, phone_number_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    tracing::debug!(%version, %phone_number_id, "fake-meta: outbound recibido");

    // (0) Guard anti-CSRF: solo se acepta `application/json`. Un POST con
    //     `text/plain` no dispara preflight CORS, así que sin este check
    //     cualquier pestaña del browser podría escribir transcripts.
    if !is_json_content_type(&headers) {
        return unsupported_media_type();
    }

    // (2) FaultPolicy ANTES de parsear. Guard corto: la decisión se clona
    //     y el lock se suelta al cerrar el bloque — NADA de guards vivos a
    //     través del `.await` del sleep (riesgo ADR #4).
    let decision = {
        let policy = state.fault.read().expect("fault lock poisoned");
        policy.decide()
    };

    match decision {
        FaultDecision::Respond { status } => return injected_error(status),
        FaultDecision::Delay { ms } => {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            // §C-4: tras el delay se responde normal (decide() ya corrió).
        }
        FaultDecision::Pass => {}
    }

    // (3) Parseo + evento de transcript + `to` (engine, T8; UN solo parseo).
    let (event, meta_response, to) = match process_outbound(&body) {
        Ok(ok) => ok,
        Err(NvError::Parse(msg)) => return parse_error(&msg),
        // Defensivo: process_outbound hoy solo devuelve Parse.
        Err(e) => return parse_error(&e.to_string()),
    };

    // (4) Resolver conversación por `to` y persistir el evento. Los bodies
    //     sin `to` (mark_as_read) no son atribuibles a ninguna conversación:
    //     no tocan transcript y se responden igual (§C-3, `{"success":true}`).
    if let Some(to) = to {
        let conversation_id = state.registry.get_or_create_by_phone(&to);
        if let Err(e) = state.registry.append_event(&conversation_id, event) {
            // get_or_create garantiza existencia; si falla igual se responde
            // (mock honesto: el cliente recibe su 200 y el error queda logueado).
            tracing::error!(error = %e, %conversation_id, "append_event falló tras get_or_create");
        }
    }

    // (5) Respuesta Meta-like (200 implícito).
    Json(meta_response).into_response()
}

/// ¿El Content-Type es `application/json`? Se acepta la variante con
/// parámetros (`application/json; charset=utf-8`), case-insensitive.
/// Header ausente o no-UTF8 → `false` (415).
fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
}

/// Body de error Meta-like (§C-3): `{"error":{"message","type","code"}}`.
fn error_body(message: &str, kind: &str, code: u16) -> serde_json::Value {
    serde_json::json!({
        "error": { "message": message, "type": kind, "code": code }
    })
}

/// Fallo inyectado activo (§C-3/§C-4): status configurado + `NVInjected`.
fn injected_error(status: u16) -> Response {
    let status_code =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status_code,
        Json(error_body("NV fault injection", "NVInjected", status)),
    )
        .into_response()
}

/// JSON malformado o shape roto de tipo conocido (§C-3): 400 `NVParseError`.
fn parse_error(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(error_body(message, "NVParseError", 400)),
    )
        .into_response()
}

/// Content-Type no-JSON (guard anti-CSRF): 415 `NVUnsupportedMediaType`.
fn unsupported_media_type() -> Response {
    (
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        Json(serde_json::json!({
            "error": {
                "message": "Content-Type must be application/json",
                "type": "NVUnsupportedMediaType",
            }
        })),
    )
        .into_response()
}
