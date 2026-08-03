//! Registry de conversaciones (§C-5): `DashMap` + broadcast POR conversación.
//!
//! Patrón SSE anti-pérdida: la UI pide `snapshot()` (transcript completo) y
//! LUEGO abre el tail de deltas con `subscribe()`. El broadcast tiene cap
//! acotado (256 POR conversación, NO global): memoria O(conversaciones × 256).
//! Si un receiver se atrasa más que el cap, su `recv()` devuelve
//! `Err(Lagged)` y el caller (T11) lo traduce a `event: resync` + refetch del
//! transcript. El registry solo expone el receiver crudo.
//!
//! Regla de guards (riesgo ADR #4): NINGÚN método del registry es async.
//! Toda la API es sync (append al transcript y `tx.send` son sync), así que
//! es imposible sostener un guard de DashMap/RwLock a través de un `.await`.
//! Aun así, los guards de DashMap se sueltan de inmediato: se clona el
//! `Arc<Session>` / `Sender` y se trabaja siempre con el clon.

use std::sync::{Arc, RwLock};

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use nv_core::conversation::{ChatEvent, ConversationId, Transcript};
use nv_core::error::{NvError, NvResult};
use nv_core::ids;
use tokio::sync::broadcast;

/// Capacidad del canal broadcast por conversación (§C-5: 256).
pub const BROADCAST_CAPACITY: usize = 256;

/// Resumen de una conversación para `GET /api/conversations`.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ConversationSummary {
    pub conversation_id: ConversationId,
    pub phone: String,
    pub event_count: usize,
    pub last_ts: Option<String>,
}

/// Sesión viva de una conversación: transcript + difusión de deltas.
pub struct Session {
    /// Phone del usuario simulado (sin `+`, §C-1). Lo lee T9 para el `from`.
    phone: String,
    /// Transcript completo (snapshot anti-pérdida). `std::sync::RwLock`
    /// porque las secciones críticas son cortas y NUNCA cruzan un `.await`.
    transcript: RwLock<Transcript>,
    /// Deltas en vivo para el SSE (cap `BROADCAST_CAPACITY`).
    tx: broadcast::Sender<ChatEvent>,
}

impl Session {
    fn new(phone: &str) -> Self {
        let (tx, _rx) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            phone: phone.to_string(),
            transcript: RwLock::new(Transcript::new()),
            tx,
        }
    }

    /// Phone del usuario simulado (sin `+`).
    pub fn phone(&self) -> &str {
        &self.phone
    }
}

/// Registry central: conversaciones por id + índice por phone (§C-3:
/// auto-creación al vuelo cuando llega outbound para un `to` desconocido).
#[derive(Default)]
pub struct Registry {
    conversations: DashMap<ConversationId, Arc<Session>>,
    /// Índice phone → id (el phone es la clave estable del lado fake-Meta).
    by_phone: DashMap<String, ConversationId>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Crea una conversación NUEVA para `phone` (§C-1: `POST /api/conversations`).
    /// Si el phone ya tenía conversación, el índice pasa a apuntar a la nueva
    /// (la vieja sigue viva por id, pero el outbound del bot va a la última).
    pub fn create(&self, phone: &str) -> ConversationId {
        let id = ids::new_conversation_id();
        let session = Arc::new(Session::new(phone));
        // Guards sueltos entre llamadas: jamás anidados ni vivos en un await.
        self.conversations.insert(id.clone(), session);
        self.by_phone.insert(phone.to_string(), id.clone());
        id
    }

    /// Id de la conversación de `phone`, creándola al vuelo si no existe
    /// (§C-3: el outbound del bot NUNCA se pierde).
    pub fn get_or_create_by_phone(&self, phone: &str) -> ConversationId {
        if let Some(id) = self.find_by_phone(phone) {
            return id;
        }
        // No existe: crear la sesión FUERA de cualquier guard y adjudicar el
        // índice atómicamente. Si otro hilo ganó la carrera, se usa la de él
        // y la sesión propia se descarta (nunca se publicó).
        let session = Arc::new(Session::new(phone));
        let new_id = ids::new_conversation_id();
        let (id, winner) = match self.by_phone.entry(phone.to_string()) {
            Entry::Occupied(e) => (e.get().clone(), None),
            Entry::Vacant(e) => {
                e.insert(new_id.clone());
                (new_id, Some(session))
            }
        };
        // El guard del entry ya murió arriba; recién ahora toco `conversations`.
        if let Some(session) = winner {
            self.conversations.insert(id.clone(), session);
        }
        id
    }

