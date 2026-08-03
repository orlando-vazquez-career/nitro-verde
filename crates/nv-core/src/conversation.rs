//! Conversación y transcript de NitroVerde (§C-5).
//!
//! `ChatEvent` ES la forma wire del transcript y de los deltas SSE:
//! `{"seq":7,"ts":"...Z","direction":"from_bot","kind":"buttons","text":"...",
//!   "buttons":[...],"sections":[],"wamid":"wamid.NV_...","media_id":null,"raw":{...}}`
//! Los campos van SIEMPRE presentes (shape estable para la UI vanilla):
//! `text`/`wamid`/`media_id`/`raw` serializan `null` y `buttons`/`sections`
//! `[]` cuando no aplican.

use serde::{Deserialize, Serialize};

/// Id de conversación (`conv_<16hex>`, §C-6). Alias plano: atraviesa
/// path params HTTP y claves del registry sin wrap/unwrap.
pub type ConversationId = String;

/// Quién emitió el evento (wire snake_case, §C-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Mensaje outbound del bot (akiveo-api → fake-Meta).
    FromBot,
    /// Input del usuario simulado (NitroVerde → webhook).
    FromUser,
    /// Evento interno de NitroVerde (p.ej. fallo inyectado).
    System,
}

/// Tipo de evento (wire snake_case, §C-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Text,
    Buttons,
    List,
    CtaUrl,
    Template,
    MarkAsRead,
    UserText,
    UserButtonReply,
    UserListReply,
    /// Imagen adjuntada por el usuario simulado (T19; `media_id` en el evento).
    Image,
    /// Documento (PDF de receta) adjuntado por el usuario simulado (T19).
    Document,
    /// `type` wire desconocido: se registra con `raw` completo, sin error (§C-3).
    Unsupported,
    /// Nota interna de NitroVerde (`direction = system`, §C-5): p.ej. el
    /// webhook target falló tras registrar el input del usuario (§T9).
    SystemNote,
}

/// Botón clickeable de un evento `buttons` (§C-5: `{"id","title"}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
    pub id: String,
    pub title: String,
}

/// Sección de una lista interactiva (evento `list`, §C-3 list → §C-5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub rows: Vec<Row>,
}

/// Fila clickeable dentro de una `Section`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Evento del transcript (§C-5). `seq` lo asigna `Transcript::append`;
/// al construirlo fuera del transcript queda en 0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatEvent {
    /// Monótono por conversación, 1-based.
    pub seq: u64,
    /// RFC3339 UTC (`2026-08-02T22:00:00Z`).
    pub ts: String,
    pub direction: Direction,
    pub kind: EventKind,
    /// Texto visible (body del mensaje o título clickeado). `null` si no aplica.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub buttons: Vec<Button>,
    #[serde(default)]
    pub sections: Vec<Section>,
    /// `wamid.NV_<16hex>` asociado (outbound del bot o input del usuario).
    #[serde(default)]
    pub wamid: Option<String>,
    /// `media_<16hex>` del adjunto (eventos `image`/`document`, T19). La UI
    /// lo usa para pedir los bytes a `GET /media/{media_id}`. `null` si no aplica.
    #[serde(default)]
    pub media_id: Option<String>,
    /// Payload wire original (debug + mock honesto, §C-5).
    #[serde(default)]
    pub raw: Option<serde_json::Value>,
}

impl ChatEvent {
    /// Evento nuevo con `ts` actual y `seq` pendiente (0) hasta el `append`.
    pub fn new(direction: Direction, kind: EventKind) -> Self {
        Self {
            seq: 0,
            ts: crate::ids::now_rfc3339(),
            direction,
            kind,
            text: None,
            buttons: Vec::new(),
            sections: Vec::new(),
            wamid: None,
            media_id: None,
            raw: None,
        }
    }
}

/// Transcript completo de una conversación (snapshot anti-pérdida SSE, §C-5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub events: Vec<ChatEvent>,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    /// Agrega el evento asignándole `seq` monótono (último + 1, 1-based)
    /// y devuelve el evento ya numerado (para broadcast, §C-5).
    pub fn append(&mut self, mut event: ChatEvent) -> ChatEvent {
        event.seq = self.events.last().map_or(1, |e| e.seq + 1);
        self.events.push(event.clone());
        event
    }

    /// Copia completa y ordenada del transcript (snapshot para la UI).
    pub fn snapshot(&self) -> Vec<ChatEvent> {
        self.events.clone()
    }
}

/// Input del usuario simulado: body EXACTO de `POST /api/conversations/{id}/messages`
/// (§C-1) y del `send` de los escenarios YAML (T4, escenarios como datos).
/// Las variantes media (`image`/`document`, T19) las arma el endpoint
/// multipart `POST /api/conversations/{id}/media`, no el body JSON §C-1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserInput {
    Text { body: String },
    ButtonReply { id: String, title: String },
    ListReply { id: String, title: String },
    /// Imagen adjuntada (T19): webhook `type: "image"` Meta-real.
    Image {
        media_id: String,
        mime_type: String,
        #[serde(default)]
        caption: Option<String>,
        /// sha256 real de los bytes (lo calcula `MediaStore` al guardar).
        #[serde(default)]
        sha256: Option<String>,
    },
    /// Documento adjuntado (T19): webhook `type: "document"` con `filename`.
    Document {
        media_id: String,
        mime_type: String,
        #[serde(default)]
        caption: Option<String>,
        filename: String,
        #[serde(default)]
        sha256: Option<String>,
    },
}

