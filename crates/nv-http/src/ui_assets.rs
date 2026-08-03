//! UI embebida de NitroVerde (§C-1): `GET /` (index.html) y
//! `GET /assets/{*path}` (app.js, style.css), servida con rust-embed desde
//! `ui/` en la raíz del repo.
//!
//! Vanilla JS sin build step (riesgo ADR #7): no hay pipeline front, los 3
//! archivos se embeben tal cual. En debug rust-embed los lee del disco
//! (iteración rápida); en release van dentro del binario.

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{Router, get};
use rust_embed::RustEmbed;

/// Assets de la UI (`ui/` en la raíz del workspace; la ruta es relativa al
/// manifest de este crate: `crates/nv-http`).
#[derive(RustEmbed)]
#[folder = "../../ui/"]
struct UiAssets;

/// Router de la UI embebida. No toca estado: se mergea tal cual en el
/// router público de `routes::router` (T12).
pub fn router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/{*path}", get(asset))
}

/// `GET /` → `200 text/html` (index.html).
async fn index() -> Response {
    serve_embedded("index.html")
}

/// `GET /assets/{*path}` → el asset embebido con su MIME (mime_guess).
async fn asset(Path(path): Path<String>) -> Response {
    serve_embedded(&path)
}

/// Sirve un archivo del embed con su content-type; 404 si no existe.
/// (El `path` nunca sale del embed: rust-embed busca por clave exacta, así
/// que un `../` no lee nada del disco en release.)
fn serve_embedded(path: &str) -> Response {
    match UiAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref())],
                content.data.into_owned(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