    /// Id existente para `phone`, SIN crear (lookups y asserts de tests).
    pub fn find_by_phone(&self, phone: &str) -> Option<ConversationId> {
        self.by_phone.get(phone).map(|r| r.value().clone())
    }

    /// Listado de conversaciones vivas para `GET /api/conversations`
    /// (mensajes que LLEGAN primero — el sistema escribe antes que el usuario:
    /// la UI necesita descubrirlas sin crear duplicados). Orden: último
    /// evento primero; las vacías al final. Todo sync, guards sueltos.
    pub fn list_summaries(&self) -> Vec<ConversationSummary> {
        let mut out: Vec<ConversationSummary> = self
            .conversations
            .iter()
            .map(|entry| {
                let (id, session) = (entry.key(), entry.value());
                let transcript = session.transcript.read().expect("transcript lock poisoned");
                ConversationSummary {
                    conversation_id: id.clone(),
                    phone: session.phone().to_string(),
                    event_count: transcript.events.len(),
                    last_ts: transcript.events.last().map(|e| e.ts.clone()),
                }
            })
            .collect();
        out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        out
    }

    /// Append al transcript + difusión del delta (§C-5). Devuelve el evento
    /// ya numerado (`seq` monótono, asignado por `Transcript::append`).
    /// El error de `tx.send` (cero subscribers) se ignora a propósito: el
    /// transcript es la fuente de verdad; el broadcast es solo el tail en vivo.
    pub fn append_event(&self, id: &ConversationId, event: ChatEvent) -> NvResult<ChatEvent> {
        let session = self.session(id)?; // Arc clonado, guard de DashMap suelto
        let numbered = session
            .transcript
            .write()
            .expect("transcript lock poisoned")
            .append(event);
        let _ = session.tx.send(numbered.clone());
        Ok(numbered)
    }

    /// Copia completa y ordenada del transcript (snapshot anti-pérdida, §C-5).
    pub fn snapshot(&self, id: &ConversationId) -> NvResult<Vec<ChatEvent>> {
        let session = self.session(id)?;
        let events = session
            .transcript
            .read()
            .expect("transcript lock poisoned")
            .snapshot();
        Ok(events)
    }

    /// Receiver crudo del tail en vivo. Si se atrasa más que el cap, su
    /// `recv()` devuelve `Err(Lagged)` — el caller (T11) emite `resync`.
    pub fn subscribe(&self, id: &ConversationId) -> NvResult<broadcast::Receiver<ChatEvent>> {
        let session = self.session(id)?;
        Ok(session.tx.subscribe())
    }

    /// Phone de la conversación (sin `+`). Lo usa T9 para el `from` de §C-2.
    pub fn phone_of(&self, id: &ConversationId) -> NvResult<String> {
        Ok(self.session(id)?.phone().to_string())
    }

