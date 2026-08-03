//! Tests de las rutas de media (T19) vía `Router::oneshot` (tower ServiceExt,
//! sin levantar red — patrón del ADR §Testing nivel 3).
//!
//! Cubren el done_cmd de T19:
//! (a) info endpoint (`GET /graph/{version}/{media_id}`) → url ABSOLUTA
//!     (`public_url` + `/media/{id}`), `file_size` y `sha256` REAL correctos;
//! (b) download endpoint (`GET /media/{media_id}`) → bytes exactos con su
//!     Content-Type;
//! (c) NO-colisión de rutas: `/graph/v22.0/abc123` (2 segmentos) cae en la
//!     info de media y `/graph/v22.0/123/messages` (3 segmentos) sigue cayendo
//!     en fake_meta;
//! (d) multipart `POST /api/conversations/{id}/media` end-to-end: 200 con
//!     `wamid` + `media_id`, evento `image` en el transcript y webhook
//!     `type:"image"` Meta-real capturado por el sender fake;
//! (e) 400 sin `file` / `kind` inválido, 404 de conversación desconocida.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use nv_core::conversation::{Direction, EventKind};
use nv_core::error::NvError;
use nv_core::ports::{BoxFuture, WebhookSender};
use nv_engine::orchestrator::Orchestrator;
use nv_http::routes;
use nv_http::state::AppState;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const PHONE: &str = "56900000001";
const PUBLIC_URL: &str = "http://host.docker.internal:9100";
/// sha256("abc") — vector conocido, regenerable con:
/// `python -c "import hashlib; print(hashlib.sha256(b'abc').hexdigest())"`
const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

/// Sender fake: captura (url, bytes, firma) sin red (mismo patrón que
/// `api_sse.rs`); el orchestrator siempre ve un target 200.
struct CapturingSender {
    calls: Mutex<Vec<(String, Vec<u8>, String)>>,
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
            Ok(())
        })
    }
}

/// Fixture: router completo ensamblado (composition root real) con sender
/// capturante. Devuelve también el sender para inspeccionar el webhook.
fn fixture() -> (AppState, Arc<CapturingSender>, Router) {
    let state = AppState::new("NV_PHONE_ID", PUBLIC_URL);
    let sender = Arc::new(CapturingSender {
        calls: Mutex::new(vec![]),
    });
    let orchestrator = Arc::new(Orchestrator::new(
        Arc::clone(&sender) as Arc<dyn WebhookSender>,
        Arc::clone(&state.registry),
        "http://localhost:8002/api/v1/whatsapp/webhook",
        "nv-dev-secret",
        "NV_PHONE_ID",
    ));
    let app = routes::router(state.clone(), orchestrator, CancellationToken::new());
    (state, sender, app)
}

/// Crea una conversación vía la API real y devuelve su id.
async fn create_conversation(app: &Router) -> String {
    let request = Request::post("/api/conversations")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"phone":"56900000001"}"#))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["conversation_id"].as_str().unwrap().to_string()
}

/// GET simple y lectura del body crudo.
async fn get(app: Router, path: &str) -> (StatusCode, String, Vec<u8>) {
    let request = Request::get(path).body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, bytes.to_vec())
}

/// Body multipart armado a mano (sin reqwest en nv-http): campos `kind`,
/// `caption` opcional y `file` con filename + content-type.
fn multipart_body(
    boundary: &str,
    kind: &str,
    caption: Option<&str>,
    filename: &str,
    content_type: &str,
    file_bytes: &[u8],
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"kind\"\r\n\r\n{kind}\r\n")
            .as_bytes(),
    );
    if let Some(caption) = caption {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"caption\"\r\n\r\n{caption}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file_bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// POST multipart al endpoint de media y parseo de la respuesta JSON.
async fn post_media(
    app: Router,
    conv_id: &str,
    body: Vec<u8>,
) -> (StatusCode, serde_json::Value) {
    let request = Request::post(format!("/api/conversations/{conv_id}/media"))
        .header(
            "content-type",
            "multipart/form-data; boundary=NvBoundaryT19",
        )
        .body(Body::from(body))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("body no es JSON ({e}): {}", String::from_utf8_lossy(&bytes)));
    (status, json)
}

