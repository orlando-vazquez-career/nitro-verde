//! Tests de la ruta fake-Meta (T10) vía `Router::oneshot` (tower ServiceExt,
//! sin levantar red — patrón del ADR §Testing nivel 3).
//!
//! Cubren el contrato §C-3/§C-4 con la FaultPolicy aplicada ANTES de parsear:
//! (a) text → 200 + `wamid.NV_`; (b) mark_as_read → `{"success":true}`;
//! (c) fault `{"status":500}` → 500 `NVInjected`; (d) basura → 400
//! `NVParseError`; (e) `to` nuevo → auto-creación con 1 evento `from_bot`.
//! Extra: fault `{"delay_ms"}` → responde normal tras el sleep (el guard del
//! lock se suelta antes del `.await`, riesgo ADR #4).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use nv_core::conversation::Direction;
use nv_http::routes::fake_meta;
use nv_http::state::AppState;
use tower::ServiceExt;

/// Lee un fixture golden de nv-core (T3) vía `CARGO_MANIFEST_DIR`.
fn fixture(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../nv-core/fixtures/");
    std::fs::read(format!("{path}{name}")).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Router de la ruta fake-Meta con el estado de test aplicado.
fn app(state: &AppState) -> Router {
    fake_meta::router().with_state(state.clone())
}

/// POST al endpoint fake-Meta (version/phone_number_id opacos, §C-3) y
/// parseo de la respuesta como JSON.
async fn post_messages(app: Router, body: Vec<u8>) -> (StatusCode, serde_json::Value) {
    let request = Request::post("/graph/v22.0/NV_PHONE_ID/messages")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("body no es JSON ({e}): {}", String::from_utf8_lossy(&bytes)));
    (status, json)
}

/// Setea la FaultPolicy activa desde un body §C-4 (guard corto, test sync).
fn set_fault(state: &AppState, policy_json: &str) {
    *state.fault.write().expect("fault lock poisoned") =
        serde_json::from_str(policy_json).expect("policy json");
}

/// POST al endpoint fake-Meta con un Content-Type arbitrario (o ausente) y
/// parseo de la respuesta como JSON. Para los tests del guard anti-CSRF.
async fn post_messages_with_content_type(
    app: Router,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, serde_json::Value) {
    let builder = Request::post("/graph/v22.0/NV_PHONE_ID/messages");
    let builder = match content_type {
        Some(ct) => builder.header("content-type", ct),
        None => builder,
    };
    let request = builder.body(Body::from(body)).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("body no es JSON ({e}): {}", String::from_utf8_lossy(&bytes)));
    (status, json)
}

#[tokio::test]
async fn fake_meta_route_text_200_con_wamid() {
    let state = AppState::new("NV_PHONE_ID", "http://127.0.0.1:9100");
    let (status, json) = post_messages(app(&state), fixture("outbound_text.json")).await;

    assert_eq!(status, StatusCode::OK);
    let wamid = json["messages"][0]["id"].as_str().expect("messages[0].id");
    assert!(wamid.starts_with("wamid.NV_"), "wamid: {wamid}");
    assert_eq!(wamid.len(), "wamid.NV_".len() + 16, "wamid: {wamid}");
    // Eco del `to` del fixture (§C-3).
    assert_eq!(json["messaging_product"], "whatsapp");
    assert_eq!(json["contacts"][0]["input"], "56900000001");
    assert_eq!(json["contacts"][0]["wa_id"], "56900000001");
}

#[tokio::test]
async fn fake_meta_route_mark_as_read_success_true() {
    let state = AppState::new("NV_PHONE_ID", "http://127.0.0.1:9100");
    let (status, json) =
        post_messages(app(&state), fixture("outbound_mark_as_read.json")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, serde_json::json!({"success":true}));
    // Sin `to` en el wire: no se crea conversación (no es atribuible).
    assert_eq!(state.registry.find_by_phone("56900000001"), None);
}

