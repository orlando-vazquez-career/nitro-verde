//! Ruta SSE (§C-5): `GET /api/conversations/{id}/events` — tail de deltas
//! anti-pérdida + `resync` ante `Lagged` + keep-alive cada 15s.
//!
//! Patrón anti-pérdida (riesgo ADR #1): la UI primero pide el snapshot
//! (`GET /transcript`, T11 api.rs) y LUEGO abre este stream; los deltas se
//! aplican sobre el snapshot con dedup por `seq`. Si el receiver se atrasa
//! más que el cap del broadcast (256), su `Err(Lagged)` se mapea a
//! `event: resync` y la UI refetchea el transcript — ningún evento se
//! pierde en silencio.
//!
//! Riesgo ADR #3 (graceful shutdown): el stream envuelve al broadcast con
//! un tail cancelable (`CancellableTail`): al cancelarse el token, el body
//! SSE TERMINA y la conexión queda idle, así `with_graceful_shutdown` (T13)
//! no queda colgado esperando conexiones vivas. El keep-alive de 15s además
//! mantiene la conexión activa y detecta clientes muertos.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{Router, get};
use nv_core::conversation::ChatEvent;
use nv_core::error::NvError;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

/// Intervalo del comentario keep-alive (§C-5: cada 15s).
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// Router de la ruta SSE, sin estado aplicado. Recibe el token de shutdown
/// como `Extension` (el `AppState` congelado de T10 no puede llevarlo).
pub fn router(shutdown: CancellationToken) -> Router<AppState> {
    Router::new()
        .route("/api/conversations/{id}/events", get(events))
        .layer(axum::Extension(shutdown))
}

/// Handler del tail SSE. La suscripción se crea ACÁ (post-snapshot del
/// cliente); un id desconocido responde 404 ANTES de abrir el stream.
async fn events(
    State(state): State<AppState>,
    axum::Extension(shutdown): axum::Extension<CancellationToken>,
    Path(id): Path<String>,
) -> Response {
    match state.registry.subscribe(&id) {
        Ok(rx) => Sse::new(chat_event_stream(rx, shutdown))
            .keep_alive(
                KeepAlive::new()
                    .interval(KEEP_ALIVE_INTERVAL)
                    .text("keep-alive"),
            )
            .into_response(),
        Err(NvError::ConversationNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "conversation_not_found", "conversation_id": id})),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "error suscribiendo al tail SSE");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal"})),
            )
                .into_response()
        }
    }
}

/// Mensaje de alto nivel del tail, ANTES de serializar a SSE. Tipo público
/// para que el mapeo `Lagged → resync` sea testeable sin socket (§T11-e).
#[derive(Debug, Clone, PartialEq)]
pub enum SseMessage {
    /// Delta nuevo: se emite como `event: message\ndata: <ChatEvent JSON>`.
    /// Boxeado: ChatEvent es grande y Resync es ZST (clippy large_enum_variant).
    Event(Box<ChatEvent>),
    /// El receiver se atrasó más que el cap: `event: resync\ndata: {}` y la
    /// UI refetchea `/transcript` (riesgo ADR #1).
    Resync,
}

/// Mapeo puro del item del broadcast a mensaje SSE (§C-5).
pub fn map_broadcast_item(item: Result<ChatEvent, BroadcastStreamRecvError>) -> SseMessage {
    match item {
        Ok(event) => SseMessage::Event(Box::new(event)),
        Err(BroadcastStreamRecvError::Lagged(skipped)) => {
            tracing::warn!(skipped, "receiver SSE atrasado: emitiendo resync");
            SseMessage::Resync
        }
    }
}

/// Stream del tail: deltas del broadcast → `SseMessage` → `Event` SSE.
/// Termina cuando se cancela el token de shutdown (riesgo ADR #3).
pub fn chat_event_stream(
    rx: broadcast::Receiver<ChatEvent>,
    shutdown: CancellationToken,
) -> impl Stream<Item = Result<Event, Infallible>> {
    CancellableTail::new(rx, shutdown)
        .map(map_broadcast_item)
        .map(|message| {
            let event = match message {
                SseMessage::Event(chat_event) => {
                    // La serialización de ChatEvent es infalible en la
                    // práctica; si fallara, `{}` y log (nunca cortar el tail).
                    let data = serde_json::to_string(&chat_event).unwrap_or_else(|e| {
                        tracing::error!(error = %e, "ChatEvent no serializó: data vacía");
                        "{}".to_string()
                    });
                    Event::default().event("message").data(data)
                }
                SseMessage::Resync => Event::default().event("resync").data("{}"),
            };
            Ok(event)
        })
}

/// Tail del broadcast que se corta al cancelarse el token (riesgo ADR #3:
/// los streams SSE NO deben bloquear el graceful shutdown).
///
/// `tokio_stream::StreamExt` no tiene `take_until` (vive en `futures-util`,
/// fuera del stack fijo), así que la cancelación se implementa acá: en cada
/// `poll_next` se pollea PRIMERO el futuro de cancelación (registra el
/// waker del task, así la cancelación despierta al stream aunque el
/// broadcast esté quieto) y recién después el receiver.
struct CancellableTail {
    rx: BroadcastStream<ChatEvent>,
    cancelled: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl CancellableTail {
    fn new(rx: broadcast::Receiver<ChatEvent>, shutdown: CancellationToken) -> Self {
        Self {
            rx: BroadcastStream::new(rx),
            cancelled: Box::pin(shutdown.cancelled_owned()),
        }
    }
}

impl Stream for CancellableTail {
    type Item = Result<ChatEvent, BroadcastStreamRecvError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // BroadcastStream y el futuro boxed son Unpin: proyección segura.
        let this = self.get_mut();
        if this.cancelled.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Pin::new(&mut this.rx).poll_next(cx)
    }
}