/// (a) info endpoint: url absoluta desde `public_url`, file_size y sha256
/// REAL de los bytes guardados.
#[tokio::test]
async fn info_endpoint_devuelve_url_absoluta_file_size_y_sha256_real() {
    let (state, _sender, app) = fixture();
    let media_id = state.media.put(b"abc".to_vec(), "image/jpeg");

    let (status, content_type, bytes) = get(app, &format!("/graph/v22.0/{media_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("application/json"), "ct: {content_type}");
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["messaging_product"], "whatsapp");
    // url ABSOLUTA: public_url + /media/{id} (el target la usa tal cual).
    assert_eq!(
        json["url"],
        format!("{PUBLIC_URL}/media/{media_id}").as_str()
    );
    assert_eq!(json["mime_type"], "image/jpeg");
    assert_eq!(json["sha256"], SHA256_ABC, "sha256 real de b\"abc\"");
    assert_eq!(json["file_size"], 3);
}

/// (b) download endpoint: los BYTES exactos con el Content-Type guardado.
#[tokio::test]
async fn download_endpoint_devuelve_bytes_exactos_y_content_type() {
    let (state, _sender, app) = fixture();
    // Bytes binarios no-UTF8 a propósito: viajan crudos, sin interpretación.
    let payload: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0x00, 0x61, 0x62];
    let media_id = state.media.put(payload.clone(), "image/jpeg");

    let (status, content_type, bytes) = get(app, &format!("/media/{media_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "image/jpeg");
    assert_eq!(bytes, payload, "bytes exactos, sin transformación");
}

/// (c) NO-colisión: 2 segmentos tras /graph → info de media; 3 segmentos →
/// fake_meta. Ambos en el MISMO router ensamblado.
#[tokio::test]
async fn rutas_graph_no_colisionan_por_aridad() {
    let (state, _sender, app) = fixture();
    let media_id = state.media.put(b"abc".to_vec(), "application/pdf");

    // 2 segmentos, id conocido → 200 info de media.
    let (status, _, _) = get(app.clone(), &format!("/graph/v22.0/{media_id}")).await;
    assert_eq!(status, StatusCode::OK);

    // 2 segmentos, id desconocido → 404 del handler de MEDIA (NVMediaNotFound),
    // no de otro router: prueba que la ruta matcheó acá.
    let (status, _, bytes) = get(app.clone(), "/graph/v22.0/media_0000000000000000").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["type"], "NVMediaNotFound");

    // 3 segmentos → fake_meta sigue funcionando igual (POST outbound §C-3).
    let outbound = r#"{"messaging_product":"whatsapp","to":"56900000001","type":"text","text":{"body":"hola"}}"#;
    let request = Request::post("/graph/v22.0/NV_PHONE_ID/messages")
        .header("content-type", "application/json")
        .body(Body::from(outbound))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["messages"][0]["id"].as_str().unwrap().starts_with("wamid.NV_"));
}

/// (d) multipart end-to-end: 200 con wamid + media_id, evento `image` en el
/// transcript y webhook `type:"image"` Meta-real con el sha256 real.
#[tokio::test]
async fn post_media_image_end_to_end() {
    let (state, sender, app) = fixture();
    let conv_id = create_conversation(&app).await;

    let body = multipart_body(
        "NvBoundaryT19",
        "image",
        Some("mi receta"),
        "receta.jpg",
        "image/jpeg",
        b"abc",
    );
    let (status, json) = post_media(app, &conv_id, body).await;

    assert_eq!(status, StatusCode::OK);
    let wamid = json["wamid"].as_str().expect("wamid");
    assert!(wamid.starts_with("wamid.NV_"), "wamid: {wamid}");
    let media_id = json["media_id"].as_str().expect("media_id").to_string();
    assert!(media_id.starts_with("media_"), "media_id: {media_id}");

    // El store quedó con los bytes y su sha256 real.
    let entry = state.media.get(&media_id).expect("media guardado");
    assert_eq!(entry.bytes, b"abc");
    assert_eq!(entry.sha256, SHA256_ABC);

    // El webhook salió UNA vez, con type:"image" Meta-real (§C-2/T19).
    let calls = sender.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let payload: serde_json::Value = serde_json::from_slice(&calls[0].1).unwrap();
    let msg = &payload["entry"][0]["changes"][0]["value"]["messages"][0];
    assert_eq!(msg["from"], PHONE);
    assert_eq!(msg["type"], "image");
    assert_eq!(msg["image"]["id"], media_id.as_str());
    assert_eq!(msg["image"]["mime_type"], "image/jpeg");
    assert_eq!(msg["image"]["caption"], "mi receta");
    assert_eq!(msg["image"]["sha256"], SHA256_ABC);
    drop(calls);

    // Transcript: evento from_user / image con media_id y caption (§C-5).
    let snap = state.registry.snapshot(&conv_id).unwrap();
    assert_eq!(snap.len(), 1);
    let ev = &snap[0];
    assert_eq!(ev.direction, Direction::FromUser);
    assert_eq!(ev.kind, EventKind::Image);
    assert_eq!(ev.media_id.as_deref(), Some(media_id.as_str()));
    assert_eq!(ev.text.as_deref(), Some("mi receta"));
    assert_eq!(ev.wamid.as_deref(), Some(wamid));
}

