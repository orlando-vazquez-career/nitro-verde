//! Test de integración T9: orchestrator contra un webhook target REAL
//! (wiremock). La firma `X-Hub-Signature-256` se RE-VERIFICA del lado del
//! receptor sobre los bytes crudos capturados, con un HMAC-SHA256 calculado
//! acá de forma independiente (riesgo ADR #2), y el body se valida contra la
//! estructura §C-2.

use std::sync::Arc;

use hmac::{Hmac, KeyInit, Mac};
use nv_core::conversation::{ConversationId, Direction, EventKind, UserInput};
use nv_core::error::NvError;
use nv_engine::http_client::ReqwestWebhookSender;
use nv_engine::orchestrator::Orchestrator;
use nv_engine::registry::Registry;
use sha2::Sha256;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SECRET: &str = "nv-dev-secret";
const PHONE: &str = "56900000001";
const WEBHOOK_PATH: &str = "/api/v1/whatsapp/webhook";

/// Arma el orchestrator apuntando al `server` y una conversación fresca.
async fn fixture(server: &MockServer) -> (Orchestrator, Arc<Registry>, ConversationId) {
    let registry = Arc::new(Registry::new());
    let conv_id = registry.create(PHONE);
    let orch = Orchestrator::new(
        Arc::new(ReqwestWebhookSender::new()),
        Arc::clone(&registry),
        format!("{}{WEBHOOK_PATH}", server.uri()),
        SECRET,
        "NV_PHONE_ID",
    );
    (orch, registry, conv_id)
}

/// Monta el mock del webhook target respondiendo `status` y esperando
/// exactamente un POST al path de §C-2.
async fn mount_target(server: &MockServer, status: u16) {
    Mock::given(method("POST"))
        .and(path(WEBHOOK_PATH))
        .respond_with(ResponseTemplate::new(status).set_body_json(serde_json::json!({"status":"ok"})))
        .expect(1)
        .mount(server)
        .await;
}

/// Re-verificación INDEPENDIENTE: HMAC-SHA256(secret, body) == header.
fn verify_signature(secret: &str, body: &[u8], header: &str) -> bool {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).expect("clave HMAC válida");
    mac.update(body);
    let expected = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    expected == header
}

/// Extrae `entry[0].changes[0].value` del body capturado (§C-2).
fn change_value(body: &serde_json::Value) -> &serde_json::Value {
    &body["entry"][0]["changes"][0]["value"]
}

/// (a) text: POST firmado al path del webhook, firma re-verificada sobre los
/// bytes crudos capturados y estructura §C-2 con `text.body == "4"`.
#[tokio::test]
async fn orchestrator_text_post_firmado_y_estructura_c2() {
    let server = MockServer::start().await;
    mount_target(&server, 200).await;
    let (orch, registry, conv_id) = fixture(&server).await;

    let wamid = orch
        .send_user_input(&conv_id, UserInput::Text { body: "4".into() })
        .await
        .unwrap();
    assert!(wamid.starts_with("wamid.NV_"), "wamid: {wamid}");

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let request = &received[0];

    // 1) Firma re-verificada sobre los bytes crudos recibidos.
    let header = request
        .headers
        .get("X-Hub-Signature-256")
        .expect("falta header X-Hub-Signature-256");
    assert!(
        verify_signature(SECRET, &request.body, header.to_str().unwrap()),
        "la firma no cubre los bytes crudos recibidos"
    );

    // 2) Estructura §C-2 completa.
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["object"], "whatsapp_business_account");
    let value = change_value(&body);
    assert_eq!(value["messaging_product"], "whatsapp");
    assert_eq!(value["metadata"]["display_phone_number"], "56900000000");
    assert_eq!(value["metadata"]["phone_number_id"], "NV_PHONE_ID");
    assert_eq!(value["contacts"][0]["profile"]["name"], "NV User");
    assert_eq!(value["contacts"][0]["wa_id"], PHONE);
    let msg = &value["messages"][0];
    assert_eq!(msg["from"], PHONE, "from sin +");
    assert_eq!(msg["id"], wamid);
    assert_eq!(msg["type"], "text");
    assert_eq!(msg["text"]["body"], "4");
    assert!(msg["timestamp"].as_str().unwrap().parse::<u64>().is_ok());

    // 3) El evento del usuario quedó en el transcript (§C-5).
    let snap = registry.snapshot(&conv_id).unwrap();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].direction, Direction::FromUser);
    assert_eq!(snap[0].kind, EventKind::UserText);
    assert_eq!(snap[0].wamid.as_deref(), Some(wamid.as_str()));
}

