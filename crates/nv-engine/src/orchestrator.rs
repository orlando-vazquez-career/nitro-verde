//! Orchestrator (§C-2): input del usuario simulado → payload webhook Meta-fiel
//! → evento `user_*` en el transcript (§C-5) → firma HMAC-SHA256 sobre los
//! BYTES CRUDOS → POST firmado al target (§T9).
//!
//! Orden causal del transcript: el evento del usuario se appendea ANTES del
//! POST al webhook. Si el bot responde inline durante el procesamiento del
//! webhook, su respuesta queda con seq MAYOR que el mensaje que la causó
//! (con el orden viejo — POST y después append — quedaba invertido).
//!
//! Riesgo ADR #2: el payload se serializa UNA sola vez (`serde_json::to_vec`);
//! la firma de [`sign_hmac_sha256`] cubre esos bytes exactos y esos mismos
//! bytes son los que viajan. Acá no hay string-roundtrip del body.
//!
//! Regla de guards (riesgo ADR #4): el registry es sync y sus métodos ya
//! devuelven datos owned (`String` / `ChatEvent`); ningún guard de
//! DashMap/RwLock cruza el `.await` del envío.
//!
//! Si el target responde no-2xx (o es inalcanzable), se appendea un segundo
//! evento `Direction::System` / `EventKind::SystemNote` con la nota del error
//! y se devuelve `NvError::WebhookTarget` (§T9).

use std::sync::Arc;

use nv_core::conversation::{ChatEvent, ConversationId, Direction, EventKind, UserInput};
use nv_core::error::NvResult;
use nv_core::ids;
use nv_core::meta::webhook::{
    Change, ChangeValue, Entry, InteractiveReply, ListReplyRef, MediaRef, Metadata, Profile,
    RawMessage, ReplyRef, TextBody, WebhookContact, WebhookPayload,
};
use nv_core::ports::WebhookSender;

use crate::http_client::sign_hmac_sha256;
use crate::registry::Registry;

/// `object` fijo del payload (§C-2).
const OBJECT_WABA: &str = "whatsapp_business_account";
/// `entry[0].id` del WABA simulado (§C-2).
const WABA_ID: &str = "WABA_NV";
/// `metadata.display_phone_number` del bot simulado (§C-2). Es cosmético:
/// el webhook real lee `messages[0].from`, no este campo.
const DISPLAY_PHONE_NUMBER: &str = "56900000000";
/// `contacts[0].profile.name` del usuario simulado (§C-2).
const CONTACT_NAME: &str = "NV User";

/// Orquesta el camino inbound: UI/API → webhook firmado → transcript.
///
/// Es el ÚNICO lugar del workspace que construye el payload §C-2: los tipos
/// de `nv_core::meta::webhook` SON el contrato wire (sin DTOs espejo).
pub struct Orchestrator {
    /// Emisor del POST firmado (reqwest en prod, fake en tests).
    sender: Arc<dyn WebhookSender>,
    /// Registry de conversaciones: de acá salen el `from` y el transcript.
    registry: Arc<Registry>,
    /// URL del webhook target (`NV_WEBHOOK_URL`, §C-7).
    webhook_url: String,
    /// Secreto HMAC (`NV_APP_SECRET`, §C-7). NV firma SIEMPRE bien (§C-2).
    app_secret: String,
    /// `metadata.phone_number_id` (`NV_PHONE_NUMBER_ID`, §C-7).
    phone_number_id: String,
}

impl Orchestrator {
    pub fn new(
        sender: Arc<dyn WebhookSender>,
        registry: Arc<Registry>,
        webhook_url: impl Into<String>,
        app_secret: impl Into<String>,
        phone_number_id: impl Into<String>,
    ) -> Self {
        Self {
            sender,
            registry,
            webhook_url: webhook_url.into(),
            app_secret: app_secret.into(),
            phone_number_id: phone_number_id.into(),
        }
    }

