//! Cliente HTTP del webhook saliente (NitroVerde → bot, §C-2) + firma HMAC.
//!
//! Riesgo ADR #2 (crítico): la firma HMAC-SHA256 se calcula sobre los BYTES
//! EXACTOS que se envían. El caller (orchestrator, T9) serializa UNA vez a
//! `Vec<u8>`, firma ESOS bytes con [`sign_hmac_sha256`] y envía ESOS bytes con
//! [`ReqwestWebhookSender`]. Está PROHIBIDO reserializar el payload aquí
//! dentro: este módulo nunca hace string-roundtrip del body.
//!
//! ÚNICO uso de reqwest del workspace (el grafo hexagonal lo enforza:
//! `nv-http` no conoce reqwest).

use std::time::Duration;

use hmac::{Hmac, KeyInit, Mac};
use nv_core::error::NvError;
use nv_core::ports::{BoxFuture, WebhookSender};
use sha2::Sha256;

/// HMAC-SHA256 tal como lo espera Meta (`X-Hub-Signature-256`).
type HmacSha256 = Hmac<Sha256>;

/// Timeout del POST al webhook target. Sin él, un target colgado dejaría el
/// envío (y el request de la UI que lo dispara) esperando forever. NO hay
/// retries: NV es emisor de webhooks, el retry lo prueba del lado akiveo.
const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// Firma `body` (bytes crudos) con `secret` y retorna el valor del header
/// `X-Hub-Signature-256` en formato `"sha256=<hex>"`.
///
/// `body` son los bytes EXACTOS que se van a enviar; esta función no los
/// interpreta ni transforma (prohibido parsear/re-serializar JSON acá).
pub fn sign_hmac_sha256(secret: &str, body: &[u8]) -> String {
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(secret.as_bytes())
        .expect("HMAC-SHA256 acepta claves de cualquier longitud");
    mac.update(body);
    let tag = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(tag))
}

/// Implementación reqwest del puerto [`WebhookSender`].
///
/// Envía los bytes recibidos tal cual (`body: Vec<u8>`, sin tocarlos) con el
/// header `X-Hub-Signature-256: <signature>` ya calculado por el caller.
#[derive(Debug, Clone)]
pub struct ReqwestWebhookSender {
    client: reqwest::Client,
}

impl ReqwestWebhookSender {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(WEBHOOK_TIMEOUT)
            .build()
            .expect("reqwest client con config estática válida");
        Self { client }
    }
}

impl Default for ReqwestWebhookSender {
    fn default() -> Self {
        Self::new()
    }
}

impl WebhookSender for ReqwestWebhookSender {
    fn send<'a>(
        &'a self,
        url: &'a str,
        body: Vec<u8>,
        signature: &'a str,
    ) -> BoxFuture<'a, Result<(), NvError>> {
        Box::pin(async move {
            let response = self
                .client
                .post(url)
                .header("X-Hub-Signature-256", signature)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|err| {
                    // Error de transporte (DNS, conexión rechazada, timeout):
                    // el target nunca respondió, así que no hay status HTTP.
                    // Convención: `status: 0` (el detalle queda en el log).
                    tracing::warn!(url, error = %err, "webhook target inalcanzable");
                    NvError::WebhookTarget { status: 0 }
                })?;

            let status = response.status();
            if !status.is_success() {
                tracing::warn!(url, status = status.as_u16(), "webhook target respondió no-2xx");
                return Err(NvError::WebhookTarget { status: status.as_u16() });
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vector conocido OBLIGATORIO (§T6). Regenerable con:
    /// `python -c "import hmac,hashlib; print(hmac.new(b'nv-dev-secret', b'{\"object\":\"whatsapp_business_account\"}', hashlib.sha256).hexdigest())"`
    #[test]
    fn hmac_vector_conocido() {
        let body = br#"{"object":"whatsapp_business_account"}"#;
        let signature = sign_hmac_sha256("nv-dev-secret", body);
        assert_eq!(
            signature,
            "sha256=c42e43d56facd7e5489b0f2f6eaae56e6c2711f7812d33a186e804d04f9fae2c"
        );
    }

    /// La firma cubre los bytes tal cual: un solo byte distinto cambia el tag.
    #[test]
    fn hmac_sensible_a_un_byte() {
        let a = sign_hmac_sha256("nv-dev-secret", b"{\"a\":1}");
        let b = sign_hmac_sha256("nv-dev-secret", b"{\"a\":2}");
        assert_ne!(a, b);
    }

    /// Body vacío y binario no-UTF8 también se firman sin roundtrip.
    #[test]
    fn hmac_bytes_arbitrarios() {
        let sig = sign_hmac_sha256("s", b"\x00\xff\xfe");
        assert!(sig.starts_with("sha256="));
        assert_eq!(sig.len(), "sha256=".len() + 64);
    }
}
