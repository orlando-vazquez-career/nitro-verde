//! Test de graceful shutdown (T13, riesgo ADR #3): con un cliente SSE
//! conectado, cancelar el token debe (1) completar el handle del server en
//! < 2s y (2) terminar el stream del cliente.
//!
//! Cliente SSE crudo sobre `TcpStream` (sin reqwest en el bin: el grafo
//! reserva reqwest a nv-engine). El keep-alive de 15s no se espera: el
//! cierre lo dispara `CancellableTail`, no el keep-alive.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nv_engine::http_client::ReqwestWebhookSender;
use nv_engine::orchestrator::Orchestrator;
use nv_http::state::AppState;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// Lee del socket hasta encontrar `needle` (o EOF); devuelve lo acumulado.
async fn read_until(stream: &mut TcpStream, needle: &[u8]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if data.windows(needle.len()).any(|w| w == needle) {
            return data;
        }
        let n = stream.read(&mut buf).await.expect("leyendo del socket");
        if n == 0 {
            return data; // EOF
        }
        data.extend_from_slice(&buf[..n]);
    }
}

#[tokio::test]
async fn graceful_shutdown_cierra_sse_y_completa_en_menos_de_2s() {
    // Wiring igual al de main: Registry → Orchestrator → AppState → router.
    let state = AppState::new("NV_PHONE_ID", "http://127.0.0.1:9100");
    let conversation_id = state.registry.create("56900000001");
    let orchestrator = Arc::new(Orchestrator::new(
        Arc::new(ReqwestWebhookSender::new()),
        Arc::clone(&state.registry),
        "http://127.0.0.1:1/webhook", // inalcanzable: no se usa en este test
        "test-secret",
        "NV_PHONE_ID",
    ));
    let token = CancellationToken::new();
    let app = nv_http::routes::router(state, orchestrator, token.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(nv::serve(listener, app, token.clone()));

    // Cliente SSE crudo: GET del tail y espera de headers.
    let mut client = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "GET /api/conversations/{conversation_id}/events HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Accept: text/event-stream\r\n\
         \r\n"
    );
    client.write_all(request.as_bytes()).await.unwrap();
    let headers = read_until(&mut client, b"\r\n\r\n").await;
    let headers = String::from_utf8(headers).unwrap();
    assert!(headers.contains("200"), "headers: {headers}");
    assert!(headers.contains("text/event-stream"), "headers: {headers}");

    // Cancelación: el server completa en < 2s a pesar del SSE vivo.
    token.cancel();
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("el server no completó en 2s (SSE bloqueando el shutdown)")
        .expect("task del server paniqueó")
        .expect("serve devolvió error");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "shutdown tardó {:?}",
        started.elapsed()
    );

    // Y el stream del cliente TERMINA (EOF tras el chunk de cierre).
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut buf = [0u8; 4096];
        loop {
            let n = client.read(&mut buf).await.expect("leyendo hasta EOF");
            if n == 0 {
                break; // EOF: la conexión cerró
            }
        }
    })
    .await
    .expect("el stream SSE del cliente no terminó en 2s");
}
