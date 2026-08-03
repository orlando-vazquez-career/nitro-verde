//! Estado compartido de la capa HTTP (§C-1). Todo `Arc`/String: clonar
//! `AppState` es barato y todos los clones apuntan al mismo estado vivo.

use std::sync::{Arc, RwLock};

use nv_engine::fault::FaultPolicy;
use nv_engine::media::MediaStore;
use nv_engine::registry::Registry;

/// Estado de la aplicación.
///
/// - `registry`: conversaciones vivas (T7), compartido con el orchestrator.
/// - `fault`: política de inyección activa (§C-4). `std::sync::RwLock`
///   porque las secciones críticas son cortas y NUNCA cruzan un `.await`
///   (riesgo ADR #4: se clona la decisión/política, se suelta el guard y
///   recién se awaita — mismo criterio que `Session::transcript` en T7).
/// - `phone_number_id`: el configurado (§C-7, default `NV_PHONE_ID`). Lo
///   usa el orchestrator para el payload webhook (§C-2); la ruta fake-Meta
///   acepta CUALQUIER `{phone_number_id}` de path (§C-3, opaco).
/// - `media`: adjuntos del usuario simulado (T19). Lo escribe el endpoint
///   multipart `/api/conversations/{id}/media` y lo leen
///   `GET /graph/{version}/{media_id}` (info) y `GET /media/{media_id}`
///   (bytes) — el camino de dos pasos que hace `download_media` del target.
/// - `public_url`: base pública con la que se arma la `url` absoluta de la
///   info de media (§C-7 `NV_PUBLIC_URL`). Es la URL con la que el TARGET
///   alcanza a NV (p.ej. `http://host.docker.internal:9100` desde docker),
///   no necesariamente la de escucha local.
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    pub fault: Arc<RwLock<FaultPolicy>>,
    pub phone_number_id: String,
    pub media: Arc<MediaStore>,
    pub public_url: String,
}

impl AppState {
    /// Estado nuevo con registry vacío, media vacío y fault off. El wiring
    /// del binario lo construye con `phone_number_id` y `public_url`
    /// resueltos de la config (§C-7).
    pub fn new(phone_number_id: impl Into<String>, public_url: impl Into<String>) -> Self {
        Self {
            registry: Arc::new(Registry::new()),
            fault: Arc::new(RwLock::new(FaultPolicy::default())),
            phone_number_id: phone_number_id.into(),
            media: Arc::new(MediaStore::new()),
            public_url: public_url.into(),
        }
    }

    /// `url` absoluta de descarga de un adjunto (info Meta-like, T19).
    /// Tolera `public_url` con `/` final.
    pub fn media_download_url(&self, media_id: &str) -> String {
        format!("{}/media/{media_id}", self.public_url.trim_end_matches('/'))
    }
}