    /// Envía un input del usuario al webhook target y lo registra en el
    /// transcript. Devuelve el `wamid.NV_*` generado (§C-1: response 202).
    ///
    /// Flujo (§T9, orden causal): phone de la conversación → payload §C-2 →
    /// append del evento `user_*` PRIMERO (así una respuesta inline del bot
    /// durante el POST queda DESPUÉS en el transcript) → `to_vec` UNA vez →
    /// firma sobre esos bytes → POST. Si el target falla, se appendea una
    /// nota `system`/`system_note` con el error y se devuelve
    /// `NvError::WebhookTarget { status }`.
    pub async fn send_user_input(
        &self,
        conv_id: &ConversationId,
        input: UserInput,
    ) -> NvResult<String> {
        // Guards sueltos antes de cualquier await: `phone_of` es sync y
        // devuelve un String owned (riesgo ADR #4).
        let phone = self.registry.phone_of(conv_id)?;
        let wamid = ids::new_wamid();
        let payload = build_webhook_payload(&phone, &wamid, &input, &self.phone_number_id);

        // (1) Evento del usuario PRIMERO (orden causal): si el bot responde
        //     inline mientras el POST está en vuelo, su respuesta entra al
        //     transcript con seq MAYOR que el mensaje que la causó.
        let raw = serde_json::to_value(&payload)
            .map_err(|e| nv_core::error::NvError::Parse(format!("payload §C-2 a Value: {e}")))?;
        let mut event = ChatEvent::new(Direction::FromUser, input.event_kind());
        event.text = Some(input.display_text().to_string());
        event.wamid = Some(wamid.clone());
        event.media_id = input.media_id().map(str::to_string);
        event.raw = Some(raw);
        self.registry.append_event(conv_id, event)?;

        // (2) Riesgo ADR #2: serializar UNA vez, firmar ESOS bytes, enviar
        //     ESOS bytes. La serialización de estos tipos es infalible en la
        //     práctica (strings y vecs), pero se mapea el error en vez de
        //     panickear.
        let bytes = serde_json::to_vec(&payload)
            .map_err(|e| nv_core::error::NvError::Parse(format!("serializando payload §C-2: {e}")))?;
        let signature = sign_hmac_sha256(&self.app_secret, &bytes);

        let send_result = self
            .sender
            .send(&self.webhook_url, bytes, &signature)
            .await;

        // (3) Mock honesto (§T9): si el target falló, la nota del error queda
        //     en el transcript como evento system DESPUÉS del del usuario.
        if let Err(err) = &send_result {
            let mut note = ChatEvent::new(Direction::System, EventKind::SystemNote);
            note.text = Some(format!("webhook target falló: {err}"));
            if let Err(e) = self.registry.append_event(conv_id, note) {
                // La conversación existe (phone_of ya la resolvió); si igual
                // falla no se enmascara el error original del target.
                tracing::error!(error = %e, %conv_id, "append de system_note falló");
            }
        }

        send_result?;
        Ok(wamid)
    }
}

/// Payload §C-2 completo: `entry[0].changes[0].value.messages[0]`.
/// `from` y `wa_id` van SIN `+` (invariante del wire, §C-2).
fn build_webhook_payload(
    phone: &str,
    wamid: &str,
    input: &UserInput,
    phone_number_id: &str,
) -> WebhookPayload {
    // Defensa del invariante: aunque la conversación se haya creado con `+`,
    // en el wire el phone va pelado (el webhook real normaliza contra E.164).
    let from = phone.strip_prefix('+').unwrap_or(phone);
    WebhookPayload {
        object: OBJECT_WABA.to_string(),
        entry: vec![Entry {
            id: WABA_ID.to_string(),
            changes: vec![Change {
                field: "messages".to_string(),
                value: ChangeValue {
                    messaging_product: Some("whatsapp".to_string()),
                    metadata: Some(Metadata {
                        display_phone_number: DISPLAY_PHONE_NUMBER.to_string(),
                        phone_number_id: phone_number_id.to_string(),
                    }),
                    contacts: vec![WebhookContact {
                        profile: Profile {
                            name: CONTACT_NAME.to_string(),
                        },
                        wa_id: from.to_string(),
                    }],
                    messages: vec![user_message(from, wamid, input)],
                },
            }],
        }],
    }
}