impl UserInput {
    /// `EventKind` del evento `from_user` que genera este input (§C-5).
    pub fn event_kind(&self) -> EventKind {
        match self {
            UserInput::Text { .. } => EventKind::UserText,
            UserInput::ButtonReply { .. } => EventKind::UserButtonReply,
            UserInput::ListReply { .. } => EventKind::UserListReply,
            UserInput::Image { .. } => EventKind::Image,
            UserInput::Document { .. } => EventKind::Document,
        }
    }

    /// Texto visible en la burbuja del usuario (body, título clickeado,
    /// caption del adjunto o etiqueta de respaldo).
    pub fn display_text(&self) -> &str {
        match self {
            UserInput::Text { body } => body,
            UserInput::ButtonReply { title, .. } | UserInput::ListReply { title, .. } => title,
            UserInput::Image { caption, .. } => caption.as_deref().unwrap_or("[imagen]"),
            UserInput::Document {
                caption, filename, ..
            } => caption.as_deref().unwrap_or(filename),
        }
    }

    /// `media_id` del adjunto (`image`/`document`), `None` en el resto.
    pub fn media_id(&self) -> Option<&str> {
        match self {
            UserInput::Image { media_id, .. } | UserInput::Document { media_id, .. } => {
                Some(media_id)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_event_forma_exacta_c5() {
        // JSON literal de §C-5 (evento buttons con raw).
        let json = r#"{"seq":7,"ts":"2026-08-02T22:00:00Z","direction":"from_bot","kind":"buttons","text":"Lamentamos eso. ¿Cuál fue el motivo?","buttons":[{"id":"motivo_precio","title":"Me pareció caro"}],"sections":[],"wamid":"wamid.NV_0123456789abcdef","media_id":null,"raw":{"type":"interactive"}}"#;
        let parsed: ChatEvent = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.seq, 7);
        assert_eq!(parsed.ts, "2026-08-02T22:00:00Z");
        assert_eq!(parsed.direction, Direction::FromBot);
        assert_eq!(parsed.kind, EventKind::Buttons);
        assert_eq!(
            parsed.buttons,
            vec![Button {
                id: "motivo_precio".into(),
                title: "Me pareció caro".into()
            }]
        );
        assert_eq!(parsed.wamid.as_deref(), Some("wamid.NV_0123456789abcdef"));
        // round-trip estructural (comparación por Value, no por string)
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), v);
    }

    #[test]
    fn chat_event_shape_estable_con_nulos() {
        // Evento mínimo: los campos van siempre presentes (UI vanilla sin
        // checks de undefined).
        let ev = ChatEvent::new(Direction::FromBot, EventKind::Text);
        let v = serde_json::to_value(&ev).unwrap();
        let obj = v.as_object().unwrap();
        for key in [
            "seq", "ts", "direction", "kind", "text", "buttons", "sections", "wamid", "media_id",
            "raw",
        ] {
            assert!(obj.contains_key(key), "falta {key} en {v}");
        }
        assert_eq!(obj["text"], serde_json::Value::Null);
        assert_eq!(obj["media_id"], serde_json::Value::Null);
        assert_eq!(obj["buttons"], serde_json::json!([]));
        // y deserializa de vuelta sin problemas
        let back: ChatEvent = serde_json::from_value(v).unwrap();
        assert_eq!(back.kind, EventKind::Text);
    }

