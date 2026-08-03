//! Payload INBOUND del webhook (NitroVerde → bot), §C-2.
//!
//! Forma Meta-fiel: `entry[0].changes[0].value.messages[0]`.
//! El webhook soporta `text`, `interactive` (`button_reply` / `list_reply`),
//! `button` (template reply) y media (`image` / `document`, T19); todo lo
//! demás cae en `Other`.

use serde::{Deserialize, Serialize};

/// Payload raíz: `{"object":"whatsapp_business_account","entry":[...]}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookPayload {
    pub object: String,
    #[serde(default)]
    pub entry: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    #[serde(default)]
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub field: String,
    pub value: ChangeValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeValue {
    #[serde(default)]
    pub messaging_product: Option<String>,
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub contacts: Vec<WebhookContact>,
    #[serde(default)]
    pub messages: Vec<RawMessage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub display_phone_number: String,
    pub phone_number_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookContact {
    pub profile: Profile,
    pub wa_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
}

/// Mensaje crudo tal como viene en el wire (conserva todos los campos
/// relevantes; la interpretación de alto nivel es `InboundMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawMessage {
    /// Teléfono del usuario SIN `+`.
    pub from: String,
    /// `wamid.NV_<16hex>`.
    pub id: String,
    pub timestamp: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: Option<TextBody>,
    #[serde(default)]
    pub interactive: Option<InteractiveReply>,
    #[serde(default)]
    pub button: Option<TemplateButton>,
    #[serde(default)]
    pub image: Option<MediaRef>,
    #[serde(default)]
    pub document: Option<MediaRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBody {
    pub body: String,
}

/// Respuesta interactiva del usuario, discriminada por su `type` interno.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractiveReply {
    ButtonReply { button_reply: ReplyRef },
    ListReply { list_reply: ListReplyRef },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyRef {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListReplyRef {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

/// Respuesta a botón de template (`type: "button"` en el wire).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateButton {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub payload: Option<String>,
}

/// Referencia a un adjunto (`image` / `document` en el wire, §C-2 media).
/// Meta manda `{"id","mime_type","sha256","caption"}` y, solo para
/// `document`, `filename`. Un solo struct: los ausentes van `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaRef {
    /// `media_id` con el que el target pide info y bytes (`GET /{id}`).
    pub id: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
}

/// Interpretación de alto nivel de `messages[0]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundMessage {
    Text { body: String },
    ButtonReply { id: String, title: String },
    ListReply { id: String, title: String, description: String },
    /// `type: "image"`: receta/foto del usuario (T19, flujo OCR de akiveo-api).
    Image { media_id: String, mime_type: Option<String>, caption: Option<String> },
    /// `type: "document"`: PDF u otro archivo; lleva `filename` además.
    Document {
        media_id: String,
        mime_type: Option<String>,
        caption: Option<String>,
        filename: Option<String>,
    },
    Other,
}

impl WebhookPayload {
    /// Extrae `entry[0].changes[0].value.messages[0]` como `InboundMessage`.
    pub fn first_message(&self) -> Option<InboundMessage> {
        let msg = self.entry.first()?.changes.first()?.value.messages.first()?;
        Some(msg.as_inbound())
    }
}