#[tokio::test]
async fn fake_meta_route_fault_500_responde_nvinjected() {
    let state = AppState::new("NV_PHONE_ID", "http://127.0.0.1:9100");
    set_fault(&state, r#"{"status":500}"#);
    let (status, json) = post_messages(app(&state), fixture("outbound_text.json")).await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(json["error"]["type"], "NVInjected");
    assert_eq!(json["error"]["message"], "NV fault injection");
    assert_eq!(json["error"]["code"], 500);
    // El fallo se decide ANTES de parsear: no quedó evento en transcript.
    assert_eq!(state.registry.find_by_phone("56900000001"), None);
}

#[tokio::test]
async fn fake_meta_route_basura_400_nvparseerror() {
    let state = AppState::new("NV_PHONE_ID", "http://127.0.0.1:9100");
    let (status, json) = post_messages(app(&state), b"{not json".to_vec()).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["type"], "NVParseError");
    assert_eq!(json["error"]["code"], 400);
    assert!(json["error"]["message"].as_str().is_some());
}

#[tokio::test]
async fn fake_meta_route_to_nuevo_autocrea_conversacion() {
    let state = AppState::new("NV_PHONE_ID", "http://127.0.0.1:9100");
    let (status, _) = post_messages(app(&state), fixture("outbound_text.json")).await;
    assert_eq!(status, StatusCode::OK);

    // §C-3: outbound para un `to` sin conversación → auto-creación al vuelo.
    let conversation_id = state
        .registry
        .find_by_phone("56900000001")
        .expect("conversación auto-creada para el `to`");
    let snapshot = state.registry.snapshot(&conversation_id).unwrap();
    assert_eq!(snapshot.len(), 1, "1 solo evento en el transcript");
    assert_eq!(snapshot[0].direction, Direction::FromBot);
    assert_eq!(snapshot[0].seq, 1);
    assert!(
        snapshot[0]
            .wamid
            .as_deref()
            .is_some_and(|w| w.starts_with("wamid.NV_"))
    );
}

#[tokio::test]
async fn fake_meta_route_fault_delay_responde_normal_tras_sleep() {
    let state = AppState::new("NV_PHONE_ID", "http://127.0.0.1:9100");
    set_fault(&state, r#"{"delay_ms":50}"#);
    let (status, json) = post_messages(app(&state), fixture("outbound_text.json")).await;

    // §C-4: delay espera y luego responde NORMAL (decide() corrió una vez).
    assert_eq!(status, StatusCode::OK);
    assert!(
        json["messages"][0]["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("wamid.NV_"))
    );
    // Y el evento quedó persistido tras el sleep.
    let conversation_id = state.registry.find_by_phone("56900000001").unwrap();
    assert_eq!(state.registry.snapshot(&conversation_id).unwrap().len(), 1);
}

// --- Guard anti-CSRF: Content-Type debe ser application/json (FIX-1) ---

#[tokio::test]
async fn fake_meta_route_text_plain_es_415_unsupported_media_type() {
    let state = AppState::new("NV_PHONE_ID", "http://127.0.0.1:9100");
    // Body JSON VÁLIDO pero Content-Type text/plain (POST sin preflight
    // desde un browser): se rechaza ANTES de parsear y de aplicar faults.
    let (status, json) = post_messages_with_content_type(
        app(&state),
        Some("text/plain"),
        fixture("outbound_text.json"),
    )
    .await;

    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(json["error"]["type"], "NVUnsupportedMediaType");
    assert_eq!(
        json["error"]["message"],
        "Content-Type must be application/json"
    );
    // No tocó transcript (rechazado antes del parseo).
    assert_eq!(state.registry.find_by_phone("56900000001"), None);

    // Sin header Content-Type directamente: también 415.
    let (status, _) =
        post_messages_with_content_type(app(&state), None, fixture("outbound_text.json")).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn fake_meta_route_json_con_charset_pasa_como_antes() {
    let state = AppState::new("NV_PHONE_ID", "http://127.0.0.1:9100");
    let (status, json) = post_messages_with_content_type(
        app(&state),
        Some("application/json; charset=utf-8"),
        fixture("outbound_text.json"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        json["messages"][0]["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("wamid.NV_"))
    );
    // Y el `to` sigue auto-creando la conversación (§C-3).
    assert!(state.registry.find_by_phone("56900000001").is_some());
}
