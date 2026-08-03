//! fake_meta: parseo de bodies outbound (akiveo-api → fake-Meta, §C-3) a
//! `ChatEvent` de transcript (§C-5) + respuesta Meta-like con `wamid.NV_*`
//! nuevo (§C-6).
//!
//! Mock honesto: `type` wire desconocido NO es error — queda registrado como
//! evento `unsupported` con el `raw` completo (§C-3). Solo el JSON malformado
//! (o shape roto de un tipo conocido) produce `NvError::Parse`, que T10
//! traduce al 400 `NVParseError` de §C-3.

use nv_core::conversation::{Button, ChatEvent, Direction, EventKind, Section};
use nv_core::error::NvError;
use nv_core::ids::new_wamid;
use nv_core::meta::cloud_api::{
    ContactEcho, Interactive, MarkAsRead, MessageId, MessageResponse, OutboundMessage,
    OutboundPayload, ReadResponse,
};
use serde::{Deserialize, Serialize};

/// Respuesta del fake-Meta a un POST `/messages` (§C-3):
/// éxito con `wamid.NV_<16hex>` para mensajes, `{"success":true}` para
/// mark_as_read. Untagged: serializa directo a la forma wire de cada caso.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetaResponse {
    Message(MessageResponse),
    Read(ReadResponse),
}

/// Parsea un body outbound crudo y devuelve:
///
/// 1. El evento de transcript (`direction = from_bot`).
/// 2. La respuesta Meta-like a devolver.
/// 3. El `to` del payload (`None` en mark_as_read o si viene vacío), para que
///    el caller (T10) resuelva la conversación SIN re-parsear el body.
///
/// Puro y síncrono: la conversación/registry la resuelve el caller (T10).
pub fn process_outbound(body: &[u8]) -> Result<(ChatEvent, MetaResponse, Option<String>), NvError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| NvError::Parse(e.to_string()))?;

    if !is_supported_shape(&value) {
        // `type` desconocido (o ausente sin `status`): evento `unsupported`
        // con raw completo, SIN error (§C-3, mock honesto).
        let to = value
            .get("to")
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .filter(|t| !t.trim().is_empty());
        let mut event = ChatEvent::new(Direction::FromBot, EventKind::Unsupported);
        event.raw = Some(value);
        let response = MetaResponse::Message(message_response(None, new_wamid()));
        return Ok((event, response, to));
    }

    let payload: OutboundPayload =
        serde_json::from_value(value.clone()).map_err(|e| NvError::Parse(e.to_string()))?;

    match payload {
        OutboundPayload::MarkAsRead(mark) => Ok(mark_as_read_event(mark, value)),
        OutboundPayload::Message(message) => Ok(message_event(message, value)),
    }
}

/// ¿El shape wire es uno de los soportados (§C-3)? Se decide sobre el
/// `Value` antes de deserializar tipado, para que los `type` desconocidos
/// caigan en `unsupported` y no en un error serde.
fn is_supported_shape(value: &serde_json::Value) -> bool {
    match value.get("type").and_then(|t| t.as_str()) {
        Some("text") | Some("template") => true,
        Some("interactive") => matches!(
            value
                .get("interactive")
                .and_then(|i| i.get("type"))
                .and_then(|t| t.as_str()),
            Some("button") | Some("list") | Some("cta_url")
        ),
        // mark_as_read: sin `type`, con `status` (§C-3).
        None => value.get("status").is_some(),
        _ => false,
    }
}

/// Evento + respuesta para un mensaje (text / interactive / template) + el
/// `to` del payload (`None` si viene vacío, misma regla que el peek viejo).
fn message_event(
    message: OutboundMessage,
    raw: serde_json::Value,
) -> (ChatEvent, MetaResponse, Option<String>) {
    let (to, mut event) = match message {
        OutboundMessage::Text { to, text, .. } => {
            let mut event = ChatEvent::new(Direction::FromBot, EventKind::Text);
            event.text = Some(text.body);
            (to, event)
        }
        OutboundMessage::Interactive {
            to, interactive, ..
        } => {
            let event = match interactive {
                Interactive::Button { body, action } => {
                    let mut event = ChatEvent::new(Direction::FromBot, EventKind::Buttons);
                    event.text = Some(body.text);
                    event.buttons = action
                        .buttons
                        .into_iter()
                        .map(|b| Button {
                            id: b.reply.id,
                            title: b.reply.title,
                        })
                        .collect();
                    event
                }
                Interactive::List { body, action } => {
                    let mut event = ChatEvent::new(Direction::FromBot, EventKind::List);
                    event.text = Some(body.text);
                    event.sections = action
                        .sections
                        .into_iter()
                        .map(|s| Section {
                            title: s.title,
                            rows: s
                                .rows
                                .into_iter()
                                .map(|r| nv_core::conversation::Row {
                                    id: r.id,
                                    title: r.title,
                                    description: r.description,
                                })
                                .collect(),
                        })
                        .collect();
                    event
                }
                Interactive::CtaUrl { body, .. } => {
                    let mut event = ChatEvent::new(Direction::FromBot, EventKind::CtaUrl);
                    event.text = Some(body.text);
                    event
                }
            };
            (to, event)
        }
        OutboundMessage::Template { to, template, .. } => {
            let mut event = ChatEvent::new(Direction::FromBot, EventKind::Template);
            // Render best-effort (§T8): nombre + variables en texto plano.
            let vars: Vec<&str> = template
                .components
                .iter()
                .flat_map(|c| &c.parameters)
                .filter_map(|p| p.text.as_deref())
                .collect();
            event.text = Some(if vars.is_empty() {
                template.name
            } else {
                format!("{}: {}", template.name, vars.join(", "))
            });
            (to, event)
        }
    };

    let wamid = new_wamid();
    event.wamid = Some(wamid.clone());
    event.raw = Some(raw);
    let to = to.filter(|t| !t.trim().is_empty());
    (
        event,
        MetaResponse::Message(message_response(to.clone(), wamid)),
        to,
    )
}

