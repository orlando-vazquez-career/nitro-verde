//! nv: binario NitroVerde (composition root).
//!
//! La librería expone lo testeable: resolución de config (§C-7) y el serve
//! con graceful shutdown (riesgo ADR #3). `main.rs` solo parsea CLI, arma el
//! wiring y llama a [`serve`].

pub mod config;

/// Sirve el router ya ensamblado hasta que se cancele `shutdown`
/// (riesgo ADR #3: los streams SSE vivos se cierran solos vía
/// `CancellableTail`, así `with_graceful_shutdown` no se bloquea).
pub async fn serve(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    shutdown: tokio_util::sync::CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}