    /// `Arc<Session>` clonado con el guard de DashMap ya suelto (ADR #4).
    fn session(&self, id: &ConversationId) -> NvResult<Arc<Session>> {
        self.conversations
            .get(id)
            .map(|r| Arc::clone(r.value()))
            .ok_or_else(|| NvError::ConversationNotFound(id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nv_core::conversation::{Direction, EventKind};

    fn ev() -> ChatEvent {
        ChatEvent::new(Direction::FromBot, EventKind::Text)
    }

    #[test]
    fn registry_create_y_snapshot_vacio() {
        let reg = Registry::new();
        let id = reg.create("56900000001");
        assert!(id.starts_with("conv_"), "id: {id}");
        assert_eq!(reg.snapshot(&id).unwrap(), Vec::new());
        assert_eq!(reg.phone_of(&id).unwrap(), "56900000001");
        assert_eq!(reg.find_by_phone("56900000001").as_deref(), Some(id.as_str()));
    }

    #[test]
    fn registry_append_numera_y_guarda_en_transcript() {
        let reg = Registry::new();
        let id = reg.create("56900000001");
        let e1 = reg.append_event(&id, ev()).unwrap();
        let e2 = reg.append_event(&id, ev()).unwrap();
        assert_eq!(e1.seq, 1);
        assert_eq!(e2.seq, 2);
        let snap = reg.snapshot(&id).unwrap();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].seq, 1);
        assert_eq!(snap[1].seq, 2);
    }

    #[test]
    fn registry_get_or_create_by_phone_idempotente() {
        let reg = Registry::new();
        let id1 = reg.get_or_create_by_phone("56900000001");
        let id2 = reg.get_or_create_by_phone("56900000001");
        assert_eq!(id1, id2, "mismo phone debe dar misma conversación");
        // Otro phone, otra conversación.
        let otro = reg.get_or_create_by_phone("56900000002");
        assert_ne!(id1, otro);
    }

    #[test]
    fn registry_create_siempre_nuevo_y_by_phone_apunta_al_ultimo() {
        let reg = Registry::new();
        let id1 = reg.create("56900000001");
        let id2 = reg.create("56900000001");
        assert_ne!(id1, id2);
        assert_eq!(reg.find_by_phone("56900000001").as_deref(), Some(id2.as_str()));
        // La vieja sigue viva por id.
        assert_eq!(reg.snapshot(&id1).unwrap(), Vec::new());
    }

    #[test]
    fn registry_id_desconocido_err_conversation_not_found() {
        let reg = Registry::new();
        let missing = "conv_0000000000000000".to_string();
        assert!(matches!(
            reg.append_event(&missing, ev()),
            Err(NvError::ConversationNotFound(_))
        ));
        assert!(matches!(
            reg.snapshot(&missing),
            Err(NvError::ConversationNotFound(_))
        ));
        assert!(matches!(
            reg.subscribe(&missing),
            Err(NvError::ConversationNotFound(_))
        ));
        assert!(matches!(
            reg.phone_of(&missing),
            Err(NvError::ConversationNotFound(_))
        ));
        assert_eq!(reg.find_by_phone("56900000999"), None);
    }

    #[tokio::test]
    async fn registry_subscribe_recibe_solo_deltas_en_orden() {
        // Patrón snapshot + tail (§C-5): lo anterior a la suscripción sale
        // por `snapshot()`, lo posterior llega por el receiver.
        let reg = Registry::new();
        let id = reg.create("56900000001");
        reg.append_event(&id, ev()).unwrap(); // seq 1, pre-suscripción
        let mut rx = reg.subscribe(&id).unwrap();
        reg.append_event(&id, ev()).unwrap(); // seq 2
        reg.append_event(&id, ev()).unwrap(); // seq 3

        assert_eq!(reg.snapshot(&id).unwrap().len(), 3);
        assert_eq!(rx.recv().await.unwrap().seq, 2);
        assert_eq!(rx.recv().await.unwrap().seq, 3);
    }

    #[tokio::test]
    async fn registry_lagged_overflow() {
        // Cap 256, 300 eventos y un receiver que NO lee → Err(Lagged).
        let reg = Registry::new();
        let id = reg.create("56900000001");
        let mut rx = reg.subscribe(&id).unwrap();
        for _ in 0..300 {
            reg.append_event(&id, ev()).unwrap();
        }
        match rx.recv().await {
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                assert_eq!(skipped, 44, "300 enviados - 256 de cap = 44 perdidos");
            }
            other => panic!("esperaba Err(Lagged), vino {other:?}"),
        }
        // Tras el Lagged el canal sigue vivo: llega el evento retenido más
        // viejo (seq 45) y luego el resto en orden.
        assert_eq!(rx.recv().await.unwrap().seq, 45);
        assert_eq!(rx.recv().await.unwrap().seq, 46);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn registry_stress_no_deadlock() {
        let reg = Arc::new(Registry::new());
        let id = reg.create("56900000001");

        let work = async {
            let mut handles = Vec::new();
            // 64 tasks de append concurrente.
            for _ in 0..64 {
                let reg = Arc::clone(&reg);
                let id = id.clone();
                handles.push(tokio::spawn(async move {
                    for _ in 0..50 {
                        reg.append_event(&id, ev()).unwrap();
                    }
                }));
            }
            // 64 tasks de subscribe/snapshot concurrente.
            for _ in 0..64 {
                let reg = Arc::clone(&reg);
                let id = id.clone();
                handles.push(tokio::spawn(async move {
                    for _ in 0..50 {
                        let _rx = reg.subscribe(&id).unwrap();
                        let snap = reg.snapshot(&id).unwrap();
                        let _ = snap.len();
                        tokio::task::yield_now().await;
                    }
                }));
            }
            for h in handles {
                h.await.unwrap();
            }
        };

        tokio::time::timeout(std::time::Duration::from_secs(10), work)
            .await
            .expect("stress: deadlock o más de 10s");

        // 64×50 appends: todos numerados, sin duplicados ni huecos (detecta
        // carreras en el lock del transcript).
        let snap = reg.snapshot(&id).unwrap();
        assert_eq!(snap.len(), 64 * 50);
        for (i, e) in snap.iter().enumerate() {
            assert_eq!(e.seq, (i + 1) as u64, "seq roto en posición {i}");
        }
    }
}
