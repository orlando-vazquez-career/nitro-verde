//! Payloads OUTBOUND de la Cloud API de Meta (akiveo-api → NitroVerde), §C-3.
//!
//! El cliente real (`WhatsAppCloudClient`) envía bodies JSON con
//! `messaging_product`, `to` y un discriminador `type`. `mark_as_read` se
//! reconoce por la presencia de `status: "read"` (sin `type`).
//! Campos desconocidos se ignoran (serde default); `recipient_type` opcional.

use serde::{Deserialize, Serialize};

/// Body entrante al endpoint `POST /graph/{version}/{phone_number_id}/messages`.
///
/// `MarkAsRead` va primero: es la única variante con `status` y sin `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutboundPayload {
    MarkAsRead(MarkAsRead),
    Message(OutboundMessage),
}

/// `{"messaging_product":"whatsapp","status":"read","message_id":"wamid..."}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkAsRead {
    pub messaging_product: Option<String>,
    pub status: String,
    pub message_id: String,
}

/// Mensaje outbound, discriminado por el campo wire `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutboundMessage {
    Text {
        #[serde(default)]
        messaging_product: Option<String>,
        #[serde(default)]
        to: Option<String>,
        text: TextBody,
    },
    Interactive {
        #[serde(default)]
        messaging_product: Option<String>,
        #[serde(default)]
        to: Option<String>,
        interactive: Interactive,
    },
    Template {
        #[serde(default)]
        messaging_product: Option<String>,
        #[serde(default)]
        to: Option<String>,
        template: Template,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBody {
    pub body: String,
}

/// Contenido interactivo, discriminado por su `type` interno.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Interactive {
    Button { body: Body, action: ButtonAction },
    List { body: Body, action: ListAction },
    CtaUrl { body: Body, action: CtaUrlAction },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ButtonAction {
    pub buttons: Vec<ButtonSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ButtonSpec {
    /// En el wire siempre es `"reply"`.
    #[serde(rename = "type")]
    pub kind: String,
    pub reply: ReplyRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyRef {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListAction {
    /// Texto del botón que abre la lista (ej. "Ver opciones").
    pub button: String,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    #[serde(default)]
    pub title: Option<String>,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CtaUrlAction {
    /// En el wire siempre es `"cta_url"`.
    pub name: String,
    pub parameters: CtaUrlParams,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CtaUrlParams {
    pub display_text: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub language: Language,
    #[serde(default)]
    pub components: Vec<Component>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Language {
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    /// Ej. `"body"`.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    /// Ej. `"text"`.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
}

/// Respuesta de éxito Meta-like a un envío de mensaje (C-3):
/// `{"messaging_product":"whatsapp","contacts":[...],"messages":[{"id":"wamid..."}]}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageResponse {
    pub messaging_product: String,
    pub contacts: Vec<ContactEcho>,
    pub messages: Vec<MessageId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContactEcho {
    pub input: String,
    pub wa_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageId {
    pub id: String,
}

/// Respuesta de éxito a `mark_as_read`, fiel a Meta: `{"success":true}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadResponse {
    pub success: bool,
}

/// Error Meta-style (400 parse / fallo inyectado), C-3:
/// `{"error":{"message":"...","type":"NVParseError","code":400}}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetaError {
    pub error: MetaErrorBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetaErrorBody {
    pub message: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub code: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_text_roundtrip() {
        let json = r#"{"messaging_product":"whatsapp","to":"56900000001","type":"text","text":{"body":"Hola, ¿cómo evaluarías tu atención del 1 al 5?"}}"#;
        let parsed: OutboundPayload = serde_json::from_str(json).unwrap();
        let OutboundPayload::Message(OutboundMessage::Text { to, text, .. }) = &parsed else {
            panic!("expected text message, got {parsed:?}");
        };
        assert_eq!(to.as_deref(), Some("56900000001"));
        assert!(text.body.contains("1 al 5"));
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), v);
    }

    #[test]
    fn outbound_interactive_button_roundtrip() {
        let json = r#"{"messaging_product":"whatsapp","to":"56900000001","type":"interactive","interactive":{"type":"button","body":{"text":"Lamentamos eso. ¿Cuál fue el motivo?"},"action":{"buttons":[{"type":"reply","reply":{"id":"motivo_precio","title":"Me pareció caro"}}]}}}"#;
        let parsed: OutboundPayload = serde_json::from_str(json).unwrap();
        let OutboundPayload::Message(OutboundMessage::Interactive { interactive, .. }) = &parsed
        else {
            panic!("expected interactive, got {parsed:?}");
        };
        let Interactive::Button { action, .. } = interactive else {
            panic!("expected button interactive, got {interactive:?}");
        };
        assert_eq!(action.buttons[0].reply.id, "motivo_precio");
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), v);
    }

    #[test]
    fn outbound_interactive_list_roundtrip() {
        let json = r#"{"messaging_product":"whatsapp","to":"56900000001","type":"interactive","interactive":{"type":"list","body":{"text":"Elegí una opción"},"action":{"button":"Ver opciones","sections":[{"title":"Opciones","rows":[{"id":"lista_x","title":"Opción X","description":"Detalle X"}]}]}}}"#;
        let parsed: OutboundPayload = serde_json::from_str(json).unwrap();
        let OutboundPayload::Message(OutboundMessage::Interactive { interactive, .. }) = &parsed
        else {
            panic!("expected interactive, got {parsed:?}");
        };
        let Interactive::List { action, .. } = interactive else {
            panic!("expected list interactive, got {interactive:?}");
        };
        assert_eq!(action.sections[0].rows[0].id, "lista_x");
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), v);
    }

    #[test]
    fn outbound_interactive_cta_url_roundtrip() {
        let json = r#"{"messaging_product":"whatsapp","to":"56900000001","type":"interactive","interactive":{"type":"cta_url","body":{"text":"Agendá tu asesoría"},"action":{"name":"cta_url","parameters":{"display_text":"Agendar","url":"https://example.com/agendar"}}}}"#;
        let parsed: OutboundPayload = serde_json::from_str(json).unwrap();
        let OutboundPayload::Message(OutboundMessage::Interactive { interactive, .. }) = &parsed
        else {
            panic!("expected interactive, got {parsed:?}");
        };
        let Interactive::CtaUrl { action, .. } = interactive else {
            panic!("expected cta_url interactive, got {interactive:?}");
        };
        assert_eq!(action.parameters.url, "https://example.com/agendar");
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), v);
    }

    #[test]
    fn outbound_template_roundtrip() {
        let json = r#"{"messaging_product":"whatsapp","to":"56900000001","type":"template","template":{"name":"nps_followup","language":{"code":"es"},"components":[{"type":"body","parameters":[{"type":"text","text":"Juan"}]}]}}"#;
        let parsed: OutboundPayload = serde_json::from_str(json).unwrap();
        let OutboundPayload::Message(OutboundMessage::Template { template, .. }) = &parsed else {
            panic!("expected template, got {parsed:?}");
        };
        assert_eq!(template.name, "nps_followup");
        assert_eq!(template.components[0].parameters[0].text.as_deref(), Some("Juan"));
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), v);
    }

    #[test]
    fn outbound_mark_as_read_roundtrip() {
        let json = r#"{"messaging_product":"whatsapp","status":"read","message_id":"wamid.NV_0123456789abcdef"}"#;
        let parsed: OutboundPayload = serde_json::from_str(json).unwrap();
        let OutboundPayload::MarkAsRead(mark) = &parsed else {
            panic!("expected mark_as_read, got {parsed:?}");
        };
        assert_eq!(mark.status, "read");
        assert_eq!(mark.message_id, "wamid.NV_0123456789abcdef");
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), v);
    }

    #[test]
    fn message_response_meta_like_roundtrip() {
        let json = r#"{"messaging_product":"whatsapp","contacts":[{"input":"56900000001","wa_id":"56900000001"}],"messages":[{"id":"wamid.NV_0123456789abcdef"}]}"#;
        let parsed: MessageResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.messages[0].id, "wamid.NV_0123456789abcdef");
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), v);
    }

    #[test]
    fn meta_error_roundtrip() {
        let json = r#"{"error":{"message":"NV fault injection","type":"NVInjected","code":500}}"#;
        let parsed: MetaError = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.error.kind, "NVInjected");
        assert_eq!(parsed.error.code, 500);
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), v);
    }
}
