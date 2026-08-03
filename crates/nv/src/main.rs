//! nv — NitroVerde: fake WhatsApp Cloud API + orchestrator de pruebas.
//!
//! Composition root (§C-7, riesgo ADR #3): parsea CLI, resuelve config
//! (CLI > env > archivo JSON > default), arma el wiring
//! `Registry → Orchestrator (ReqwestWebhookSender) → AppState → router` y
//! sirve con graceful shutdown: `ctrl_c` cancela el token, los streams SSE
//! vivos se cierran solos (CancellableTail) y el server completa.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use nv::config;
use nv_engine::http_client::ReqwestWebhookSender;
use nv_engine::orchestrator::Orchestrator;
use nv_http::state::AppState;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

/// NitroVerde: fake WhatsApp Cloud API para probar bots end-to-end.
#[derive(Debug, Parser)]
#[command(name = "nv", version, about)]
struct Cli {
    /// Dirección de escucha (env: NV_LISTEN, default: 127.0.0.1:9100).
    #[arg(long)]
    listen: Option<String>,
    /// URL del webhook target (env: NV_WEBHOOK_URL,
    /// default: http://localhost:8002/api/v1/whatsapp/webhook).
    #[arg(long)]
    webhook_url: Option<String>,
    /// Secreto HMAC para firmar el webhook (env: NV_APP_SECRET,
    /// default: nv-dev-secret).
    #[arg(long)]
    app_secret: Option<String>,
    /// phone_number_id del bot simulado (env: NV_PHONE_NUMBER_ID,
    /// default: NV_PHONE_ID).
    #[arg(long)]
    phone_number_id: Option<String>,
    /// URL pública con la que el target alcanza a NV; base de la `url`
    /// absoluta en la info de media (env: NV_PUBLIC_URL,
    /// default: http://host.docker.internal:9100).
    #[arg(long)]
    public_url: Option<String>,
    /// Archivo de config JSON (env: NV_CONFIG). Precedence: CLI > env > archivo.
    #[arg(long)]
    config: Option<PathBuf>,
}

/// tracing fmt con env-filter `NV_LOG`, fallback `RUST_LOG`, fallback info.
fn init_tracing() {
    let filter = EnvFilter::try_from_env("NV_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();

    // §C-7: CLI > env > archivo JSON > default. El path del archivo sale de
    // `--config` o de NV_CONFIG.
    let env = |key: &str| std::env::var(key).ok();
    let config_path = cli.config.clone().or_else(|| env("NV_CONFIG").map(PathBuf::from));
    let file = match &config_path {
        Some(path) => Some(config::load_file(path)?),
        None => None,
    };
    let overrides = config::Overrides {
        listen: cli.listen,
        webhook_url: cli.webhook_url,
        app_secret: cli.app_secret,
        phone_number_id: cli.phone_number_id,
        public_url: cli.public_url,
    };
    let cfg = config::resolve(&overrides, &env, file.as_ref());
    tracing::info!(listen = %cfg.listen, webhook_url = %cfg.webhook_url, public_url = %cfg.public_url, "nv: config resuelta");

    // Wiring: Registry → Orchestrator (ReqwestWebhookSender) → AppState →
    // router. El registry del AppState ES el del orchestrator (mismo Arc).
    let state = AppState::new(cfg.phone_number_id.clone(), cfg.public_url.clone());
    let orchestrator = Arc::new(Orchestrator::new(
        Arc::new(ReqwestWebhookSender::new()),
        Arc::clone(&state.registry),
        cfg.webhook_url,
        cfg.app_secret,
        cfg.phone_number_id,
    ));
    let shutdown = CancellationToken::new();
    let app = nv_http::routes::router(state, orchestrator, shutdown.clone());

    let listener = TcpListener::bind(&cfg.listen)
        .await
        .with_context(|| format!("bindeando {}", cfg.listen))?;
    tracing::info!(addr = %cfg.listen, "nv escuchando");

    // ctrl_c → cancelación: cierra los SSE vivos y frena el server (ADR #3).
    let on_signal = shutdown.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::info!("ctrl_c recibido: apagando");
            on_signal.cancel();
        }
    });

    nv::serve(listener, app, shutdown).await?;
    tracing::info!("nv detenido");
    Ok(())
}
