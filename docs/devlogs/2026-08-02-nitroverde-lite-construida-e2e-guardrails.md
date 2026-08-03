# Devlog — 2026-08-02: NitroVerde versión lite construida, verificada E2E y endurecida

**Repo**: `nitro-verde` (nuevo, `C:\dev\tools\nitro-verde`) · **Ciclo AEGIS completo** en un día · **Origen**: veredicto MNEMA `vrd_nitroverde_2026-08-02` (counsel completo, gate humano aceptado con amendment)

## Qué se construyó

**NitroVerde lite**: mock de la Meta WhatsApp Cloud API + chat browser para conversar con bots reales en localhost sin tocar Meta. Workspace Rust de 4 crates (ADR-01):

- `nv-core` — dominio puro (sin tokio/axum/reqwest): contrato Meta wire (outbound text/interactive/template/mark_as_read; inbound webhook text/button_reply/list_reply/image/document), ChatEvent/Transcript, escenarios COMO DATOS (serde-yaml-ng), ports (traits), ids sin `rand`.
- `nv-engine` — aplicación async: registry (DashMap + broadcast por conversación cap 256), orchestrator (payload §C-2 + HMAC sobre bytes crudos + POST al target), fake_meta (parseo outbound → ChatEvent, `unsupported` explícito para tipos desconocidos), FaultPolicy (500/429/delay — inyección de fallos es núcleo), MediaStore (upload/download de media, sha256 real), http_client (único reqwest).
- `nv-http` — único axum: route fake-Meta (`POST /graph/{version}/{phone_number_id}/messages`), API de control (conversations/messages/media/faults/health), SSE (snapshot+tail+resync ante Lagged, keep-alive 15s), media endpoints (`GET /graph/{version}/{media_id}` info + `GET /media/{id}` bytes), UI embebida (rust-embed, 3 archivos vanilla sin build).
- `nv` — composition root: clap (listen/webhook-url/app-secret/phone-number-id/public-url/config), precedence CLI>env>archivo, tracing, graceful shutdown con CancellationToken.

## Verificación E2E real (contra akiveo-api en docker, 2026-08-02)

Con `WHATSAPP_CLOUD_API_BASE_URL=http://host.docker.internal:9100/graph` (paso 0, rama `feat/whatsapp-base-url-configurable`, commit `ed9eb39`):

1. **Chat completo**: NPS "4" → lista de motivos del bot real → button `motivo_precio` → cupón + propuesta clínica (bloque óptica incluido) → `no_personalizada` → gancho estético → `asesoria_agendar` → link con lead_token. Estado final en DB: `oferta_asesoria`. ✓
2. **Outbound solo a NV**: todo el tráfico del bot llegó al fake-Meta; cero a `graph.facebook.com`. ✓
3. **Inyección 500**: fault activada → logs de akiveo-api muestran `reintentar status=500 attempt=1/2/3` — el circuit breaker/retry del cliente real ejercitado, lo que el stub in-process del script Python no puede probar (justificación central del veredicto). ✓
4. **Media (T19, alcance del gate humano UI)**: adjuntar receta por UI/API → akiveo-api la descarga desde NV en 2 pasos Meta-like (info JSON con url absoluta → bytes). ✓

**144 tests verdes + clippy `-D warnings` limpio.**

## Guardrails aplicados (code-reviewer + security-auditor, 8 fixes)

- FIX-1: fake-Meta exige `Content-Type: application/json` (415) — cierra CSRF de escritura desde pestañas del browser.
- FIX-2: el evento del usuario se appendea ANTES del POST al webhook (orden causal del transcript; evento `system_note` si el target falla).
- FIX-3: UI suscribe SSE ANTES del snapshot (buffer + dedup + refetch en reconexión) — cierra la ventana de pérdida silenciosa.
- FIX-4: allowlist `https?:` en links CTA de la UI.
- FIX-5: `.gitignore` para configs locales (`nv.config.json`, `*.local.json`, `.env*`).
- FIX-6: poda de deps muertas (`hex`, `thiserror`, `tokio-stream`, `tokio-util`, `subtle`, `tower-http`; `tower` a dev-deps).
- FIX-7: `process_outbound` devuelve el `to` (sin doble parseo).
- FIX-8: `raw` en mark_as_read + mensaje "bot inalcanzable" cuando status 0.

Backlog consciente (documentado, no bloqueante): caps de memoria en registry (M-2), security headers, warn si listen no es loopback, cargo-audit en CI.

## Decisiones de proceso

- **Arquitectura**: ADR-01 adoptó workspace hexagonal-lite 4 crates sobre research del agente arquitecto (single-crate descartado por falta de enforcement; vertical por feature descartado por basurero común).
- **Táctica delegada** al agente táctico (18 tareas); ejecución en olas paralelas con archivos disjuntos (T1+T2 → T3∥T4∥T5∥T14 → T6∥T7∥T8 → T9∥T10 → T11→T12→T13), verificación al volver del orquestador en cada ola.
- **Desviaciones aceptadas**: `serde_yaml_ng` (nombre real del crate), `wiremock =0.6.4` (0.6.5 exige rustc 1.88 por let-chains; toolchain pin 1.85), MetaResponse en nv-engine, `SseMessage::Event(Box<..>)` (clippy large_enum_variant).
- **Gate humano UI**: el usuario detectó el alcance media (T19) chateando — la UI pedía receta sin dónde adjuntarla (flujo OCR de números sin lead).

## Pendiente

- Publicación en GitHub `orlando-vazquez-career/nitro-verde` (este devlog precede al push).
- Gate de validación del veredicto: 60 días, ≥2 iteraciones reales del funnel + ≥1 bug capturado que `simulate_bot_funnel.py` no veía, o se archiva (forecasts.jsonl).
- Fase 2 (si el gate se supera): panel de estado (feature-gated), escenarios preset con seed, modo headless runner, cleanup automático.