/// (b) button_reply: `interactive.button_reply.id == "motivo_precio"`.
#[tokio::test]
async fn orchestrator_button_reply_interactive_c2() {
    let server = MockServer::start().await;
    mount_target(&server, 200).await;
    let (orch, registry, conv_id) = fixture(&server).await;

    orch.send_user_input(
        &conv_id,
        UserInput::ButtonReply {
            id: "motivo_precio".into(),
            title: "Me pareció caro".into(),
        },
    )
    .await
    .unwrap();

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    let msg = &change_value(&body)["messages"][0];
    assert_eq!(msg["type"], "interactive");
    assert_eq!(msg["interactive"]["type"], "button_reply");
    assert_eq!(msg["interactive"]["button_reply"]["id"], "motivo_precio");
    assert_eq!(
        msg["interactive"]["button_reply"]["title"],
        "Me pareció caro"
    );

    let snap = registry.snapshot(&conv_id).unwrap();
    assert_eq!(snap[0].kind, EventKind::UserButtonReply);
    assert_eq!(snap[0].text.as_deref(), Some("Me pareció caro"));
}

/// (c) list_reply: `interactive.list_reply` con id/title/description (§C-2).
#[tokio::test]
async fn orchestrator_list_reply_interactive_c2() {
    let server = MockServer::start().await;
    mount_target(&server, 200).await;
    let (orch, registry, conv_id) = fixture(&server).await;

    orch.send_user_input(
        &conv_id,
        UserInput::ListReply {
            id: "lista_x".into(),
            title: "Opción X".into(),
        },
    )
    .await
    .unwrap();

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    let msg = &change_value(&body)["messages"][0];
    assert_eq!(msg["type"], "interactive");
    assert_eq!(msg["interactive"]["type"], "list_reply");
    assert_eq!(msg["interactive"]["list_reply"]["id"], "lista_x");
    assert_eq!(msg["interactive"]["list_reply"]["title"], "Opción X");
    assert_eq!(msg["interactive"]["list_reply"]["description"], "");

    let snap = registry.snapshot(&conv_id).unwrap();
    assert_eq!(snap[0].kind, EventKind::UserListReply);
    assert_eq!(snap[0].text.as_deref(), Some("Opción X"));
}

/// (d) target 500 → `Err(WebhookTarget { status: 500 })`, el evento del
/// usuario queda registrado PRIMERO (orden causal) y la nota del error es un
/// segundo evento `system`/`system_note` (§T9).
#[tokio::test]
async fn orchestrator_target_500_webhook_target_y_evento_registrado() {
    let server = MockServer::start().await;
    mount_target(&server, 500).await;
    let (orch, registry, conv_id) = fixture(&server).await;

    let err = orch
        .send_user_input(&conv_id, UserInput::Text { body: "4".into() })
        .await
        .unwrap_err();
    match err {
        NvError::WebhookTarget { status } => assert_eq!(status, 500),
        other => panic!("esperaba WebhookTarget, vino {other:?}"),
    }

    // El POST igual salió (firmado): el mock lo recibió.
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);

    // Transcript en orden causal: [user(seq 1), system_note(seq 2)].
    let snap = registry.snapshot(&conv_id).unwrap();
    assert_eq!(snap.len(), 2);
    assert_eq!(snap[0].seq, 1);
    assert_eq!(snap[0].direction, Direction::FromUser);
    assert_eq!(snap[0].kind, EventKind::UserText);
    assert_eq!(snap[1].seq, 2);
    assert_eq!(snap[1].direction, Direction::System);
    assert_eq!(snap[1].kind, EventKind::SystemNote);
    let nota = snap[1].text.as_deref().expect("nota de error");
    assert!(nota.contains("500"), "nota: {nota}");
}