/// `messages[0]` según la variante de `UserInput` (§C-2: text / interactive
/// button_reply / interactive list_reply; T19: image / document con el
/// `media_id` que el target resuelve en dos pasos contra el fake-Meta).
/// Timestamp unix en segundos, string.
fn user_message(from: &str, wamid: &str, input: &UserInput) -> RawMessage {
    let base = |kind: &str| RawMessage {
        from: from.to_string(),
        id: wamid.to_string(),
        timestamp: ids::now_timestamp(),
        kind: kind.to_string(),
        text: None,
        interactive: None,
        button: None,
        image: None,
        document: None,
    };
    match input {
        UserInput::Text { body } => RawMessage {
            text: Some(TextBody { body: body.clone() }),
            ..base("text")
        },
        UserInput::ButtonReply { id, title } => RawMessage {
            interactive: Some(InteractiveReply::ButtonReply {
                button_reply: ReplyRef {
                    id: id.clone(),
                    title: title.clone(),
                },
            }),
            ..base("interactive")
        },
        UserInput::ListReply { id, title } => RawMessage {
            interactive: Some(InteractiveReply::ListReply {
                list_reply: ListReplyRef {
                    id: id.clone(),
                    title: title.clone(),
                    // §C-2: list_reply lleva `description` presente (vacía).
                    description: String::new(),
                },
            }),
            ..base("interactive")
        },
        UserInput::Image {
            media_id,
            mime_type,
            caption,
            sha256,
        } => RawMessage {
            image: Some(MediaRef {
                id: media_id.clone(),
                mime_type: Some(mime_type.clone()),
                sha256: sha256.clone(),
                caption: caption.clone(),
                filename: None,
            }),
            ..base("image")
        },
        UserInput::Document {
            media_id,
            mime_type,
            caption,
            filename,
            sha256,
        } => RawMessage {
            document: Some(MediaRef {
                id: media_id.clone(),
                mime_type: Some(mime_type.clone()),
                sha256: sha256.clone(),
                caption: caption.clone(),
                filename: Some(filename.clone()),
            }),
            ..base("document")
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nv_core::conversation::EventKind;
    use nv_core::error::NvError;
    use nv_core::ports::BoxFuture;
    use std::sync::Mutex;

    const SECRET: &str = "nv-dev-secret";
    const PHONE: &str = "56900000001";

    /// Sender fake: captura (url, bytes, firma) y opcionalmente falla con un
    /// status HTTP, sin red. El re-verify criptográfico real está en el test
    /// wiremock (`tests/orchestrator_wiremock.rs`).
    struct FakeSender {
        fail_with: Option<u16>,
        calls: Mutex<Vec<(String, Vec<u8>, String)>>,
    }

    impl FakeSender {
        fn ok() -> Self {
            Self {
                fail_with: None,
                calls: Mutex::new(vec![]),
            }
        }
        fn failing(status: u16) -> Self {
            Self {
                fail_with: Some(status),
                calls: Mutex::new(vec![]),
            }
        }
    }

    impl WebhookSender for FakeSender {
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

    fn fixture(sender: Arc<FakeSender>) -> (Orchestrator, Arc<Registry>, ConversationId) {
        let registry = Arc::new(Registry::new());
        let conv_id = registry.create(PHONE);
        let orch = Orchestrator::new(
            sender,
            Arc::clone(&registry),
            "http://localhost:8002/api/v1/whatsapp/webhook",
            SECRET,
            "NV_PHONE_ID",
        );
        (orch, registry, conv_id)
    }

    #[test]
    fn payload_text_forma_exacta_c2() {
        let payload = build_webhook_payload(
            PHONE,
            "wamid.NV_0123456789abcdef",
            &UserInput::Text { body: "4".into() },
            "NV_PHONE_ID",
        );
        let v = serde_json::to_value(&payload).unwrap();
        // Estructura literal de §C-2 (campos semánticos, no string compare).
        assert_eq!(v["object"], "whatsapp_business_account");
        assert_eq!(v["entry"][0]["id"], "WABA_NV");
        assert_eq!(v["entry"][0]["changes"][0]["field"], "messages");
        let value = &v["entry"][0]["changes"][0]["value"];
        assert_eq!(value["messaging_product"], "whatsapp");
        assert_eq!(value["metadata"]["display_phone_number"], "56900000000");
        assert_eq!(value["metadata"]["phone_number_id"], "NV_PHONE_ID");
        assert_eq!(value["contacts"][0]["profile"]["name"], "NV User");
        assert_eq!(value["contacts"][0]["wa_id"], PHONE);
        let msg = &value["messages"][0];
        assert_eq!(msg["from"], PHONE);
        assert_eq!(msg["id"], "wamid.NV_0123456789abcdef");
        assert_eq!(msg["type"], "text");
        assert_eq!(msg["text"]["body"], "4");
        assert!(msg["timestamp"].as_str().unwrap().parse::<u64>().is_ok());
    }

    #[test]
    fn payload_button_reply_forma_exacta_c2() {
        let payload = build_webhook_payload(
            PHONE,
            "wamid.NV_0123456789abcdef",
            &UserInput::ButtonReply {
                id: "motivo_precio".into(),
                title: "Me pareció caro".into(),
            },
            "NV_PHONE_ID",
        );
        let v = serde_json::to_value(&payload).unwrap();
        let msg = &v["entry"][0]["changes"][0]["value"]["messages"][0];
        assert_eq!(msg["type"], "interactive");
        assert_eq!(msg["interactive"]["type"], "button_reply");
        assert_eq!(msg["interactive"]["button_reply"]["id"], "motivo_precio");
        assert_eq!(
            msg["interactive"]["button_reply"]["title"],
            "Me pareció caro"
        );
    }

    #[test]
    fn payload_list_reply_forma_exacta_c2() {
        let payload = build_webhook_payload(
            PHONE,
            "wamid.NV_0123456789abcdef",
            &UserInput::ListReply {
                id: "lista_x".into(),
                title: "Opción X".into(),
            },
            "NV_PHONE_ID",
        );
        let v = serde_json::to_value(&payload).unwrap();
        let msg = &v["entry"][0]["changes"][0]["value"]["messages"][0];
        assert_eq!(msg["type"], "interactive");
        assert_eq!(msg["interactive"]["type"], "list_reply");
        assert_eq!(msg["interactive"]["list_reply"]["id"], "lista_x");
        assert_eq!(msg["interactive"]["list_reply"]["title"], "Opción X");
        // §C-2: description presente, vacía.
        assert_eq!(msg["interactive"]["list_reply"]["description"], "");
    }

    #[test]
    fn payload_from_va_sin_plus() {
        let payload = build_webhook_payload(
            "+56900000001",
            "wamid.NV_0123456789abcdef",
            &UserInput::Text { body: "4".into() },
            "NV_PHONE_ID",
        );
        let v = serde_json::to_value(&payload).unwrap();
        let value = &v["entry"][0]["changes"][0]["value"];
        assert_eq!(value["messages"][0]["from"], PHONE);
        assert_eq!(value["contacts"][0]["wa_id"], PHONE);
    }

    #[test]
    fn payload_image_forma_meta_real() {
        // Forma del webhook de imagen que espera el flujo OCR de akiveo-api
        // (T19): `image: {"id","mime_type","sha256","caption"}`.
        let payload = build_webhook_payload(
            PHONE,
            "wamid.NV_0123456789abcdef",
            &UserInput::Image {
                media_id: "media_0123456789abcdef".into(),
                mime_type: "image/jpeg".into(),
                caption: Some("mi receta".into()),
                sha256: Some("deadbeef".into()),
            },
            "NV_PHONE_ID",
        );
        let v = serde_json::to_value(&payload).unwrap();
        let msg = &v["entry"][0]["changes"][0]["value"]["messages"][0];
        assert_eq!(msg["from"], PHONE);
        assert_eq!(msg["type"], "image");
        assert_eq!(msg["image"]["id"], "media_0123456789abcdef");
        assert_eq!(msg["image"]["mime_type"], "image/jpeg");
        assert_eq!(msg["image"]["sha256"], "deadbeef");
        assert_eq!(msg["image"]["caption"], "mi receta");
        // y NO viaja como document ni como texto
        assert_eq!(msg["document"], serde_json::Value::Null);
        assert_eq!(msg["text"], serde_json::Value::Null);
    }

    #[test]
    fn payload_document_forma_meta_real() {
        // Document: igual que image + `filename` (PDF de receta, T19).
        let payload = build_webhook_payload(
            PHONE,
            "wamid.NV_0123456789abcdef",
            &UserInput::Document {
                media_id: "media_fedcba9876543210".into(),
                mime_type: "application/pdf".into(),
                caption: None,
                filename: "receta.pdf".into(),
                sha256: Some("cafe".into()),
            },
            "NV_PHONE_ID",
        );
        let v = serde_json::to_value(&payload).unwrap();
        let msg = &v["entry"][0]["changes"][0]["value"]["messages"][0];
        assert_eq!(msg["type"], "document");
        assert_eq!(msg["document"]["id"], "media_fedcba9876543210");
        assert_eq!(msg["document"]["mime_type"], "application/pdf");
        assert_eq!(msg["document"]["filename"], "receta.pdf");
        assert_eq!(msg["document"]["caption"], serde_json::Value::Null);
        assert_eq!(msg["image"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn send_text_firma_serializa_una_vez_y_registra_evento() {
        let sender = Arc::new(FakeSender::ok());
        let (orch, registry, conv_id) = fixture(Arc::clone(&sender));

        let wamid = orch
            .send_user_input(&conv_id, UserInput::Text { body: "4".into() })
            .await
            .unwrap();
        assert!(wamid.starts_with("wamid.NV_"), "wamid: {wamid}");

        // Un solo POST, firma con formato Meta, bytes = payload §C-2.
        let calls = sender.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (url, body, signature) = &calls[0];
        assert_eq!(url, "http://localhost:8002/api/v1/whatsapp/webhook");
        assert!(signature.starts_with("sha256=") && signature.len() == 7 + 64);
        // La firma cubre los bytes enviados (recomputada con la fn pública).
        assert_eq!(*signature, sign_hmac_sha256(SECRET, body));
        let v: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(
            v["entry"][0]["changes"][0]["value"]["messages"][0]["text"]["body"],
            "4"
        );
        drop(calls);

        // Transcript: evento from_user / user_text con wamid y raw §C-2.
        let snap = registry.snapshot(&conv_id).unwrap();
        assert_eq!(snap.len(), 1);
        let ev = &snap[0];
        assert_eq!(ev.direction, Direction::FromUser);
        assert_eq!(ev.kind, EventKind::UserText);
        assert_eq!(ev.text.as_deref(), Some("4"));
        assert_eq!(ev.wamid.as_deref(), Some(wamid.as_str()));
        let raw = ev.raw.as_ref().expect("raw §C-2 presente");
        assert_eq!(
            raw["entry"][0]["changes"][0]["value"]["messages"][0]["id"],
            wamid
        );
        assert!(raw.get("error").is_none(), "sin nota de error en éxito");
    }

    #[tokio::test]
    async fn send_button_reply_registra_user_button_reply() {
        let sender = Arc::new(FakeSender::ok());
        let (orch, registry, conv_id) = fixture(sender);

        orch.send_user_input(
            &conv_id,
            UserInput::ButtonReply {
                id: "motivo_precio".into(),
                title: "Me pareció caro".into(),
            },
        )
        .await
        .unwrap();

        let snap = registry.snapshot(&conv_id).unwrap();
        assert_eq!(snap[0].kind, EventKind::UserButtonReply);
        assert_eq!(snap[0].text.as_deref(), Some("Me pareció caro"));
    }

    #[tokio::test]
    async fn send_image_registra_evento_con_media_id() {
        let sender = Arc::new(FakeSender::ok());
        let (orch, registry, conv_id) = fixture(sender);

        let wamid = orch
            .send_user_input(
                &conv_id,
                UserInput::Image {
                    media_id: "media_0123456789abcdef".into(),
                    mime_type: "image/jpeg".into(),
                    caption: Some("mi receta".into()),
                    sha256: Some("deadbeef".into()),
                },
            )
            .await
            .unwrap();
        assert!(wamid.starts_with("wamid.NV_"), "wamid: {wamid}");

        // Transcript: evento from_user / image con media_id y caption (§C-5).
        let snap = registry.snapshot(&conv_id).unwrap();
        assert_eq!(snap.len(), 1);
        let ev = &snap[0];
        assert_eq!(ev.direction, Direction::FromUser);
        assert_eq!(ev.kind, EventKind::Image);
        assert_eq!(ev.text.as_deref(), Some("mi receta"));
        assert_eq!(ev.media_id.as_deref(), Some("media_0123456789abcdef"));
        assert_eq!(ev.wamid.as_deref(), Some(wamid.as_str()));
        let raw = ev.raw.as_ref().expect("raw §C-2 presente");
        assert_eq!(
            raw["entry"][0]["changes"][0]["value"]["messages"][0]["image"]["id"],
            "media_0123456789abcdef"
        );
    }

    #[tokio::test]
    async fn target_no_2xx_devuelve_error_y_registra_system_note() {
        let sender = Arc::new(FakeSender::failing(500));
        let (orch, registry, conv_id) = fixture(Arc::clone(&sender));

        let err = orch
            .send_user_input(&conv_id, UserInput::Text { body: "4".into() })
            .await
            .unwrap_err();
        assert!(matches!(err, NvError::WebhookTarget { status: 500 }));

        // §T9 nuevo: el evento del usuario queda SIN raw.error (ya estaba
        // appendeado cuando el POST falló) y la nota es un evento aparte.
        let snap = registry.snapshot(&conv_id).unwrap();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].kind, EventKind::UserText);
        assert!(snap[0].raw.as_ref().unwrap().get("error").is_none());
        assert_eq!(snap[1].direction, Direction::System);
        assert_eq!(snap[1].kind, EventKind::SystemNote);
        let nota = snap[1].text.as_deref().expect("nota de error");
        assert!(nota.contains("500"), "nota: {nota}");
    }

    #[tokio::test]
    async fn target_500_transcript_en_orden_causal_user_antes_que_system_note() {
        let sender = Arc::new(FakeSender::failing(500));
        let (orch, registry, conv_id) = fixture(Arc::clone(&sender));

        orch.send_user_input(&conv_id, UserInput::Text { body: "4".into() })
            .await
            .unwrap_err();

        // Contrato exacto (FIX-2): transcript = [user(seq 1), system_note(seq 2)].
        let snap = registry.snapshot(&conv_id).unwrap();
        assert_eq!(snap.len(), 2, "transcript: {snap:?}");
        assert_eq!(snap[0].seq, 1);
        assert_eq!(snap[0].direction, Direction::FromUser);
        assert_eq!(snap[0].kind, EventKind::UserText);
        assert_eq!(snap[1].seq, 2);
        assert_eq!(snap[1].direction, Direction::System);
        assert_eq!(snap[1].kind, EventKind::SystemNote);
    }

    #[tokio::test]
    async fn conversacion_desconocida_err_sin_post() {
        let sender = Arc::new(FakeSender::ok());
        let (orch, _registry, _conv_id) = fixture(Arc::clone(&sender));
        let missing = "conv_0000000000000000".to_string();

        let err = orch
            .send_user_input(&missing, UserInput::Text { body: "4".into() })
            .await
            .unwrap_err();
        assert!(matches!(err, NvError::ConversationNotFound(_)));
        assert!(
            sender.calls.lock().unwrap().is_empty(),
            "no debe salir POST si la conversación no existe"
        );
    }

    #[tokio::test]
    async fn wamids_son_unicos_por_mensaje() {
        let sender = Arc::new(FakeSender::ok());
        let (orch, _registry, conv_id) = fixture(sender);

        let w1 = orch
            .send_user_input(&conv_id, UserInput::Text { body: "4".into() })
            .await
            .unwrap();
        let w2 = orch
            .send_user_input(&conv_id, UserInput::Text { body: "5".into() })
            .await
            .unwrap();
        assert_ne!(w1, w2, "cada mensaje lleva wamid nuevo (§C-6)");
    }
}
