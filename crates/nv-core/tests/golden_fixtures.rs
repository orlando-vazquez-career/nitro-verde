//! Golden tests anti mock-espejo (T3).
//!
//! Cada fixture de `crates/nv-core/fixtures/` es un JSON del contrato REAL
//! (§C-2 / §C-3), extraído del cliente `WhatsAppCloudClient` y del webhook de
//! akiveo-api — no inventado para que el test pase. Si nv-core no parsea uno
//! de estos fixtures, el mock NO es fiel al contrato de Meta que usa el bot
//! real y hay que arreglar los tipos (no el fixture).

use nv_core::meta::cloud_api::{
    Interactive, MessageResponse, OutboundMessage, OutboundPayload,
};
use nv_core::meta::webhook::{InboundMessage, WebhookPayload};

// ── OUTBOUND (akiveo-api → fake-Meta), §C-3 ────────────────────────────────

#[test]
fn outbound_text_parsea_y_extrae_body() {
    let parsed: OutboundPayload =
        serde_json::from_str(include_str!("../fixtures/outbound_text.json")).unwrap();
    let OutboundPayload::Message(OutboundMessage::Text { to, text, .. }) = &parsed else {
        panic!("expected text message, got {parsed:?}");
    };
    assert_eq!(to.as_deref(), Some("56900000001"));
    assert!(text.body.contains("1 al 5"));
}

#[test]
fn outbound_interactive_buttons_parsea_y_extrae_replies() {
    let parsed: OutboundPayload =
        serde_json::from_str(include_str!("../fixtures/outbound_interactive_buttons.json"))
            .unwrap();
    let OutboundPayload::Message(OutboundMessage::Interactive { interactive, .. }) = &parsed
    else {
        panic!("expected interactive, got {parsed:?}");
    };
    let Interactive::Button { body, action } = interactive else {
        panic!("expected button interactive, got {interactive:?}");
    };
    assert!(body.text.contains("motivo"));
    // Semántica clave del funnel: los ids de motivo sobreviven el wire.
    let replies: Vec<(&str, &str)> = action
        .buttons
        .iter()
        .map(|b| (b.reply.id.as_str(), b.reply.title.as_str()))
        .collect();
    assert_eq!(
        replies,
        [
            ("motivo_precio", "Me pareció caro"),
            ("motivo_atencion", "Mala atención")
        ]
    );
    assert!(action.buttons.iter().all(|b| b.kind == "reply"));
}

#[test]
fn outbound_interactive_list_parsea_y_extrae_rows() {
    let parsed: OutboundPayload =
        serde_json::from_str(include_str!("../fixtures/outbound_interactive_list.json")).unwrap();
    let OutboundPayload::Message(OutboundMessage::Interactive { interactive, .. }) = &parsed
    else {
        panic!("expected interactive, got {parsed:?}");
    };
    let Interactive::List { action, .. } = interactive else {
        panic!("expected list interactive, got {interactive:?}");
    };
    assert_eq!(action.button, "Ver opciones");
    assert_eq!(action.sections.len(), 1);
    assert_eq!(action.sections[0].rows[0].id, "lista_x");
    assert_eq!(action.sections[0].rows[0].title, "Opción X");
    assert_eq!(
        action.sections[0].rows[0].description.as_deref(),
        Some("Detalle de la opción X")
    );
}

#[test]
fn outbound_cta_url_parsea_y_extrae_url() {
    let parsed: OutboundPayload =
        serde_json::from_str(include_str!("../fixtures/outbound_cta_url.json")).unwrap();
    let OutboundPayload::Message(OutboundMessage::Interactive { interactive, .. }) = &parsed
    else {
        panic!("expected interactive, got {parsed:?}");
    };
    let Interactive::CtaUrl { action, .. } = interactive else {
        panic!("expected cta_url interactive, got {interactive:?}");
    };
    assert_eq!(action.name, "cta_url");
    assert_eq!(action.parameters.display_text, "Agendar ahora");
    assert_eq!(action.parameters.url, "https://akiveo.cl/agendar");
}

#[test]
fn outbound_template_parsea_y_extrae_parametros() {
    let parsed: OutboundPayload =
        serde_json::from_str(include_str!("../fixtures/outbound_template.json")).unwrap();
    let OutboundPayload::Message(OutboundMessage::Template { template, .. }) = &parsed else {
        panic!("expected template, got {parsed:?}");
    };
    assert_eq!(template.name, "nps_followup");
    assert_eq!(template.language.code, "es");
    assert_eq!(template.components[0].kind, "body");
    assert_eq!(
        template.components[0].parameters[0].text.as_deref(),
        Some("Juan")
    );
}

#[test]
fn outbound_mark_as_read_parsea_por_status_read() {
    let parsed: OutboundPayload =
        serde_json::from_str(include_str!("../fixtures/outbound_mark_as_read.json")).unwrap();
    let OutboundPayload::MarkAsRead(mark) = &parsed else {
        panic!("expected mark_as_read, got {parsed:?}");
    };
    assert_eq!(mark.status, "read");
    assert_eq!(mark.message_id, "wamid.NV_0123456789abcdef");
}

#[test]
fn outbound_response_200_serializa_estructura_meta_fiel() {
    // Comparación estructural (Value vs Value), no de string: la respuesta que
    // nv-core emite tiene que ser indistinguible de la de Meta para el cliente.
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/outbound_response_200.json")).unwrap();
    let parsed: MessageResponse = serde_json::from_value(fixture.clone()).unwrap();
    assert_eq!(parsed.messaging_product, "whatsapp");
    assert_eq!(parsed.contacts[0].input, "56900000001");
    assert_eq!(parsed.contacts[0].wa_id, "56900000001");
    assert_eq!(parsed.messages[0].id, "wamid.NV_0123456789abcdef");
    assert_eq!(serde_json::to_value(&parsed).unwrap(), fixture);
}

// ── INBOUND (NitroVerde → webhook del bot), §C-2 ───────────────────────────

#[test]
fn inbound_webhook_text_parsea_y_first_message_extrae_body() {
    let parsed: WebhookPayload =
        serde_json::from_str(include_str!("../fixtures/inbound_webhook_text.json")).unwrap();
    assert_eq!(parsed.object, "whatsapp_business_account");
    assert_eq!(parsed.entry[0].id, "WABA_NV");
    assert_eq!(parsed.entry[0].changes[0].field, "messages");
    assert_eq!(
        parsed.first_message(),
        Some(InboundMessage::Text { body: "4".into() })
    );
}

#[test]
fn inbound_webhook_button_reply_parsea_y_extrae_id_y_title() {
    let parsed: WebhookPayload =
        serde_json::from_str(include_str!("../fixtures/inbound_webhook_button_reply.json"))
            .unwrap();
    assert_eq!(
        parsed.first_message(),
        Some(InboundMessage::ButtonReply {
            id: "motivo_precio".into(),
            title: "Me pareció caro".into()
        })
    );
}

#[test]
fn inbound_webhook_list_reply_parsea_y_extrae_id_title_description() {
    let parsed: WebhookPayload =
        serde_json::from_str(include_str!("../fixtures/inbound_webhook_list_reply.json")).unwrap();
    assert_eq!(
        parsed.first_message(),
        Some(InboundMessage::ListReply {
            id: "lista_x".into(),
            title: "Opción X".into(),
            description: String::new()
        })
    );
}
