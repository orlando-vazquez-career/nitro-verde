# ADR-01 — Arquitectura de código de NitroVerde (versión lite)

**Fecha**: 2026-08-02 · **Estado**: APROBADO por el orquestador · **Insumos**: veredicto `vrd_nitroverde_2026-08-02`, Estrategia 01, investigación web del agente arquitecto (fuentes inline)

## Contexto

NitroVerde es una tool Rust (binario único, MIT) que mockea la Meta WhatsApp Cloud API + chat browser para conversar con bots reales en localhost. Restricciones del dueño: legibilidad ante todo (nada de monolito ilegible), concurrencia SSE (decenas de conversaciones, "que no se caiga a los 100 users"), diseño que no bloquee crecimiento (headless como datos día 1, panel pluggable después, endpoints Meta nuevos). Coherencia de estilo con SEELE (otra tool Rust del ecosistema).

## Opciones consideradas

- **A — Single crate, módulos por capa** (estilo Zero2Prod / realworld-axum-sqlx): mínima ceremonia, pero nada enforza la dirección de dependencias — se degrada hacia el monolito vetado. Descartada porque el brief exige garantías estructurales, no disciplina manual.
- **B — Workspace chico hexagonal-lite (4 crates)**: crates por capa; el grafo de `Cargo.toml` ES la regla de dependencias, enforced por el compilador. Estilo SEELE. Elegida.
- **C — Workspace vertical por feature**: fuerza un crate `nv-common` que se degrada en basurero compartido; peor legibilidad E2E para 1-3 personas. Descartada.

## Decisión

**Candidata B — workspace de 4 crates**:

```
nv (binario `nv`) ──▶ nv-http ──▶ nv-engine ──▶ nv-core
nv (binario)      ───────────────▶ nv-engine ──▶ nv-core   # el runner headless futuro usa esta arista sin nv-http
```

- `nv-core`: DOMINIO PURO (sin tokio/axum/reqwest) — tipos del contrato Meta (`meta/cloud_api.rs`, `meta/webhook.rs`), `conversation.rs`, `scenario.rs` (escenarios COMO DATOS serde, día 1), `ports.rs` (traits WebhookSender, EventSink, StatusSource, Clock), `error.rs` (thiserror).
- `nv-engine`: APLICACIÓN async — `registry.rs` (DashMap<ConversationId, Arc<Session>>), `orchestrator.rs` (payload webhook + firma HMAC + POST al target), `fake_meta.rs` (parseo outbound + wamid.NV_*), `fault.rs` (FaultPolicy 500/429/delay — inyección de fallos es NÚCLEO per veredicto), `http_client.rs` (único reqwest).
- `nv-http`: INFRAESTRUCTURA HTTP (único crate con axum) — `routes/{fake_meta,api,sse}.rs`, `state.rs`, `ui_assets.rs` (rust-embed de `ui/`).
- `nv`: composition root — clap CLI (config CLI > env > archivo), tracing, wiring, graceful shutdown.

**Regla de una línea**: `nv-core` no sabe qué es un socket; `nv-engine` no sabe qué es axum; `nv-http` no sabe qué es reqwest. **Regla anti-segmentación**: solo 4 crates; uno nuevo requiere ≥2 consumidores reales o razón de compile-time documentada. Si `nv-engine` < 400 LOC en la lite, fusionar en `nv-core` como módulo `app/`.

**YAGNI explícito**: sin SessionStore trait (sesiones en memoria; crecer si el panel lo pide); sin DTOs espejo (los tipos de `nv-core::meta` SON el contrato wire); sin `utoipa`; sin sqlx hoy (panel fase 2, feature-gated `panel`); sin framework de actores (tokio channels bastan; actor model solo si aparece ordenamiento estricto o timers por sesión).

## Concurrencia SSE (el driver "100 users")

`DashMap<ConversationId, Arc<Session>>` + **un `tokio::sync::broadcast` por conversación** (cap 256, NO global) + `Sse::keep_alive` 15s. Anti-pérdida: **snapshot + live tail** — la UI fetchea `/transcript` al conectar y el SSE solo empuja deltas; ante `Lagged` se emite evento `resync` y la UI refetchea. Memoria O(conversaciones × 256) — sin OOM por cliente zombie. (Refs: axum Discussion #1957, tokio broadcast docs, Shuttle htmx+SSE.)

## Stack (versiones verificadas contra crates.io, 2026-08-02)

axum 0.8.x · tokio 1.x (features explícitas, no `full`) · tokio-stream + tokio-util (BroadcastStream, CancellationToken) · tower 0.5 / tower-http 0.7 · serde + serde_json (preserve_order) · **serde-yaml-ng 0.10** (serde_yaml archivado, serde_yml unmaintained/unsound — aislado detrás de `load_scenario()`) · reqwest **0.12.x** rustls (0.13 muy reciente) · hmac 0.13 + sha2 0.11 + subtle 2.6 + hex 0.4 · dashmap 6.2 · tracing + tracing-subscriber · thiserror 2 (libs) / anyhow 1 (bin) · clap 4.6 derive · rust-embed 8.12 + mime_guess 2.0 · dev: wiremock 0.6.5, http-body-util 0.1. Edition 2024, rust-version 1.85.

## Testing (3 niveles, sin levantar red)

1. `nv-core`: unitarios puros (sin tokio) + **golden tests contra `fixtures/`** con payloads copiados de la doc oficial de Meta — defensa estructural contra el mock-espejo del premortem.
2. `nv-engine`: `wiremock` como target; asserts de firma HMAC contra vector conocido y de FaultPolicy.
3. `nv-http`: `Router::oneshot(Request)` vía `tower::ServiceExt`.

## Consecuencias

- (+) Dirección de dependencias enforced por el compilador; headless futuro sin refactor; tests de dominio sin runtime; coherencia con SEELE.
- (+) Los riesgos del premortem MNEMA quedan mitigados por diseño: fixtures anti mock-espejo, fault injection en núcleo, LIMITACIONES.md, escenarios como datos.
- (−) Boilerplate de 4 Cargo.toml; renombrar tipos entre crates cuesta más; ~2h extra de setup vs single crate (se recuperan en el primer crecimiento).

## Riesgos aceptados con mitigación (del research, resumen)

1. broadcast `Lagged` pierde mensajes → snapshot+tail+cap+resync (incorporado).
2. Firmar JSON re-serializado en vez del raw body → serializar UNA vez a Vec<u8>, firmar esos bytes, enviar esos bytes; test con vector conocido.
3. SSE bloquea graceful shutdown → CancellationToken cierra streams + keep-alive 15s.
4. Guards DashMap/RwLock a través de `.await` → regla de review: clonar Arc/Sender, soltar guard, recién await.
5. Sobre-segmentación → regla de promoción de crates (arriba).
6. serde-yaml-ng bus-factor → aislado detrás de una fn; migración = 1 diff.
7. UI vanilla se come el núcleo → 3 archivos estáticos sin build step; framework front = señal de API de control incompleta.

## Referencias clave

Zero to Production in Rust · launchbadge/realworld-axum-sqlx · howtocodeit.com hexagonal Rust · ryhl.io Actors with Tokio · axum Discussion #1957 · tokio broadcast docs · Shuttle axum guide · URLO serde_yaml deprecation · Meta Cloud API docs · fdarian/whap · @whatsapp-cloudapi/emulator · Hookdeck WhatsApp webhooks.
