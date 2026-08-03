//! Rutas de la API de control de NitroVerde (§C-1): conversaciones,
//! transcript, input del usuario, faults y health.
//!
//! Handlers finos: la lógica de conversaciones vive en `Registry` (T7), la
//! del webhook saliente en `Orchestrator` (T9) y la de fallos en
//! `FaultPolicy` (T5). Acá solo se cablea HTTP ↔ engine.
//!
//! Regla de guards (riesgo ADR #4): el `RwLock<FaultPolicy>` es std y sus
//! secciones críticas son sync y cortas (set/get/clear); NINGÚN guard cruza
//! un `.await` — el único await de este módulo es `send_user_input`, que
//! opera sobre `Arc<Orchestrator>` clonado por el extractor `State`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{Router, get, post};
use nv_core::conversation::UserInput;
use nv_core::error::NvError;
use nv_engine::fault::FaultPolicy;
use nv_engine::orchestrator::Orchestrator;
use serde::Deserialize;

use crate::state::AppState;

/// Estado de las rutas `/api/*`. Envuelve al `AppState` congelado de T10
/// (registry + fault + phone_number_id) y le suma el `Orchestrator` de T9,
/// que T10 aún no podía conocer (T9 corría en paralelo). Clonar es barato:
/// todo es `Arc`/String.
#[derive(Clone)]
pub struct ApiState {
    pub app: AppState,
    pub orchestrator: Arc<Orchestrator>,
}

/// Router de la API de control, sin estado aplicado: el composition root
/// (`routes::router`, T11) le aplica `.with_state(...)` una sola vez.
pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/conversations", post(create_conversation))
        .route("/api/conversations/{id}/transcript", get(get_transcript))
        .route(
            "/api/conversations/{id}/messages",
            post(post_user_message),
        )
        .route("/api/conversations/{id}/media", post(post_user_media))
        .route(
            "/api/faults",
            post(set_fault).get(get_fault).delete(clear_fault),
        )
}

/// `GET /api/health` (§C-1): `200 {"status":"ok"}`.
async fn health() -> Response {
    Json(serde_json::json!({"status": "ok"})).into_response()
}

/// Body de `POST /api/conversations` (§C-1): `{"phone":"56900000001"}`
/// (phone = usuario simulado, SIN `+`).
#[derive(Debug, Deserialize)]
struct CreateConversation {
    phone: String,
}

/// `POST /api/conversations` → `201 {"conversation_id":"...","phone":"..."}`.
async fn create_conversation(
    State(state): State<ApiState>,
    Json(body): Json<CreateConversation>,
) -> Response {
    let phone = body.phone.trim();
    if phone.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_phone"})),
        )
            .into_response();
    }
    let conversation_id = state.app.registry.create(phone);
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "conversation_id": conversation_id,
            "phone": phone,
        })),
    )
        .into_response()
}

/// `GET /api/conversations/{id}/transcript` (§C-1): snapshot completo
/// anti-pérdida SSE → `200 {"conversation_id":"...","events":[ChatEvent]}`.
async fn get_transcript(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    match state.app.registry.snapshot(&id) {
        Ok(events) => Json(serde_json::json!({
            "conversation_id": id,
            "events": events,
        }))
        .into_response(),
        Err(e) => conversation_error(e),
    }
}

/// `POST /api/conversations/{id}/messages` (§C-1): input del usuario →
/// webhook firmado al target (Orchestrator, T9).
///
/// - Éxito → `202 {"accepted":true,"wamid":"wamid.NV_*"}`.
/// - Target no-2xx → `502 {"error":"webhook_target","status":N}` (el evento
///   del usuario queda registrado y se suma una nota `system_note`, §T9).
/// - Conversación desconocida → 404.
async fn post_user_message(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(input): Json<UserInput>,
) -> Response {
    match state.orchestrator.send_user_input(&id, input).await {
        Ok(wamid) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"accepted": true, "wamid": wamid})),
        )
            .into_response(),
        Err(NvError::WebhookTarget { status }) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": "webhook_target", "status": status})),
        )
            .into_response(),
        Err(e) => conversation_error(e),
    }
}

/// Fallo de parseo/validación del multipart (T19): 400 con código estable.
fn media_bad_request(code: &str, detail: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": code, "detail": detail})),
    )
        .into_response()
}