    #[test]
    fn direction_y_kind_wire_names() {
        assert_eq!(
            serde_json::to_string(&Direction::FromUser).unwrap(),
            "\"from_user\""
        );
        assert_eq!(
            serde_json::to_string(&Direction::System).unwrap(),
            "\"system\""
        );
        let kinds = [
            (EventKind::Text, "text"),
            (EventKind::Buttons, "buttons"),
            (EventKind::List, "list"),
            (EventKind::CtaUrl, "cta_url"),
            (EventKind::Template, "template"),
            (EventKind::MarkAsRead, "mark_as_read"),
            (EventKind::UserText, "user_text"),
            (EventKind::UserButtonReply, "user_button_reply"),
            (EventKind::UserListReply, "user_list_reply"),
            (EventKind::Image, "image"),
            (EventKind::Document, "document"),
            (EventKind::Unsupported, "unsupported"),
            (EventKind::SystemNote, "system_note"),
        ];
        for (kind, wire) in kinds {
            assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{wire}\""));
            let back: EventKind = serde_json::from_str(&format!("\"{wire}\"")).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn transcript_append_asigna_seq_monotono() {
        let mut t = Transcript::new();
        let e1 = t.append(ChatEvent::new(Direction::FromUser, EventKind::UserText));
        let e2 = t.append(ChatEvent::new(Direction::FromBot, EventKind::Buttons));
        assert_eq!(e1.seq, 1);
        assert_eq!(e2.seq, 2);
        // el seq queda persistido en el transcript, no solo en el retorno
        let snap = t.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].seq, 1);
        assert_eq!(snap[1].seq, 2);
        // snapshot es una copia: mutarla no toca el transcript
        let mut snap2 = t.snapshot();
        snap2.clear();
        assert_eq!(t.snapshot().len(), 2);
    }

    #[test]
    fn transcript_respeta_seq_previo() {
        // Si el último evento tiene seq 7 (p.ej. deserializado), sigue en 8.
        let json = r#"{"events":[{"seq":7,"ts":"2026-08-02T22:00:00Z","direction":"from_bot","kind":"text","text":"hola","buttons":[],"sections":[],"wamid":null,"raw":null}]}"#;
        let mut t: Transcript = serde_json::from_str(json).unwrap();
        let e = t.append(ChatEvent::new(Direction::FromBot, EventKind::Text));
        assert_eq!(e.seq, 8);
    }

    #[test]
    fn user_input_bodies_exactos_c1() {
        // Los 3 bodies literales de §C-1.
        let text: UserInput = serde_json::from_str(r#"{"kind":"text","body":"4"}"#).unwrap();
        assert_eq!(text, UserInput::Text { body: "4".into() });

        let button: UserInput = serde_json::from_str(
            r#"{"kind":"button_reply","id":"motivo_precio","title":"Me pareció caro"}"#,
        )
        .unwrap();
        assert_eq!(
            button,
            UserInput::ButtonReply {
                id: "motivo_precio".into(),
                title: "Me pareció caro".into()
            }
        );

        let list: UserInput =
            serde_json::from_str(r#"{"kind":"list_reply","id":"lista_x","title":"Opción X"}"#)
                .unwrap();
        assert_eq!(
            list,
            UserInput::ListReply {
                id: "lista_x".into(),
                title: "Opción X".into()
            }
        );

        // round-trip de cada uno
        for input in [text, button, list] {
            let back: UserInput =
                serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
            assert_eq!(back, input);
        }
    }

    #[test]
    fn user_input_mapea_event_kind_y_texto() {
        assert_eq!(
            UserInput::Text { body: "4".into() }.event_kind(),
            EventKind::UserText
        );
        let btn = UserInput::ButtonReply {
            id: "motivo_precio".into(),
            title: "Me pareció caro".into(),
        };
        assert_eq!(btn.event_kind(), EventKind::UserButtonReply);
        assert_eq!(btn.display_text(), "Me pareció caro");
        assert_eq!(UserInput::Text { body: "4".into() }.display_text(), "4");
    }

    #[test]
    fn user_input_media_forma_y_mapeos() {
        // Los bodies que arma el endpoint multipart (T19).
        let image: UserInput = serde_json::from_str(
            r#"{"kind":"image","media_id":"media_0123456789abcdef","mime_type":"image/jpeg","caption":"mi receta","sha256":"deadbeef"}"#,
        )
        .unwrap();
        assert_eq!(image.event_kind(), EventKind::Image);
        assert_eq!(image.display_text(), "mi receta");
        assert_eq!(image.media_id(), Some("media_0123456789abcdef"));

        let doc: UserInput = serde_json::from_str(
            r#"{"kind":"document","media_id":"media_fedcba9876543210","mime_type":"application/pdf","filename":"receta.pdf"}"#,
        )
        .unwrap();
        assert_eq!(doc.event_kind(), EventKind::Document);
        // Sin caption: el texto visible es el filename.
        assert_eq!(doc.display_text(), "receta.pdf");
        assert_eq!(doc.media_id(), Some("media_fedcba9876543210"));

        // Imagen sin caption: etiqueta de respaldo.
        let sin_caption = UserInput::Image {
            media_id: "media_x".into(),
            mime_type: "image/png".into(),
            caption: None,
            sha256: None,
        };
        assert_eq!(sin_caption.display_text(), "[imagen]");

        // Los inputs de texto/botones no llevan adjunto.
        assert_eq!(UserInput::Text { body: "4".into() }.media_id(), None);

        // round-trip de las variantes media
        for input in [image, doc] {
            let back: UserInput =
                serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
            assert_eq!(back, input);
        }
    }

    #[test]
    fn chat_event_con_media_id_roundtrip() {
        let mut ev = ChatEvent::new(Direction::FromUser, EventKind::Image);
        ev.media_id = Some("media_0123456789abcdef".into());
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["media_id"], "media_0123456789abcdef");
        assert_eq!(v["kind"], "image");
        let back: ChatEvent = serde_json::from_value(v).unwrap();
        assert_eq!(back.media_id.as_deref(), Some("media_0123456789abcdef"));
    }
}