/// Evento + respuesta para mark_as_read: `{"success":true}`, fiel a Meta.
/// El `wamid` del evento es el `message_id` que se marca como leído y el
/// `raw` conserva el payload wire original (como el resto de los eventos).
/// Sin `to`: no es atribuible a ninguna conversación (§C-3).
fn mark_as_read_event(
    mark: MarkAsRead,
    raw: serde_json::Value,
) -> (ChatEvent, MetaResponse, Option<String>) {
    let mut event = ChatEvent::new(Direction::FromBot, EventKind::MarkAsRead);
    event.wamid = Some(mark.message_id);
    event.raw = Some(raw);
    (event, MetaResponse::Read(ReadResponse { success: true }), None)
}

/// `{"messaging_product":"whatsapp","contacts":[{input,wa_id}],"messages":[{id}]}`
fn message_response(to: Option<String>, wamid: String) -> MessageResponse {
    let to = to.unwrap_or_default();
    MessageResponse {
        messaging_product: "whatsapp".into(),
        contacts: vec![ContactEcho {
            input: to.clone(),
            wa_id: to,
        }],
        messages: vec![MessageId { id: wamid }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lee un fixture golden de nv-core (T3) vía `CARGO_MANIFEST_DIR`.
    fn fixture(name: &str) -> Vec<u8> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../nv-core/fixtures/");
        std::fs::read(format!("{path}{name}")).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// `^wamid\.NV_[0-9a-f]{16}$` sin regex (fuera del stack).
    fn assert_wamid_format(wamid: &str) {
        assert!(wamid.starts_with("wamid.NV_"), "wamid: {wamid}");
        let hex_part = &wamid["wamid.NV_".len()..];
        assert_eq!(hex_part.len(), 16, "wamid: {wamid}");
        assert!(
            hex_part
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "wamid: {wamid}"
        );
    }

    fn parse_fixture(name: &str) -> (ChatEvent, MetaResponse, Option<String>) {
        process_outbound(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    #[test]
    fn fake_meta_text_desde_fixture() {
        let (event, response, to) = parse_fixture("outbound_text.json");
        assert_eq!(event.direction, Direction::FromBot);
        assert_eq!(event.kind, EventKind::Text);
        assert_eq!(
            event.text.as_deref(),
            Some("Hola, ¿cómo evaluarías tu atención del 1 al 5?")
        );
        assert!(event.raw.is_some());
        // El `to` sale del MISMO parseo (sin re-parseo del body, FIX-7).
        assert_eq!(to.as_deref(), Some("56900000001"));
        let wamid = event.wamid.as_deref().expect("wamid");
        assert_wamid_format(wamid);
        let MetaResponse::Message(msg) = response else {
            panic!("expected Message response");
        };
        assert_eq!(msg.messaging_product, "whatsapp");
        assert_eq!(msg.contacts[0].input, "56900000001");
        assert_eq!(msg.contacts[0].wa_id, "56900000001");
        assert_eq!(msg.messages[0].id, wamid);
    }

    #[test]
    fn fake_meta_interactive_buttons_desde_fixture() {
        let (event, response, to) = parse_fixture("outbound_interactive_buttons.json");
        assert_eq!(event.kind, EventKind::Buttons);
        assert_eq!(
            event.text.as_deref(),
            Some("Lamentamos eso. ¿Cuál fue el motivo?")
        );
        assert_eq!(
            event.buttons,
            vec![
                Button {
                    id: "motivo_precio".into(),
                    title: "Me pareció caro".into()
                },
                Button {
                    id: "motivo_atencion".into(),
                    title: "Mala atención".into()
                }
            ]
        );
        assert!(event.sections.is_empty());
        assert!(matches!(response, MetaResponse::Message(_)));
        assert_eq!(to.as_deref(), Some("56900000001"));
        assert_wamid_format(event.wamid.as_deref().expect("wamid"));
    }

    #[test]
    fn fake_meta_interactive_list_desde_fixture() {
        let (event, ..) = parse_fixture("outbound_interactive_list.json");
        assert_eq!(event.kind, EventKind::List);
        assert_eq!(event.text.as_deref(), Some("Elegí una opción"));
        assert_eq!(event.sections.len(), 1);
        assert_eq!(event.sections[0].title.as_deref(), Some("Opciones"));
        assert_eq!(event.sections[0].rows.len(), 2);
        assert_eq!(event.sections[0].rows[0].id, "lista_x");
        assert_eq!(event.sections[0].rows[0].title, "Opción X");
        assert_eq!(
            event.sections[0].rows[0].description.as_deref(),
            Some("Detalle de la opción X")
        );
        assert_wamid_format(event.wamid.as_deref().expect("wamid"));
    }

    #[test]
    fn fake_meta_cta_url_desde_fixture() {
        let (event, ..) = parse_fixture("outbound_cta_url.json");
        assert_eq!(event.kind, EventKind::CtaUrl);
        assert_eq!(event.text.as_deref(), Some("Agendá tu asesoría gratuita"));
        assert_wamid_format(event.wamid.as_deref().expect("wamid"));
    }

    #[test]
    fn fake_meta_template_desde_fixture() {
        let (event, ..) = parse_fixture("outbound_template.json");
        assert_eq!(event.kind, EventKind::Template);
        // Render best-effort: nombre + variables sustituidas en texto plano.
        assert_eq!(event.text.as_deref(), Some("nps_followup: Juan"));
        assert_wamid_format(event.wamid.as_deref().expect("wamid"));
    }

    #[test]
    fn fake_meta_mark_as_read_desde_fixture() {
        let (event, response, to) = parse_fixture("outbound_mark_as_read.json");
        assert_eq!(event.kind, EventKind::MarkAsRead);
        // El wamid del evento es el message_id que se marca como leído.
        assert_eq!(
            event.wamid.as_deref(),
            Some("wamid.NV_0123456789abcdef")
        );
        // Sin `to`: no es atribuible a ninguna conversación (§C-3).
        assert_eq!(to, None);
        // El raw conserva el payload wire original, como el resto de los
        // eventos (antes se perdía, FIX-8).
        let raw = event.raw.as_ref().expect("raw wire presente");
        assert_eq!(raw["status"], "read");
        assert_eq!(raw["message_id"], "wamid.NV_0123456789abcdef");
        let MetaResponse::Read(read) = response else {
            panic!("expected Read response");
        };
        assert!(read.success);
        // y serializa a la forma wire fiel a Meta
        assert_eq!(serde_json::to_value(&read).unwrap(), serde_json::json!({"success":true}));
    }

    #[test]
    fn fake_meta_type_desconocido_es_unsupported_sin_error() {
        let body = "{\"messaging_product\":\"whatsapp\",\"to\":\"56900000001\",\"type\":\"reaction\",\"reaction\":{\"message_id\":\"wamid.NV_x\",\"emoji\":\"\u{1F44D}\"}}".as_bytes();
        let (event, response, to) = process_outbound(body).expect("reaction no debe dar error");
        assert_eq!(event.direction, Direction::FromBot);
        assert_eq!(event.kind, EventKind::Unsupported);
        // raw completo, mock honesto (§C-3)
        let raw = event.raw.expect("raw completo");
        assert_eq!(raw["type"], "reaction");
        assert_eq!(raw["reaction"]["emoji"], "👍");
        // El `to` se extrae también del shape unsupported (atribución §C-3).
        assert_eq!(to.as_deref(), Some("56900000001"));
        assert!(matches!(response, MetaResponse::Message(_)));
    }

    #[test]
    fn fake_meta_interactive_subtipo_desconocido_es_unsupported() {
        let body = br#"{"messaging_product":"whatsapp","to":"56900000001","type":"interactive","interactive":{"type":"product","body":{"text":"x"}}}"#;
        let (event, ..) = process_outbound(body).expect("product no debe dar error");
        assert_eq!(event.kind, EventKind::Unsupported);
        assert_eq!(event.raw.as_ref().unwrap()["interactive"]["type"], "product");
    }

    #[test]
    fn fake_meta_json_malformado_es_parse_error() {
        let err = process_outbound(b"{not json").unwrap_err();
        assert!(matches!(err, NvError::Parse(_)), "err: {err:?}");
    }

    #[test]
    fn fake_meta_shape_roto_de_tipo_conocido_es_parse_error() {
        // type=text conocido pero sin `text.body`: 400 NVParseError (§C-3),
        // NO unsupported (el tipo declarado existe, el shape está roto).
        let body = br#"{"messaging_product":"whatsapp","to":"56900000001","type":"text"}"#;
        let err = process_outbound(body).unwrap_err();
        assert!(matches!(err, NvError::Parse(_)), "err: {err:?}");
    }

    #[test]
    fn fake_meta_response_serializa_forma_c3() {
        let (event, response, _) = parse_fixture("outbound_text.json");
        let v = serde_json::to_value(&response).unwrap();
        assert_eq!(v["messaging_product"], "whatsapp");
        assert_eq!(v["contacts"][0]["input"], "56900000001");
        assert_eq!(v["contacts"][0]["wa_id"], "56900000001");
        assert_eq!(
            v["messages"][0]["id"],
            serde_json::Value::String(event.wamid.unwrap())
        );
    }
}