/// (d2) document: `filename` viaja al webhook y es el texto visible si no
/// hay caption.
#[tokio::test]
async fn post_media_document_con_filename() {
    let (state, sender, app) = fixture();
    let conv_id = create_conversation(&app).await;

    let body = multipart_body(
        "NvBoundaryT19",
        "document",
        None,
        "receta.pdf",
        "application/pdf",
        b"%PDF-fake",
    );
    let (status, json) = post_media(app, &conv_id, body).await;
    assert_eq!(status, StatusCode::OK);
    let media_id = json["media_id"].as_str().unwrap().to_string();

    let calls = sender.calls.lock().unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&calls[0].1).unwrap();
    let msg = &payload["entry"][0]["changes"][0]["value"]["messages"][0];
    assert_eq!(msg["type"], "document");
    assert_eq!(msg["document"]["id"], media_id.as_str());
    assert_eq!(msg["document"]["mime_type"], "application/pdf");
    assert_eq!(msg["document"]["filename"], "receta.pdf");
    drop(calls);

    let snap = state.registry.snapshot(&conv_id).unwrap();
    assert_eq!(snap[0].kind, EventKind::Document);
    assert_eq!(snap[0].text.as_deref(), Some("receta.pdf"));
}

/// (e) validaciones: sin `file` → 400; `kind` inválido → 400; conversación
/// desconocida → 404 (el adjunto queda guardado: mock honesto, el 404 es del
/// orchestrator).
#[tokio::test]
async fn post_media_validaciones_400_y_404() {
    let (_state, sender, app) = fixture();
    let conv_id = create_conversation(&app).await;

    // Sin campo file → 400 missing_file.
    let mut body = Vec::new();
    body.extend_from_slice(
        b"--NvBoundaryT19\r\nContent-Disposition: form-data; name=\"kind\"\r\n\r\nimage\r\n--NvBoundaryT19--\r\n"
            .as_slice(),
    );
    let (status, json) = post_media(app.clone(), &conv_id, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"], "missing_file");

    // kind inválido → 400 invalid_kind.
    let body = multipart_body(
        "NvBoundaryT19",
        "sticker",
        None,
        "receta.jpg",
        "image/jpeg",
        b"abc",
    );
    let (status, json) = post_media(app.clone(), &conv_id, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"], "invalid_kind");

    // Conversación desconocida → 404 conversation_not_found.
    let body = multipart_body(
        "NvBoundaryT19",
        "image",
        None,
        "receta.jpg",
        "image/jpeg",
        b"abc",
    );
    let (status, json) = post_media(app, "conv_0000000000000000", body).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"], "conversation_not_found");

    // Ninguno de los casos de validación disparó webhook.
    assert_eq!(sender.calls.lock().unwrap().len(), 0);
}

/// Regresión (bug reportado en gate humano): una foto de receta de celular
/// (3-8 MB) era rechazada con 400 `multipart_invalid` por el DefaultBodyLimit
/// de axum (2 MB). El router ensamblado ahora admite 25 MB en /api.
#[tokio::test]
async fn adjunto_de_5mb_pasa_el_body_limit() {
    let (_state, sender, app) = fixture();
    let conv_id = create_conversation(&app).await;

    let foto_celular = vec![0xFF; 5 * 1024 * 1024]; // 5 MB binarios
    let body = multipart_body(
        "NvBoundaryT19",
        "image",
        None,
        "receta.jpg",
        "image/jpeg",
        &foto_celular,
    );
    let (status, json) = post_media(app, &conv_id, body).await;

    assert_eq!(status, StatusCode::OK, "5 MB debe pasar: {json}");
    assert_eq!(json["accepted"], true);
    assert!(json["media_id"].as_str().unwrap().starts_with("media_"));
    // El webhook salió con la imagen (el target capturante lo recibió).
    assert_eq!(sender.calls.lock().unwrap().len(), 1);
}
