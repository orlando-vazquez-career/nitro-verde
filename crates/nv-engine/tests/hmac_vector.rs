//! Test de integración T6: firma HMAC sobre BYTES CRUDOS, verificada del lado
//! del receptor con una implementación hmac+sha2 independiente (riesgo ADR #2).

use hmac::{Hmac, KeyInit, Mac};
use nv_core::ports::WebhookSender;
use nv_engine::http_client::{ReqwestWebhookSender, sign_hmac_sha256};
use sha2::Sha256;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SECRET: &str = "nv-dev-secret";

/// Vector conocido OBLIGATORIO (§T6), duplicado a nivel de integración para
/// que el contrato quede fijado también desde fuera del módulo.
#[test]
fn hmac_vector_conocido_integracion() {
    let body = br#"{"object":"whatsapp_business_account"}"#;
    assert_eq!(
        sign_hmac_sha256(SECRET, body),
        "sha256=c42e43d56facd7e5489b0f2f6eaae56e6c2711f7812d33a186e804d04f9fae2c"
    );
}

/// POST capturado por wiremock: el body recibido es byte-a-byte el enviado y
/// el header `X-Hub-Signature-256` re-verifica sobre ESOS bytes crudos con un
/// HMAC-SHA256 calculado acá, independiente de `sign_hmac_sha256`.
#[tokio::test]
async fn hmac_verificado_sobre_bytes_crudos_recibidos() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/whatsapp/webhook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    // El caller serializa UNA vez; estos son los bytes que se firman y envían.
    let body: Vec<u8> =
        serde_json::to_vec(&serde_json::json!({"object":"whatsapp_business_account"})).unwrap();
    let signature = sign_hmac_sha256(SECRET, &body);

    let sender = ReqwestWebhookSender::new();
    let url = format!("{}/api/v1/whatsapp/webhook", server.uri());
    sender.send(&url, body.clone(), &signature).await.unwrap();

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let request = &received[0];

    // 1) El receptor recibió EXACTAMENTE los bytes enviados (sin roundtrip).
    assert_eq!(request.body, body);

    // 2) Re-verificación independiente: HMAC-SHA256 sobre los bytes crudos
    //    recibidos tiene que matchear el header capturado.
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(SECRET.as_bytes()).expect("clave HMAC válida");
    mac.update(&request.body);
    let expected = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    let header = request
        .headers
        .get("X-Hub-Signature-256")
        .expect("falta header X-Hub-Signature-256");
    assert_eq!(header.to_str().unwrap(), expected);

    // 3) Content-Type JSON (lo espera el webhook de akiveo-api, §C-2).
    let content_type = request
        .headers
        .get("Content-Type")
        .expect("falta header Content-Type");
    assert_eq!(content_type.to_str().unwrap(), "application/json");
}

/// Target no-2xx → `NvError::WebhookTarget { status }` (§C-1: la API lo
/// traduce a `502 {"error":"webhook_target","status":N}`).
#[tokio::test]
async fn hmac_target_no_2xx_mapea_webhook_target() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let sender = ReqwestWebhookSender::new();
    let body = b"{}".to_vec();
    let signature = sign_hmac_sha256(SECRET, &body);
    let err = sender.send(&server.uri(), body, &signature).await.unwrap_err();
    match err {
        nv_core::error::NvError::WebhookTarget { status } => assert_eq!(status, 500),
        other => panic!("esperaba WebhookTarget, vino {other:?}"),
    }
}