/// (e) image (T19): POST firmado con `type:"image"` Meta-real, firma
/// RE-VERIFICADA sobre los bytes crudos capturados y `image.id == media_id`.
#[tokio::test]
async fn orchestrator_image_post_firmado_y_forma_meta_real() {
    let server = MockServer::start().await;
    mount_target(&server, 200).await;
    let (orch, registry, conv_id) = fixture(&server).await;

    let wamid = orch
        .send_user_input(
            &conv_id,
            UserInput::Image {
                media_id: "media_0123456789abcdef".into(),
                mime_type: "image/jpeg".into(),
                caption: Some("mi receta".into()),
                sha256: Some(
                    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
                ),
            },
        )
        .await
        .unwrap();
    assert!(wamid.starts_with("wamid.NV_"), "wamid: {wamid}");

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let request = &received[0];

    // 1) Firma re-verificada sobre los bytes crudos recibidos (mismo
    //    criterio que el test de texto, riesgo ADR #2).
    let header = request
        .headers
        .get("X-Hub-Signature-256")
        .expect("falta header X-Hub-Signature-256");
    assert!(
        verify_signature(SECRET, &request.body, header.to_str().unwrap()),
        "la firma no cubre los bytes crudos recibidos"
    );

    // 2) Forma Meta-real del mensaje de imagen (flujo OCR de akiveo-api).
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    let msg = &change_value(&body)["messages"][0];
    assert_eq!(msg["from"], PHONE, "from sin +");
    assert_eq!(msg["id"], wamid);
    assert_eq!(msg["type"], "image");
    assert_eq!(msg["image"]["id"], "media_0123456789abcdef");
    assert_eq!(msg["image"]["mime_type"], "image/jpeg");
    assert_eq!(msg["image"]["caption"], "mi receta");
    assert_eq!(
        msg["image"]["sha256"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );

    // 3) Transcript: evento image con media_id (§C-5).
    let snap = registry.snapshot(&conv_id).unwrap();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].kind, EventKind::Image);
    assert_eq!(snap[0].media_id.as_deref(), Some("media_0123456789abcdef"));
}

/// (f) document (T19): `type:"document"` con `filename`, firma re-verificada.
#[tokio::test]
async fn orchestrator_document_post_firmado_con_filename() {
    let server = MockServer::start().await;
    mount_target(&server, 200).await;
    let (orch, _registry, conv_id) = fixture(&server).await;

    orch.send_user_input(
        &conv_id,
        UserInput::Document {
            media_id: "media_fedcba9876543210".into(),
            mime_type: "application/pdf".into(),
            caption: None,
            filename: "receta.pdf".into(),
            sha256: Some("cafe".into()),
        },
    )
    .await
    .unwrap();

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let request = &received[0];
    let header = request
        .headers
        .get("X-Hub-Signature-256")
        .expect("falta header X-Hub-Signature-256");
    assert!(
        verify_signature(SECRET, &request.body, header.to_str().unwrap()),
        "la firma no cubre los bytes crudos recibidos"
    );

    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    let msg = &change_value(&body)["messages"][0];
    assert_eq!(msg["type"], "document");
    assert_eq!(msg["document"]["id"], "media_fedcba9876543210");
    assert_eq!(msg["document"]["mime_type"], "application/pdf");
    assert_eq!(msg["document"]["filename"], "receta.pdf");
    assert_eq!(msg["document"]["caption"], serde_json::Value::Null);
}
