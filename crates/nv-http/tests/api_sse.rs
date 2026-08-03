//! Tests de la API de control + SSE (T11) vía `Router::oneshot`/`Service::call`
//! (tower, sin levantar red — patrón del ADR §Testing nivel 3).
//!
//! Cubren §C-1/§C-4/§C-5: (a) POST /api/conversations → 201; (b) POST
//! messages text con `WebhookSender` capturante → 202 + bytes firmados;
//! (c) GET transcript contiene el evento `user_text`; (d) faults
//! POST → GET → DELETE; (e) overflow del broadcast → `resync` (mapeo puro,
//! sin socket); (f) health → `{"status":"ok"}`. Extras: delta en vivo por el
//! stream HTTP, 404 de conversación desconocida y 502 de webhook_target.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use nv_core::conversation::{ChatEvent, Direction, EventKind};
use nv_core::error::NvError;
use nv_core::ports::{BoxFuture, WebhookSender};
use nv_engine::orchestrator::Orchestrator;
use nv_http::routes::{self, sse};
use nv_http::state::AppState;
use tokio_util::sync::CancellationToken;
use tower::{Service, ServiceExt};

const PHONE: &str = "56900000001";
const WEBHOOK_URL: &str = "http://localhost:8002/api/v1/whatsapp/webhook";
const SECRET: &str = "nv-dev-secret";

/// Sender mock capturante (§T11-b): guarda (url, bytes, firma) y puede
/// fallar con un status HTTP configurable. Sin red.
struct CapturingSender {
    calls: Mutex<Vec<(String, Vec<u8>, String)>>,
    fail_with: Option<u16>,
}

impl CapturingSender {
    fn ok() -> Self {
        Self { calls: Mutex::new(vec![]), fail_with: None }
    }
    fn failing(status: u16) -> Self {
        Self { calls: Mutex::new(vec![]), fail_with: Some(status) }
    }
}

impl WebhookSender for CapturingSender {
    fn send<'a>(
        &'a self,
        url: &'a str,
        body: Vec<u8>,
        signature: &'a str,
    ) -> BoxFuture<'a, Result<(), NvError>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push((url.to_string(), body, signature.to_string()));
            match self.fail_with {
                Some(status) => Err(NvError::WebhookTarget { status }),
                None => Ok(()),
            }
        })
    }
}

/// Fixture: estado + orchestrator con sender capturante + router ensamblado.
fn fixture(sender: Arc<CapturingSender>) -> (AppState, Router) {
    let state = AppState::new("NV_PHONE_ID", "http://127.0.0.1:9100");
    let orchestrator = Arc::new(Orchestrator::new(
        sender,
        Arc::clone(&state.registry),
        WEBHOOK_URL,
        SECRET,
        "NV_PHONE_ID",
    ));
    let app = routes::router(state.clone(), orchestrator, CancellationToken::new());
    (state, app)
}

/// POST JSON y parseo de la respuesta (status + body JSON).
async fn post_json(app: Router, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
    let request = Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("body no es JSON ({e}): {}", String::from_utf8_lossy(&bytes)));
    (status, json)
}

/// GET y parseo de la respuesta (status + body JSON).
async fn get_json(app: Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let request = Request::get(uri).body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("body no es JSON ({e}): {}", String::from_utf8_lossy(&bytes)));
    (status, json)
}

