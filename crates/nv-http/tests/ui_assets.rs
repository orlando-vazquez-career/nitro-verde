//! Tests de la UI embebida (T12) vía `Router::oneshot` (sin levantar red).
//!
//! `GET /` → 200 `text/html` con `id="chat"`; `GET /assets/app.js` → 200
//! `javascript`; `GET /assets/style.css` → 200 `css`; asset inexistente →
//! 404. En debug rust-embed lee `ui/` del disco (vía CARGO_MANIFEST_DIR).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use nv_http::ui_assets;
use tower::ServiceExt;

/// GET contra el router de assets y captura (status, content-type, body).
async fn get(uri: &str) -> (StatusCode, String, Vec<u8>) {
    let request = Request::get(uri).body(Body::empty()).unwrap();
    let response = ui_assets::router().oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = response.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, content_type, body)
}

#[tokio::test]
async fn ui_index_200_text_html_con_id_chat() {
    let (status, content_type, body) = get("/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/html"), "content-type: {content_type}");
    let html = String::from_utf8(body).unwrap();
    assert!(html.contains(r#"id="chat""#), "index.html sin id=\"chat\"");
}

#[tokio::test]
async fn ui_app_js_200_javascript() {
    let (status, content_type, body) = get("/assets/app.js").await;

    assert_eq!(status, StatusCode::OK);
    assert!(content_type.contains("javascript"), "content-type: {content_type}");
    assert!(!body.is_empty());
}

#[tokio::test]
async fn ui_style_css_200_css() {
    let (status, content_type, body) = get("/assets/style.css").await;

    assert_eq!(status, StatusCode::OK);
    assert!(content_type.contains("css"), "content-type: {content_type}");
    assert!(!body.is_empty());
}

#[tokio::test]
async fn ui_asset_inexistente_404() {
    let (status, _, _) = get("/assets/no-existo.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