/// `POST /api/conversations/{id}/media` (T19): adjuntar imagen/PDF del
/// usuario simulado. Multipart con campos:
///
/// - `file` (obligatorio): bytes + content-type + filename del adjunto.
/// - `kind` (obligatorio): `"image"` | `"document"`.
/// - `caption` (opcional): texto que acompaña al adjunto en el webhook.
///
/// Flujo: bytes → `MediaStore::put` (sha256 real calculado ahí) →
/// `UserInput::Image/Document` → webhook firmado vía orchestrator (misma
/// ruta §C-2 que el texto: UNA serialización, firma sobre esos bytes).
///
/// - Éxito → `200 {"accepted":true,"wamid":"...","media_id":"media_*"}`.
///   (200 y no 202 como `/messages`: el contrato lo fijó T19 así.)
/// - Multipart roto / sin `file` / `kind` inválido → 400.
/// - Target no-2xx → `502 {"error":"webhook_target","status":N,"media_id"}`:
///   el adjunto QUEDA guardado, el evento registrado y se suma una nota
///   `system_note` (§T9).
/// - Conversación desconocida → 404.
///
/// L-13: NO se valida tamaño máximo ni tipo real del archivo — el mime se
/// toma del header del multipart, como hace el cliente de WhatsApp.
async fn post_user_media(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Response {
    // (1) Extraer los campos del multipart. Los metadatos del campo `file`
    //     (mime/filename) se copian a Strings ANTES de consumir el campo con
    //     `bytes()`; ningún guard cruza los `.await` (riesgo ADR #4).
    let mut file: Option<(Vec<u8>, String, Option<String>)> = None;
    let mut kind: Option<String> = None;
    let mut caption: Option<String> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(e) => return media_bad_request("multipart_invalid", &e.to_string()),
        };
        match field.name().map(str::to_string).as_deref() {
            Some("file") => {
                let mime = field
                    .content_type()
                    .map(str::to_string)
                    .unwrap_or_else(|| "application/octet-stream".to_string());
                let filename = field.file_name().map(str::to_string);
                match field.bytes().await {
                    Ok(bytes) => file = Some((bytes.to_vec(), mime, filename)),
                    Err(e) => return media_bad_request("multipart_invalid", &e.to_string()),
                }
            }
            Some("kind") => kind = field.text().await.ok(),
            Some("caption") => caption = field.text().await.ok(),
            _ => {}
        }
    }

    let kind = kind.unwrap_or_default();
    let (bytes, mime, filename) = match file {
        Some(file) => file,
        None => return media_bad_request("missing_file", "falta el campo `file`"),
    };
    let caption = caption.filter(|c| !c.trim().is_empty());

    // (2) Guardar en el store: devuelve el media_id y deja calculado el
    //     sha256 real de los bytes (lo reusa la info Meta-like).
    let media_id = state.app.media.put(bytes, mime.clone());
    let sha256 = state.app.media.get(&media_id).map(|entry| entry.sha256);

    // (3) Input del usuario → webhook firmado (orchestrator, §C-2/T9).
    let input = match kind.as_str() {
        "image" => UserInput::Image {
            media_id: media_id.clone(),
            mime_type: mime,
            caption,
            sha256,
        },
        "document" => UserInput::Document {
            media_id: media_id.clone(),
            mime_type: mime,
            caption,
            filename: filename.unwrap_or_else(|| "documento".to_string()),
            sha256,
        },
        other => {
            return media_bad_request(
                "invalid_kind",
                &format!("kind debe ser \"image\" o \"document\", vino {other:?}"),
            );
        }
    };

    match state.orchestrator.send_user_input(&id, input).await {
        Ok(wamid) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "accepted": true,
                "wamid": wamid,
                "media_id": media_id,
            })),
        )
            .into_response(),
        Err(NvError::WebhookTarget { status }) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": "webhook_target",
                "status": status,
                "media_id": media_id,
            })),
        )
            .into_response(),
        Err(e) => conversation_error(e),
    }
}

/// `POST /api/faults` (§C-4): setea la política activa (`{}` = off) →
/// `200 {"ok":true}`. Guard corto y sync: jamás cruza un `.await` (ADR #4).
async fn set_fault(State(state): State<ApiState>, Json(policy): Json<FaultPolicy>) -> Response {
    *state.app.fault.write().expect("fault lock poisoned") = policy;
    Json(serde_json::json!({"ok": true})).into_response()
}

/// `GET /api/faults` (§C-1): `200 <policy>` (la activa, forma §C-4).
async fn get_fault(State(state): State<ApiState>) -> Response {
    let policy = state.app.fault.read().expect("fault lock poisoned").clone();
    Json(policy).into_response()
}

/// `DELETE /api/faults` (§C-1): limpia la política (off) → `200 {"ok":true}`.
async fn clear_fault(State(state): State<ApiState>) -> Response {
    *state.app.fault.write().expect("fault lock poisoned") = FaultPolicy::default();
    Json(serde_json::json!({"ok": true})).into_response()
}

/// Mapeo de errores de dominio a HTTP para los endpoints `/api/*`.
fn conversation_error(err: NvError) -> Response {
    match err {
        NvError::ConversationNotFound(id) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "conversation_not_found", "conversation_id": id})),
        )
            .into_response(),
        other => {
            tracing::error!(error = %other, "error interno en /api");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal"})),
            )
                .into_response()
        }
    }
}
