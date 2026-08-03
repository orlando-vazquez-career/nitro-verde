//! Puertos hexagonales de NitroVerde: el núcleo define los traits,
//! `nv-engine`/`nv-http` proveen las implementaciones.
//!
//! `StatusSource` NO va (YAGNI explícito del ADR: panel es fase 2).
//!
//! Nota de diseño: los traits async van desazucarados a
//! `Pin<Box<dyn Future + Send>>` en lugar de `async fn` en trait. Motivo:
//! `async fn` nativo NO es dyn-compatible y T9 requiere
//! `Arc<dyn WebhookSender>`; el crate `async-trait` está fuera del stack fijo.

use std::future::Future;
use std::pin::Pin;

use crate::conversation::ChatEvent;
use crate::error::NvError;

/// Futuro boxed `Send` usado por los puertos async.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Emisor del webhook firmado (NitroVerde → bot, §C-2).
///
/// Recibe el body como BYTES ya serializados y la firma ya calculada
/// (`sha256=<hex>`): está PROHIBIDO reserializar el payload dentro de la
/// implementación (riesgo ADR #2 — la firma cubre esos bytes exactos).
pub trait WebhookSender: Send + Sync {
    /// POST de `body` a `url` con header `X-Hub-Signature-256: <signature>`.
    /// Target no-2xx → `NvError::WebhookTarget { status }`.
    fn send<'a>(
        &'a self,
        url: &'a str,
        body: Vec<u8>,
        signature: &'a str,
    ) -> BoxFuture<'a, Result<(), NvError>>;
}

/// Consumidor de eventos de chat (SSE hoy; panel en fase 2).
///
/// Sync a propósito: publicar en un `tokio::sync::broadcast` es sync; si una
/// implementación necesita `.await`, lo hace por fuera del `emit` (regla §0:
/// nada de guards a través de `.await`).
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &ChatEvent);
}

/// Reloj inyectable para timestamps wire y de eventos (testeable en engine).
pub trait Clock: Send + Sync {
    /// Unix epoch en segundos (timestamp del webhook §C-2).
    fn unix_seconds(&self) -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{Direction, EventKind};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    // --- Los traits deben ser dyn-compatible (T9: Arc<dyn WebhookSender>) ---
    #[allow(dead_code)]
    fn assert_dyn_compatible(
        sender: Arc<dyn WebhookSender>,
        sink: Arc<dyn EventSink>,
        clock: Arc<dyn Clock>,
    ) {
        let _ = (sender, sink, clock);
    }

    /// block_on mínimo con waker noop (nv-core no conoce tokio; los futures
    /// de estos fakes resuelven en el primer poll).
    fn block_on<F: Future>(future: F) -> F::Output {
        fn noop(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(out) => return out,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    struct CapturingSender {
        calls: Mutex<Vec<(String, Vec<u8>, String)>>,
    }

    impl WebhookSender for CapturingSender {
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
                Ok(())
            })
        }
    }

    #[test]
    fn webhook_sender_recibe_bytes_y_firma_sin_reserializar() {
        let sender = CapturingSender {
            calls: Mutex::new(vec![]),
        };
        // bytes NO-JSON a propósito: el sender no los toca, solo los envía
        let body = b"bytes crudos \x00\x01 firmados".to_vec();
        let signature = "sha256=deadbeef";
        block_on(sender.send("http://localhost:8002/webhook", body.clone(), signature)).unwrap();
        let calls = sender.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "http://localhost:8002/webhook");
        assert_eq!(calls[0].1, body);
        assert_eq!(calls[0].2, signature);
    }

    struct CollectSink {
        events: Mutex<Vec<EventKind>>,
    }

    impl EventSink for CollectSink {
        fn emit(&self, event: &ChatEvent) {
            self.events.lock().unwrap().push(event.kind);
        }
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn unix_seconds(&self) -> u64 {
            1_785_700_000
        }
    }

    #[test]
    fn sink_y_clock_minimos() {
        let sink = CollectSink {
            events: Mutex::new(vec![]),
        };
        let ev = ChatEvent::new(Direction::FromBot, EventKind::Buttons);
        sink.emit(&ev);
        assert_eq!(sink.events.lock().unwrap().as_slice(), &[EventKind::Buttons]);
        assert_eq!(FixedClock.unix_seconds(), 1_785_700_000);
    }
}