impl RawMessage {
    /// Interpreta el mensaje según su `type` del wire.
    pub fn as_inbound(&self) -> InboundMessage {
        match self.kind.as_str() {
            "text" => match &self.text {
                Some(t) => InboundMessage::Text { body: t.body.clone() },
                None => InboundMessage::Other,
            },
            "interactive" => match &self.interactive {
                Some(InteractiveReply::ButtonReply { button_reply }) => {
                    InboundMessage::ButtonReply {
                        id: button_reply.id.clone(),
                        title: button_reply.title.clone(),
                    }
                }
                Some(InteractiveReply::ListReply { list_reply }) => InboundMessage::ListReply {
                    id: list_reply.id.clone(),
                    title: list_reply.title.clone(),
                    description: list_reply.description.clone(),
                },
                None => InboundMessage::Other,
            },
            "image" => match &self.image {
                Some(m) => InboundMessage::Image {
                    media_id: m.id.clone(),
                    mime_type: m.mime_type.clone(),
                    caption: m.caption.clone(),
                },
                None => InboundMessage::Other,
            },
            "document" => match &self.document {
                Some(m) => InboundMessage::Document {
                    media_id: m.id.clone(),
                    mime_type: m.mime_type.clone(),
                    caption: m.caption.clone(),
                    filename: m.filename.clone(),
                },
                None => InboundMessage::Other,
            },
            _ => InboundMessage::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload_with_message(message_json: &str) -> String {
        format!(
            r#"{{"object":"whatsapp_business_account","entry":[{{"id":"WABA_NV","changes":[{{"field":"messages","value":{{"messaging_product":"whatsapp","metadata":{{"display_phone_number":"56900000000","phone_number_id":"NV_PHONE_ID"}},"contacts":[{{"profile":{{"name":"NV User"}},"wa_id":"56900000001"}}],"messages":[{message_json}]}}}}]}}]}}"#
        )
    }

    #[test]
    fn inbound_text_roundtrip() {
        let json = payload_with_message(
            r#"{"from":"56900000001","id":"wamid.NV_0123456789abcdef","timestamp":"1785700000","type":"text","text":{"body":"4"}}"#,
        );
        let parsed: WebhookPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.object, "whatsapp_business_account");
        assert_eq!(
            parsed.first_message(),
            Some(InboundMessage::Text { body: "4".into() })
        );
        // round-trip: serializar y re-parsear conserva la interpretación
        let reparsed: WebhookPayload =
            serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn inbound_button_reply_roundtrip() {
        let json = payload_with_message(
            r#"{"from":"56900000001","id":"wamid.NV_0123456789abcdef","timestamp":"1785700000","type":"interactive","interactive":{"type":"button_reply","button_reply":{"id":"motivo_precio","title":"Me pareció caro"}}}"#,
        );
        let parsed: WebhookPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.first_message(),
            Some(InboundMessage::ButtonReply {
                id: "motivo_precio".into(),
                title: "Me pareció caro".into()
            })
        );
        let reparsed: WebhookPayload =
            serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn inbound_list_reply_roundtrip() {
        let json = payload_with_message(
            r#"{"from":"56900000001","id":"wamid.NV_0123456789abcdef","timestamp":"1785700000","type":"interactive","interactive":{"type":"list_reply","list_reply":{"id":"lista_x","title":"Opción X","description":""}}}"#,
        );
        let parsed: WebhookPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.first_message(),
            Some(InboundMessage::ListReply {
                id: "lista_x".into(),
                title: "Opción X".into(),
                description: String::new()
            })
        );
        let reparsed: WebhookPayload =
            serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn inbound_image_roundtrip() {
        // Payload Meta-real de imagen (flujo OCR de akiveo-api, T19).
        let json = payload_with_message(
            r#"{"from":"56900000001","id":"wamid.NV_0123456789abcdef","timestamp":"1785700000","type":"image","image":{"id":"media_0123456789abcdef","mime_type":"image/jpeg","sha256":"deadbeef","caption":"mi receta"}}"#,
        );
        let parsed: WebhookPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.first_message(),
            Some(InboundMessage::Image {
                media_id: "media_0123456789abcdef".into(),
                mime_type: Some("image/jpeg".into()),
                caption: Some("mi receta".into()),
            })
        );
        let reparsed: WebhookPayload =
            serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn inbound_document_roundtrip() {
        // Document (PDF de receta): lleva `filename` además de caption.
        let json = payload_with_message(
            r#"{"from":"56900000001","id":"wamid.NV_0123456789abcdef","timestamp":"1785700000","type":"document","document":{"id":"media_fedcba9876543210","mime_type":"application/pdf","sha256":"deadbeef","filename":"receta.pdf"}}"#,
        );
        let parsed: WebhookPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.first_message(),
            Some(InboundMessage::Document {
                media_id: "media_fedcba9876543210".into(),
                mime_type: Some("application/pdf".into()),
                caption: None,
                filename: Some("receta.pdf".into()),
            })
        );
        let reparsed: WebhookPayload =
            serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn inbound_unknown_type_maps_to_other() {
        let json = payload_with_message(
            r#"{"from":"56900000001","id":"wamid.NV_0123456789abcdef","timestamp":"1785700000","type":"sticker","sticker":{"id":"media_1"}}"#,
        );
        let parsed: WebhookPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.first_message(), Some(InboundMessage::Other));
    }

    #[test]
    fn empty_entry_yields_none() {
        let json = r#"{"object":"whatsapp_business_account","entry":[]}"#;
        let parsed: WebhookPayload = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.first_message(), None);
    }
}