/// Crea una conversación vía la API y devuelve su id.
async fn create_conversation(app: Router) -> String {
    let (status, json) = post_json(app, "/api/conversations", r#"{"phone":"56900000001"}"#).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["phone"], PHONE);
    json["conversation_id"]
        .as_str()
        .expect("conversation_id")
        .to_string()
}

// (a) POST /api/conversations → 201 con id.
#[tokio::test]
async fn api_create_conversation_201_con_id() {
    let (state, app) = fixture(Arc::new(CapturingSender::ok()));
    let conversation_id = create_conversation(app).await;

    assert!(conversation_id.starts_with("conv_"), "id: {conversation_id}");
    // La conversación quedó registrada y vacía.
    assert_eq!(state.registry.snapshot(&conversation_id).unwrap(), Vec::new());
    assert_eq!(state.registry.phone_of(&conversation_id).unwrap(), PHONE);
}

#[tokio::test]
async fn api_create_conversation_phone_vacio_400() {
    let (_state, app) = fixture(Arc::new(CapturingSender::ok()));
    let (status, json) = post_json(app, "/api/conversations", r#"{"phone":"  "}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"], "invalid_phone");
}

// (b) POST messages text → 202 y el mock recibió bytes firmados.
#[tokio::test]
async fn api_post_message_text_202_y_sender_recibio_bytes_firmados() {
    let sender = Arc::new(CapturingSender::ok());
    let (_state, app) = fixture(Arc::clone(&sender));
    let conversation_id = create_conversation(app.clone()).await;

    let (status, json) = post_json(
        app,
        &format!("/api/conversations/{conversation_id}/messages"),
        r#"{"kind":"text","body":"4"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(json["accepted"], true);
    let wamid = json["wamid"].as_str().expect("wamid");
    assert!(wamid.starts_with("wamid.NV_"), "wamid: {wamid}");

    // El mock recibió UN POST con los bytes firmados (firma sha256=<hex>).
    let calls = sender.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let (url, body, signature) = &calls[0];
    assert_eq!(url, WEBHOOK_URL);
    assert!(signature.starts_with("sha256=") && signature.len() == 7 + 64);
    // Re-verificación criptográfica contra los bytes crudos capturados.
    assert_eq!(
        *signature,
        nv_engine::http_client::sign_hmac_sha256(SECRET, body)
    );
    let payload: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(
        payload["entry"][0]["changes"][0]["value"]["messages"][0]["text"]["body"],
        "4"
    );
}

// (c) GET transcript contiene el evento `user_text` tras el POST.
#[tokio::test]
async fn api_transcript_contiene_el_evento_user_text() {
    let sender = Arc::new(CapturingSender::ok());
    let (_state, app) = fixture(sender);
    let conversation_id = create_conversation(app.clone()).await;
    post_json(
        app.clone(),
        &format!("/api/conversations/{conversation_id}/messages"),
        r#"{"kind":"text","body":"4"}"#,
    )
    .await;

    let (status, json) = get_json(
        app,
        &format!("/api/conversations/{conversation_id}/transcript"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["conversation_id"], conversation_id);
    let events = json["events"].as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["direction"], "from_user");
    assert_eq!(events[0]["kind"], "user_text");
    assert_eq!(events[0]["text"], "4");
    assert_eq!(events[0]["seq"], 1);
}

#[tokio::test]
async fn api_transcript_conversacion_desconocida_404() {
    let (_state, app) = fixture(Arc::new(CapturingSender::ok()));
    let (status, json) = get_json(app, "/api/conversations/conv_0000000000000000/transcript").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"], "conversation_not_found");
}

#[tokio::test]
async fn api_post_message_conversacion_desconocida_404_sin_post() {
    let sender = Arc::new(CapturingSender::ok());
    let (_state, app) = fixture(Arc::clone(&sender));
    let (status, _json) = post_json(
        app,
        "/api/conversations/conv_0000000000000000/messages",
        r#"{"kind":"text","body":"4"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(sender.calls.lock().unwrap().is_empty());
}

// Target no-2xx → 502 {"error":"webhook_target","status":N} y el transcript
// queda [user(seq 1), system_note(seq 2)] (§C-1/§T9, orden causal FIX-2).
#[tokio::test]
async fn api_post_message_target_500_responde_502() {
    let sender = Arc::new(CapturingSender::failing(500));
    let (state, app) = fixture(sender);
    let conversation_id = create_conversation(app.clone()).await;

    let (status, json) = post_json(
        app,
        &format!("/api/conversations/{conversation_id}/messages"),
        r#"{"kind":"text","body":"4"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(json["error"], "webhook_target");
    assert_eq!(json["status"], 500);
    // Mock honesto: el evento del usuario quedó Y la nota es un evento
    // system posterior (antes era raw.error sobre el evento del usuario).
    let snapshot = state.registry.snapshot(&conversation_id).unwrap();
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot[0].kind, EventKind::UserText);
    assert_eq!(snapshot[0].seq, 1);
    assert_eq!(snapshot[1].direction, Direction::System);
    assert_eq!(snapshot[1].kind, EventKind::SystemNote);
    assert_eq!(snapshot[1].seq, 2);
    assert!(
        snapshot[1]
            .text
            .as_deref()
            .is_some_and(|t| t.contains("500"))
    );
}

// (d) faults: POST {"status":429} → GET lo refleja → DELETE → off.
#[tokio::test]
async fn api_faults_post_get_delete_roundtrip() {
    let (_state, app) = fixture(Arc::new(CapturingSender::ok()));

    // Off inicial: GET devuelve la política default (sin campos).
    let (status, json) = get_json(app.clone(), "/api/faults").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, serde_json::json!({}));

    let (status, json) = post_json(app.clone(), "/api/faults", r#"{"status":429}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, serde_json::json!({"ok": true}));

    let (status, json) = get_json(app.clone(), "/api/faults").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, serde_json::json!({"status": 429}));

    let request = Request::delete("/api/faults").body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(), serde_json::json!({"ok": true}));

    let (status, json) = get_json(app, "/api/faults").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, serde_json::json!({}), "DELETE deja la política off");
}

// La fault seteada vía API se aplica en la ruta fake-Meta (wiring §C-4).
#[tokio::test]
async fn api_fault_seteada_se_aplica_en_fake_meta() {
    let (_state, app) = fixture(Arc::new(CapturingSender::ok()));
    post_json(app.clone(), "/api/faults", r#"{"status":500}"#).await;

    let request = Request::post("/graph/v22.0/NV_PHONE_ID/messages")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"messaging_product":"whatsapp","to":"56900000001","type":"text","text":{"body":"hola"}}"#))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["type"], "NVInjected");
}

// (e) SSE resync: overflow del broadcast (300 > cap 256) con receiver vivo
// → el stream emite `event: resync` y SIGUE con el delta retenido (§C-5).
// El mapeo puro se verifica abajo a nivel SseMessage; acá se prueba la
// forma wire sobre el stream HTTP real (sin oneshot, que consumiría el
// body infinito).
#[tokio::test]
async fn sse_lagged_emite_resync_y_sigue_el_tail() {
    let (state, app) = fixture(Arc::new(CapturingSender::ok()));
    let conversation_id = create_conversation(app.clone()).await;

    let mut service = app.into_service();
    let request = Request::get(format!("/api/conversations/{conversation_id}/events"))
        .body(Body::empty())
        .unwrap();
    let response = service.call(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Suscripción viva: 300 appends sin leer → Lagged (300-256=44 perdidos).
    for _ in 0..300 {
        state
            .registry
            .append_event(&conversation_id, ChatEvent::new(Direction::FromBot, EventKind::Text))
            .unwrap();
    }

    let mut body = response.into_body();
    let chunk = tokio::time::timeout(Duration::from_secs(2), async {
        let mut data = Vec::new();
        // El resync va primero; el tail retomado llega en frames posteriores.
        while !String::from_utf8_lossy(&data).contains(r#""seq":45"#) {
            let frame = body
                .frame()
                .await
                .expect("stream cerrado antes del resync")
                .expect("error en frame")
                .into_data()
                .expect("frame sin data");
            data.extend_from_slice(&frame);
        }
        data
    })
    .await
    .expect("timeout esperando resync + retoma del tail");

    let text = String::from_utf8(chunk).unwrap();
    assert!(text.contains("event: resync"), "chunk: {text}");
    assert!(text.contains("data: {}"), "chunk: {text}");
    // Tras el resync el tail sigue con el evento retenido más viejo (seq 45).
    assert!(text.contains("event: message"), "chunk: {text}");
    assert!(text.contains(r#""seq":45"#), "chunk: {text}");
}

// Mapeo puro (§T11-e, test del mapeo): Lagged → Resync, Ok → Event.
#[test]
fn sse_lagged_mapea_a_resync() {
    assert_eq!(
        sse::map_broadcast_item(Err(
            tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(44)
        )),
        sse::SseMessage::Resync
    );
}

// Mapeo puro: un ChatEvent Ok → SseMessage::Event (§C-5).
#[test]
fn sse_evento_ok_mapea_a_message() {
    let event = ChatEvent::new(Direction::FromBot, EventKind::Buttons);
    assert_eq!(
        sse::map_broadcast_item(Ok(event.clone())),
        sse::SseMessage::Event(Box::new(event))
    );
}

// Extra: delta en vivo a través del stream HTTP real (sin oneshot, que
// consumiría el body infinito): subscribe → append → llega `event: message`.
#[tokio::test]
async fn sse_stream_http_entrega_delta_en_vivo() {
    let (state, app) = fixture(Arc::new(CapturingSender::ok()));
    let conversation_id = create_conversation(app.clone()).await;

    let mut service = app.into_service();
    let request = Request::get(format!("/api/conversations/{conversation_id}/events"))
        .body(Body::empty())
        .unwrap();
    let response = service.call(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(content_type.starts_with("text/event-stream"), "content-type: {content_type}");

    // La suscripción ya existe (el handler corrió antes de responder):
    // el append se difunde como delta.
    state
        .registry
        .append_event(&conversation_id, {
            let mut ev = ChatEvent::new(Direction::FromBot, EventKind::Text);
            ev.text = Some("hola".to_string());
            ev
        })
        .unwrap();

    let mut body = response.into_body();
    let chunk = tokio::time::timeout(Duration::from_secs(2), async {
        let mut data = Vec::new();
        while !String::from_utf8_lossy(&data).contains("\n\n") {
            let frame = body
                .frame()
                .await
                .expect("stream cerrado antes del delta")
                .expect("error en frame")
                .into_data()
                .expect("frame sin data");
            data.extend_from_slice(&frame);
        }
        data
    })
    .await
    .expect("timeout esperando el delta SSE");

    let text = String::from_utf8(chunk).unwrap();
    assert!(text.contains("event: message"), "chunk: {text}");
    assert!(text.contains(r#""seq":1"#), "chunk: {text}");
    assert!(text.contains("hola"), "chunk: {text}");
}

// SSE de conversación desconocida → 404 antes de abrir el stream.
#[tokio::test]
async fn sse_conversacion_desconocida_404() {
    let (_state, app) = fixture(Arc::new(CapturingSender::ok()));
    let (status, json) = get_json(app, "/api/conversations/conv_0000000000000000/events").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"], "conversation_not_found");
}

// (f) health → {"status":"ok"}.
#[tokio::test]
async fn api_health_ok() {
    let (_state, app) = fixture(Arc::new(CapturingSender::ok()));
    let (status, json) = get_json(app, "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, serde_json::json!({"status": "ok"}));
}
