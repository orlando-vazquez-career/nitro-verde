# Táctica 01 — NitroVerde lite (núcleo): descomposición atómica

**Fecha**: 2026-08-02 · **Inputs**: Estrategia `docs/plans/estrategia/01-lite-nucleo.md` + ADR `docs/plans/arquitectura/01-arquitectura-codigo-lite.md` (ambos aprobados) · **Estado**: lista para Orquestador

Este documento es el **contrato de ejecución**. Cada tarea es auto-contenida: un coder puede ejecutarla leyendo solo su sección + la sección §Contrato fijo que aplique. Los `done_cmd` son binarios y ejecutables en Windows / Git Bash + docker.

---

## §0. Convenciones globales (valen para TODAS las tareas)

- **Repo**: `C:/dev/tools/nitro-verde` (hoy solo tiene `docs/`). Raíz del workspace cargo.
- **Stack fijo** (ADR, no agregar deps sin reportar): axum 0.8 · tokio 1.x (features explícitas, NO `full`) · tokio-stream + tokio-util · tower 0.5 / tower-http 0.7 · serde + serde_json (`preserve_order`) · serde_yaml_ng 0.10 (nombre real en crates.io: serde_yaml_ng) (SOLO en nv-core, solo detrás de `load_scenario()`) · reqwest 0.12 rustls (SOLO en nv-engine) · hmac 0.13 + sha2 0.11 + subtle 2.6 + hex 0.4 · dashmap 6.2 · tracing + tracing-subscriber · thiserror 2 (libs) / anyhow 1 (bin) · clap 4.6 derive · rust-embed 8.12 + mime_guess 2.0. Dev: wiremock 0.6.5, http-body-util 0.1. **Edition 2024, rust-version 1.85.**
- **Grafo de crates (lo enforza Cargo.toml)**: `nv` → `nv-http` → `nv-engine` → `nv-core`; además `nv` → `nv-engine`. `nv-core` NO conoce tokio/axum/reqwest; `nv-engine` NO conoce axum; `nv-http` NO conoce reqwest.
- **Regla de guards (riesgo ADR #4)**: PROHIBIDO mantener un guard de DashMap/RwLock a través de un `.await`. Patrón obligatorio: clonar `Arc`/`Sender`, soltar guard, recién await. Aplica a T7, T9, T10, T11.
- **Puerto default de NitroVerde**: `127.0.0.1:9100` (no colisiona con 8002/5434/6379 del stack akiveo). Siempre bindear a `127.0.0.1`.
- **Sin `rand`**: el stack no lo incluye. Aleatoriedad (wamids, probabilidad de fallo) vía `AtomicU64` + `SystemTime` nanos (xorshift de 10 líneas) — ver §C-6.
- **Idioma**: código en inglés, docs y comentarios de dominio en español.
- **Prohibido**: tareas de "investigar", frameworks front, tocar la DB de akiveo desde NitroVerde, DTOs espejo (los tipos de `nv-core::meta` SON el contrato wire).

---

## §Contrato fijo

### C-1. Endpoints de NitroVerde (nv-http)

| Método y ruta | Propósito | Respuesta |
|---|---|---|
| `POST /graph/{version}/{phone_number_id}/messages` | fake-Meta: recibe outbound del bot | ver C-3 |
| `POST /api/conversations` body `{"phone":"56900000001"}` | crea conversación (phone = usuario simulado, SIN `+`) | `201 {"conversation_id":"<id>","phone":"..."}` |
| `GET /api/conversations/{id}/transcript` | snapshot completo (anti-pérdida SSE) | `200 {"conversation_id":"...","events":[ChatEvent]}` |
| `POST /api/conversations/{id}/messages` | input del usuario → webhook firmado al target | `202 {"accepted":true,"wamid":"wamid.NV_*"}`; target no-200 → `502 {"error":"webhook_target","status":N}` |
| `GET /api/conversations/{id}/events` | SSE: solo deltas + `resync` + keep-alive | ver C-5 |
| `POST /api/faults` body ver C-4 | activa inyección de fallo en fake-Meta | `200 {"ok":true}` |
| `GET /api/faults` / `DELETE /api/faults` | ver / limpiar política | `200 <policy>` / `200 {"ok":true}` |
| `GET /api/health` | healthcheck | `200 {"status":"ok"}` |
| `GET /` y `GET /assets/*` | UI embebida (rust-embed de `ui/`) | `200 text/html` etc. |

Bodies de `POST /api/conversations/{id}/messages` (input usuario):
```json
{"kind":"text","body":"4"}
{"kind":"button_reply","id":"motivo_precio","title":"Me pareció caro"}
{"kind":"list_reply","id":"lista_x","title":"Opción X"}
```

### C-2. Payload webhook INBOUND (NitroVerde → akiveo-api)

POST a `NV_WEBHOOK_URL` (default `http://localhost:8002/api/v1/whatsapp/webhook`). Header obligatorio: `X-Hub-Signature-256: sha256=<hex HMAC-SHA256(raw_body, NV_APP_SECRET)>`. akiveo-api valida firma sobre los **bytes crudos** con `WHATSAPP_APP_SECRET`; con `WHATSAPP_ENFORCE_HMAC=false` procesa igual aunque la firma no matchee (modo dev), pero NitroVerde firma SIEMPRE bien.

El webhook real lee `entry[0].changes[0].value.messages[0]` y solo soporta: `type=text`, `type=interactive` (`button_reply` / `list_reply`), `type=button` (template reply). `from` va SIN `+`. Rate limit del webhook: 60/min (nuestros flujos: ≤5 mensajes — sin problema).

Forma Meta-fiel completa (la que construye el orchestrator):
```json
{
  "object": "whatsapp_business_account",
  "entry": [{
    "id": "WABA_NV",
    "changes": [{
      "field": "messages",
      "value": {
        "messaging_product": "whatsapp",
        "metadata": {"display_phone_number": "56900000000", "phone_number_id": "NV_PHONE_ID"},
        "contacts": [{"profile": {"name": "NV User"}, "wa_id": "56900000001"}],
        "messages": [
          {"from": "56900000001", "id": "wamid.NV_<16hex>", "timestamp": "1785700000",
           "type": "text", "text": {"body": "4"}}
        ]
      }
    }]
  }]
}
```
Variantes de `messages[0]`:
- button_reply: `{"from":"...","id":"...","timestamp":"...","type":"interactive","interactive":{"type":"button_reply","button_reply":{"id":"motivo_precio","title":"Me pareció caro"}}}`
- list_reply: `{"type":"interactive","interactive":{"type":"list_reply","list_reply":{"id":"lista_x","title":"Opción X","description":""}}}` (resto de campos igual)

Referencia viva: payloads equivalentes en `git show 1ded6ba:scripts/simulate_bot_funnel.py` del repo akiveo-api (funciones `_payload`, `_texto`, `_boton`).

### C-3. Endpoint fake-Meta (akiveo-api → NitroVerde)

El cliente real (`WhatsAppCloudClient`) pega a `{base_url}/{api_version}/{phone_number_id}/messages` con header `Authorization: Bearer <token>` y `Content-Type: application/json`. Con `WHATSAPP_CLOUD_API_BASE_URL=http://host.docker.internal:9100/graph` y `WHATSAPP_API_VERSION=v22.0` (default), la ruta que recibe NV es `POST /graph/v22.0/<phone_number_id>/messages`. NV acepta **cualquier** `{version}` y `{phone_number_id}` (son path params opacos; se ignoran salvo eco en logs).

Bodies que debe parsear (los genera el cliente real):
- text: `{"messaging_product":"whatsapp","to":"56900000001","type":"text","text":{"body":"..."}}`
- interactive button: `{"...","type":"interactive","interactive":{"type":"button","body":{"text":"..."},"action":{"buttons":[{"type":"reply","reply":{"id":"motivo_precio","title":"Me pareció caro"}}]}}}`
- interactive list: `{"type":"interactive","interactive":{"type":"list","body":{"text":"..."},"action":{"button":"Ver opciones","sections":[{"title":"...","rows":[{"id":"...","title":"...","description":"..."}]}]}}}`
- interactive cta_url: `{"type":"interactive","interactive":{"type":"cta_url","body":{"text":"..."},"action":{"name":"cta_url","parameters":{"display_text":"...","url":"..."}}}}`
- template: `{"type":"template","template":{"name":"...","language":{"code":"es"},"components":[{"type":"body","parameters":[{"type":"text","text":"..."}]}]}}`
- mark_as_read: `{"messaging_product":"whatsapp","status":"read","message_id":"wamid.NV_..."}`

Respuestas:
- Éxito mensaje: `200 {"messaging_product":"whatsapp","contacts":[{"input":"<to>","wa_id":"<to>"}],"messages":[{"id":"wamid.NV_<16hex>"}]}`
- Éxito mark_as_read: `200 {"success":true}` (fiel a Meta)
- JSON malformado: `400 {"error":{"message":"...","type":"NVParseError","code":400}}`
- Fallo inyectado (ver C-4): status configurado + `{"error":{"message":"NV fault injection","type":"NVInjected","code":<status>}}`
- `type` desconocido: `200` + evento `kind:"unsupported"` en transcript (mock honesto: NO paniquea, queda registrado)

**Auto-creación de conversación**: si llega outbound para un `to` sin conversación activa, NV crea la conversación al vuelo (el outbound del bot NUNCA se pierde).

### C-4. FaultPolicy (inyección de fallos — núcleo per veredicto)

`POST /api/faults` body (un campo; `probability` opcional, default `1.0`):
```json
{"status":500}          {"status":429}          {"delay_ms":5000}          {"status":500,"probability":0.5}
```
`DELETE /api/faults` o `POST /api/faults {}` → off. Semántica: `decide()` se invoca UNA vez por request entrante al fake-Meta, ANTES de parsear; `delay_ms` espera y luego responde normal. El cliente real reintenta en 429/5xx (3 intentos, backoff 1s/2s) y loguea `[WhatsApp] reintentar status=%d attempt=%d` — eso es lo que la verificación E2E busca.

### C-5. ChatEvent + SSE (anti-pérdida: snapshot + tail)

```json
{"seq":7,"ts":"2026-08-02T22:00:00Z","direction":"from_bot|from_user|system",
 "kind":"text|buttons|list|cta_url|template|mark_as_read|user_text|user_button_reply|user_list_reply|unsupported",
 "text":"...","buttons":[{"id":"...","title":"..."}],"sections":[],"wamid":"wamid.NV_...","raw":{...}}
```
- `seq` monótono por conversación; `raw` = payload wire original (debug + mock honesto).
- UI al conectar: `GET /transcript` (snapshot) y LUEGO abre SSE (tail de deltas). SSE: `event: message\ndata: <ChatEvent JSON>\n\n`; comentario keep-alive cada 15s; ante `Lagged` del broadcast → `event: resync\ndata: {}\n\n` y la UI refetchea `/transcript`.
- Un `tokio::sync::broadcast` POR conversación, cap 256. Memoria O(conversaciones × 256).

### C-6. wamid y IDs (sin `rand`)

`wamid.NV_{:016x}` con `AtomicU64` global sembrado con `SystemTime::now()` nanos, incremento por mensaje (xorshift opcional para opacidad). Mismo generador sirve para `conversation_id` (`"conv_{:016x}"`). Vive en `nv-core` (puro, testeable).

### C-7. Config del binario `nv` (precedence: CLI > env > archivo)

| CLI | Env | Default |
|---|---|---|
| `--listen` | `NV_LISTEN` | `127.0.0.1:9100` |
| `--webhook-url` | `NV_WEBHOOK_URL` | `http://localhost:8002/api/v1/whatsapp/webhook` |
| `--app-secret` | `NV_APP_SECRET` | `nv-dev-secret` |
| `--phone-number-id` | `NV_PHONE_NUMBER_ID` | `NV_PHONE_ID` |
| `--config <path>` | `NV_CONFIG` | (ninguno; archivo JSON, leído vía serde_json — ya está en el stack; NO agregar `toml`) |

### C-8. Setup E2E compartido (lo usan T16, T17, T18 — NO repetir en cada tarea)

Stack akiveo-api: `C:/dev/webzendevtech/freelance/akiveo/akiveo-api`, servicio `api` (host `127.0.0.1:8002`), DB postgres `akiveo/akiveo` en contenedor `db`.

```bash
# 1) NitroVerde corriendo en el HOST:
cd /c/dev/tools/nitro-verde && cargo run -p nv &
NV_PID=$!; sleep 2
curl -s http://127.0.0.1:9100/api/health   # → {"status":"ok"}

# 2) akiveo-api apuntando al fake-Meta (el contenedor ve al host como host.docker.internal):
cd /c/dev/webzendevtech/freelance/akiveo/akiveo-api
grep -q '^WHATSAPP_CLOUD_API_BASE_URL=' .env \
  && sed -i 's|^WHATSAPP_CLOUD_API_BASE_URL=.*|WHATSAPP_CLOUD_API_BASE_URL=http://host.docker.internal:9100/graph|' .env \
  || echo 'WHATSAPP_CLOUD_API_BASE_URL=http://host.docker.internal:9100/graph' >> .env
docker compose up -d api && sleep 8
curl -s http://127.0.0.1:8002/api/v1/health || docker compose logs --tail 20 api

# 3) Seed + lead de prueba con teléfono conocido en estado esperando_nps:
docker compose exec api bash -c "PYTHONPATH=/app python scripts/seed_reactivacion_besplus.py"
docker compose exec db psql -U akiveo -d akiveo -c \
  "UPDATE leads_reactivacion SET whatsapp_e164='+56900000001', estado_bot='esperando_nps' \
   WHERE id = (SELECT id FROM leads_reactivacion ORDER BY created_at ASC LIMIT 1);"

# 4) Conversación NV para ese teléfono (SIN '+'):
CONV=$(curl -s -X POST http://127.0.0.1:9100/api/conversations \
  -H 'Content-Type: application/json' -d '{"phone":"56900000001"}' | grep -o '"conversation_id":"[^"]*"' | cut -d'"' -f4)
```

Notas E2E: `WHATSAPP_USE_TEMPLATES=false` es CORRECTO — las respuestas del funnel son free-form (`send_text`/`send_interactive_*`, pegan HTTP real a NV); solo `send_template` queda mockeado in-process (va a LIMITACIONES.md). Si `host.docker.internal` no resuelve: `docker compose exec api getent hosts host.docker.internal`; fallback = IP del host en la red docker. El webhook exige lead en estado NO terminal; si el mensaje cae al flujo OCR, el transcript NV queda vacío — ese es el síntoma de lead mal seedeado.

### C-9. Funnel de referencia (bot real Besplus, 11/11 verde el 2026-08-02)

Desde lead `esperando_nps`: **(1)** texto `"4"` → bot responde botones de motivo (incluye id `motivo_precio`) → **(2)** click `motivo_precio` → bot envía «Propuesta Clínica» + cierre con gancho «hacer MATCH» (buttons) → **(3)** click `asesoria_agendar` → bot envía link de agendado («asesoría estética» + `http`/`agendar`). Estado final en DB: `oferta_asesoria` o `link_asesoria_enviado`.

---

## §Tabla de tareas y paralelismo

| T | Título | Agente | Dep | Paralela con | files_touched disjuntos |
|---|--------|--------|-----|--------------|--------------|
| T1 | Scaffold workspace 4 crates | coder | — | — | raíz + crates/* |
| T2 | nv-core: contrato Meta + errores | coder | T1 | — | nv-core/src/meta, error.rs |
| T3 | nv-core: fixtures golden anti-espejo | coder | T2 | T4 T5 T14 | nv-core/fixtures, nv-core/tests |
| T4 | nv-core: conversation + ports + scenario | coder | T2 | T3 T5 T14 | nv-core/src/{conversation,ports,scenario}.rs, scenarios/ |
| T5 | nv-engine: FaultPolicy | coder | T1 | T3 T4 T14 | nv-engine/src/fault.rs |
| T6 | nv-engine: http_client + HMAC bytes crudos | coder | T4 | T7 T8 | nv-engine/src/http_client.rs, tests/hmac |
| T7 | nv-engine: registry DashMap + broadcast | coder | T4 | T6 T8 | nv-engine/src/registry.rs |
| T8 | nv-engine: fake_meta parse + wamid.NV | coder | T3 T4 | T6 T7 | nv-engine/src/fake_meta.rs |
| T9 | nv-engine: orchestrator webhook firmado | coder | T6 T7 | T10 | nv-engine/src/orchestrator.rs |
| T10 | nv-http: state + route fake_meta | coder | T5 T7 T8 | T9 | nv-http/src/{state,routes/fake_meta}.rs |
| T11 | nv-http: routes api + sse snapshot/tail | coder | T9 T10 | — | nv-http/src/routes/{api,sse}.rs |
| T12 | UI vanilla + ui_assets | coder | T11 | — | ui/, nv-http/src/ui_assets.rs |
| T13 | nv binario: CLI + config + shutdown | coder | T10 T11 T12 | — | crates/nv/ |
| T14 | LIMITACIONES.md + README | documentador | T1 | T3–T13 | docs/LIMITACIONES.md, README.md |
| T15 | Suite verde + clippy (C#4) | test-runner | T13 T14 | — | (solo fixes menores reportados) |
| T16 | E2E outbound al fake-Meta (C#2) | test-runner | T15 | — | ninguno (verificación) |
| T17 | E2E fallo 500 retry visible (C#3) | test-runner | T16 | — | ninguno (verificación) |
| T18 | E2E chat completo bot real (C#1) | test-runner | T17 | — | ninguno (verificación) |

**Cadenas estrictamente secuenciales**: T1→T2; T11→T12→T13; T15→T16→T17→T18 (E2E comparten stack docker y lead — NO paralelizar entre sí).

---

## §Tareas

### T1 — Scaffold cargo workspace (4 crates)

**Agente**: coder · **Dep**: — · **Precede a**: T2, T5, T14

**Archivos** (todos nuevos):
- `Cargo.toml` (workspace: members `crates/nv-core`, `crates/nv-engine`, `crates/nv-http`, `crates/nv`; `resolver = "3"`)
- `rust-toolchain.toml` (`channel = "1.85.0"`)
- `crates/nv-core/Cargo.toml` + `src/lib.rs`
- `crates/nv-engine/Cargo.toml` + `src/lib.rs`
- `crates/nv-http/Cargo.toml` + `src/lib.rs`
- `crates/nv/Cargo.toml` + `src/main.rs` (binario `nv`, `fn main` con `println!("nv ok")`)
- `.gitignore` (`/target`)

**Inputs**: §0 (grafo, stack, edition 2024 / rust-version 1.85). Dependencias por crate EXACTAMENTE según §0 (nv-core: serde, serde_json, serde_yaml_ng, thiserror, hex; nv-engine: + tokio, reqwest, hmac, sha2, subtle, dashmap, tokio-stream, tokio-util, tracing; nv-http: + axum, tower, tower-http, rust-embed, mime_guess; nv: + clap, anyhow, tracing-subscriber). Declarar ya las deps del stack en cada `Cargo.toml` aunque el código aún no las use (evita conflictos de versiones entre tareas paralelas). Features de tokio explícitas: `rt-multi-thread`, `macros`, `sync`, `time`, `signal` (solo donde aplique). El grafo de `[dependencies]` entre crates ES la regla hexagonal: nv-http dep nv-engine; nv-engine dep nv-core; nv dep nv-http + nv-engine.

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo check --workspace 2>&1 | tail -3 && \
cargo tree -p nv-http | grep -q 'nv-engine' && \
! cargo tree -p nv-core | grep -qE 'tokio|axum|reqwest' && \
! cargo tree -p nv-engine --depth 1 | grep -q 'axum' && \
! cargo tree -p nv-http --depth 1 | grep -q 'reqwest' && echo T1_OK
```
→ exit 0 y `T1_OK`.

**Scope** — SÍ: workspace compilable, grafo enforced. NO: código de dominio, tests, CI.

---

### T2 — nv-core: contrato Meta (tipos wire) + errores

**Agente**: coder · **Dep**: T1 · **Precede a**: T3, T4, T8

**Archivos** (todos nuevos):
- `crates/nv-core/src/meta/mod.rs` (`pub mod cloud_api; pub mod webhook;`)
- `crates/nv-core/src/meta/cloud_api.rs` — payloads OUTBOUND (C-3): enum por `type` (`text` | `interactive`{button,list,cta_url} | `template`) + variante `mark_as_read` (presence de `status:"read"`); response de éxito Meta-like; error Meta-style. Serde: untagged o manually tagged según convenga al wire EXACTO de C-3; campos desconocidos se ignoran (serde default), `recipient_type` opcional.
- `crates/nv-core/src/meta/webhook.rs` — payload INBOUND (C-2): `WebhookPayload { object, entry: Vec<Entry> }` → `changes[0].value.messages[0]`; enum `InboundMessage` = `Text{body}` | `ButtonReply{id,title}` | `ListReply{id,title,description}` | `Other`. Helper `payload.first_message() -> Option<InboundMessage>`.
- `crates/nv-core/src/error.rs` — `NvError` con thiserror: `Parse`, `WebhookTarget{status}`, `Config`, `Fault`, `ConversationNotFound`.
- `crates/nv-core/src/lib.rs` — `pub mod meta; pub mod error;`

**Inputs**: §C-2 y §C-3 contienen los JSON EXACTOS que estos tipos deben (de)serializar. Regla ADR: estos tipos SON el contrato wire — sin DTOs espejo. nv-core queda PURO (sin tokio): todo testeable síncrono.

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv-core 2>&1 | tail -3
```
→ verde. Debe incluir tests inline mínimos: round-trip serde de cada variante outbound e inbound (al menos 6 tests), construidos con los JSON literales de §C-2/C-3 copiados en el test.

**Scope** — SÍ: tipos + serde + errores + tests inline. NO: fixtures/ (T3), lógica de negocio, traits (T4).

---

### T3 — nv-core: fixtures golden anti mock-espejo

**Agente**: coder · **Dep**: T2 · **Paralela con**: T4, T5, T14 · **Precede a**: T8

**Archivos** (todos nuevos):
- `crates/nv-core/fixtures/outbound_text.json`, `outbound_interactive_buttons.json`, `outbound_interactive_list.json`, `outbound_cta_url.json`, `outbound_template.json`, `outbound_mark_as_read.json`, `outbound_response_200.json`
- `crates/nv-core/fixtures/inbound_webhook_text.json`, `inbound_webhook_button_reply.json`, `inbound_webhook_list_reply.json`
- `crates/nv-core/tests/golden_fixtures.rs`

**Inputs**: contenido de cada fixture = los JSON literales de §C-2 y §C-3 (copiados tal cual — vienen del contrato REAL del cliente y webhook de akiveo-api, no inventados: defensa estructural contra el mock-espejo del premortem). El test golden: para cada fixture, `serde_json::from_str` → tipo nv-core → assert de campos semánticos clave (p.ej. buttons extrae `[{"id":"motivo_precio","title":"Me pareció caro"}]`); para responses, serializar nv-core → comparar contra fixture con `serde_json::Value` (comparación estructural, no de string).

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv-core --test golden_fixtures 2>&1 | tail -3 && \
ls crates/nv-core/fixtures/*.json | wc -l
```
→ tests verdes Y conteo ≥ 10.

**Scope** — SÍ: fixtures + golden tests. NO: tocar `src/` de nv-core (si un tipo no parsea un fixture, es bug de T2: fix mínimo permitido SOLO en `meta/`, reportado).

---

### T4 — nv-core: conversation + ports + scenario (escenarios COMO DATOS)

**Agente**: coder · **Dep**: T2 · **Paralela con**: T3, T5, T14 · **Precede a**: T6, T7, T8

**Archivos** (todos nuevos):
- `crates/nv-core/src/conversation.rs` — `ConversationId`, `ChatEvent` (forma EXACTA de §C-5, serde), `Direction`, `EventKind`, `Button`, `Section`; `Transcript { events: Vec<ChatEvent> }` con `append` (asigna `seq` monótono) y `snapshot()`.
- `crates/nv-core/src/ports.rs` — traits: `WebhookSender` (`async fn send(&self, url: &str, body: Vec<u8>, signature: &str) -> Result<(), NvError>` — recibe BYTES ya serializados + firma ya calculada, no reserializa), `EventSink`, `Clock`. (`StatusSource` NO va: YAGNI explícito del ADR — panel es fase 2.)
- `crates/nv-core/src/scenario.rs` — `Scenario`, `Step` como datos serde (YAML); `pub fn load_scenario(path: &Path) -> Result<Scenario, NvError>` — ÚNICA función del workspace que toca serde_yaml_ng (riesgo ADR #6). Formato libre pero razonable: `name`, `phone`, `steps: [{send: {kind: text, body: "4"}, expect: {kind_contains: "buttons"}}]`.
- `crates/nv-core/src/ids.rs` — generador §C-6 (`new_wamid()`, `new_conversation_id()`, `now_timestamp()`).
- `scenarios/ejemplo-flujo-feliz.yaml` — escenario del funnel §C-9 como datos (día 1 headless-ready; NO runner).
- test inline o `crates/nv-core/tests/scenario_load.rs` — carga el YAML ejemplo.

**Inputs**: §C-5 (ChatEvent), §C-6 (ids), §C-9 (funnel), riesgo ADR #6.

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv-core 2>&1 | tail -3 && \
! grep -rn 'serde_yaml_ng' crates --include='*.rs' | grep -v 'nv-core/src/scenario.rs' | grep . && \
! grep -rn 'serde_yaml_ng' crates/*/Cargo.toml | grep -v 'nv-core' | grep . && echo T4_OK
```
→ tests verdes Y serde_yaml_ng aparece SOLO en `nv-core/src/scenario.rs` y `nv-core/Cargo.toml`.

**Scope** — SÍ: tipos, traits, generador ids, load_scenario + YAML ejemplo. NO: runner headless, implementaciones de los traits (eso es nv-engine), `StatusSource`.

---

### T5 — nv-engine: FaultPolicy (inyección de fallos)

**Agente**: coder · **Dep**: T1 · **Paralela con**: T3, T4, T14 · **Precede a**: T10

**Archivos** (todos nuevos):
- `crates/nv-engine/src/fault.rs` + tests inline

**Inputs**: §C-4. API: `FaultPolicy` (serde, forma del body de `POST /api/faults`) con `decide(&self) -> FaultDecision` donde `FaultDecision = Pass | Respond{status: u16} | Delay{ms: u64}`. Puro, sin tokio (el delay lo ejecuta el caller). Probabilidad vía xorshift sobre `AtomicU64` (§0, sin `rand`). `Default` = off (siempre `Pass`).

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv-engine fault 2>&1 | tail -3
```
→ verde. Tests obligatorios: `probability:1.0` → 100/100 `Respond(500)`; `probability:0.0` → 100/100 `Pass`; `{"delay_ms":5000}` → `Delay{5000}`; policy off → siempre `Pass`; serde round-trip del JSON de §C-4.

**Scope** — SÍ: decisión pura + serde. NO: ejecutar el delay ni enchufar a rutas (T10), endpoint HTTP (T11).

---

### T6 — nv-engine: http_client + firma HMAC sobre BYTES CRUDOS

**Agente**: coder · **Dep**: T4 · **Paralela con**: T7, T8 · **Precede a**: T9

**Archivos** (todos nuevos):
- `crates/nv-engine/src/http_client.rs` — `ReqwestWebhookSender` implementando `nv_core::ports::WebhookSender` (ÚNICO reqwest del workspace) + `pub fn sign_hmac_sha256(secret: &str, body: &[u8]) -> String` (retorna `"sha256=<hex>"`).
- `crates/nv-engine/tests/hmac_vector.rs`

**Inputs**: **riesgo ADR #2** — serializar UNA vez a `Vec<u8>`, firmar ESOS bytes, enviar ESOS bytes (prohibido firmar JSON re-serializado). El trait `WebhookSender` (T4) ya está diseñado para esto: recibe bytes + firma. Vector conocido OBLIGATORIO en test:
```
secret = "nv-dev-secret"
body   = {"object":"whatsapp_business_account"}
header = sha256=c42e43d56facd7e5489b0f2f6eaae56e6c2711f7812d33a186e804d04f9fae2c
```
(regenerable: `python -c "import hmac,hashlib; print(hmac.new(b'nv-dev-secret', b'{\"object\":\"whatsapp_business_account\"}', hashlib.sha256).hexdigest())"`). Test wiremock adicional: POST capturado, re-verificar firma sobre los bytes crudos recibidos con hmac+sha2 independiente; comparación con `subtle` no requerida aquí (somos el emisor), pero `sign` no debe hacer string-roundtrip del body.

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv-engine hmac 2>&1 | tail -3 && \
cargo tree -p nv-http --depth 1 | grep -c reqwest | grep -q '^0$' && echo T6_OK
```
→ tests verdes (vector + wiremock) Y reqwest NO aparece en nv-http.

**Scope** — SÍ: firma + sender + tests. NO: construir payloads (T9), retries (NO reintentar: NV es emisor de webhooks, el retry lo prueba del lado akiveo con faults).

---

### T7 — nv-engine: registry (DashMap + broadcast por conversación, snapshot+tail)

**Agente**: coder · **Dep**: T4 · **Paralela con**: T6, T8 · **Precede a**: T9, T10, T11

**Archivos** (todos nuevos):
- `crates/nv-engine/src/registry.rs` + tests inline

**Inputs**: §C-5 y riesgos ADR #1 y #4. `Registry { conversations: DashMap<ConversationId, Arc<Session>> }`; `Session { transcript: RwLock<Transcript> (o Mutex), tx: broadcast::Sender<ChatEvent> }` con cap **256** POR conversación (NO global). API: `create(phone)`, `get_or_create_by_phone(phone)`, `append_event(id, event) -> Result<ChatEvent>` (append a transcript + `tx.send`, ignorando error de no-subscribers), `snapshot(id) -> Vec<ChatEvent>`, `subscribe(id) -> broadcast::Receiver<ChatEvent>`. **Regla de guards**: en TODO método, clonar `Arc<Session>`/`Sender` y soltar el guard de DashMap ANTES de cualquier `.await` (riesgo ADR #4 — lo revisa code-reviewer, y el test de estrés lo ejercita). Test obligatorio de overflow: publicar 300 eventos con un receiver suscrito que no lee → `recv()` devuelve `Err(Lagged)` (el caller —T11— lo traduce a `resync`; el registry solo expone el receiver crudo). Test de estrés multi_thread: 64 tasks append + 64 tasks subscribe/snapshot concurrentes → completa < 10s sin deadlock.

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv-engine registry -- --test-threads=1 2>&1 | tail -5
```
→ verde, incluyendo `lagged_overflow` y `stress_no_deadlock`.

**Scope** — SÍ: registry + tests concurrencia. NO: SSE/axum (T11), parseo (T8).

---

### T8 — nv-engine: fake_meta (parseo outbound → ChatEvent + wamid.NV_*)

**Agente**: coder · **Dep**: T3, T4 · **Paralela con**: T6, T7 · **Precede a**: T10

**Archivos** (todos nuevos):
- `crates/nv-engine/src/fake_meta.rs` + tests inline (consumen fixtures de T3 vía `include_str!` con path relativo `../nv-core/fixtures/...` o `CARGO_MANIFEST_DIR`)

**Inputs**: §C-3, §C-5, §C-6. API: `pub fn process_outbound(body: &[u8]) -> Result<(ChatEvent, MetaResponse), NvError>` donde `ChatEvent.direction = from_bot` y `MetaResponse` = éxito con `wamid.NV_<16hex>` nuevo (o `{"success":true}` para mark_as_read). Mapeo de kinds: text→`text`, interactive button→`buttons` (+`buttons[]`), list→`list` (+`sections`), cta_url→`cta_url`, template→`template` (render best-effort: nombre + variables sustituidas en texto plano), mark_as_read→`mark_as_read`. `type` desconocido → evento `unsupported` con `raw` completo, SIN error (§C-3). JSON malformado → `NvError::Parse`.

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv-engine fake_meta 2>&1 | tail -3
```
→ verde. Tests obligatorios: los 6 tipos outbound parsean desde fixtures; wamid matchea `^wamid\.NV_[0-9a-f]{16}$`; `{"type":"reaction",...}` → `unsupported` (no panic); bytes basura → `Err(Parse)`.

**Scope** — SÍ: parseo + generación wamid + response. NO: HTTP/rutas (T10), registry (T7), FaultPolicy (T5).

---

### T9 — nv-engine: orchestrator (input usuario → payload webhook → firma → POST)

**Agente**: coder · **Dep**: T6, T7 · **Paralela con**: T10 · **Precede a**: T11

**Archivos** (todos nuevos):
- `crates/nv-engine/src/orchestrator.rs`
- `crates/nv-engine/tests/orchestrator_wiremock.rs`

**Inputs**: §C-2 (forma EXACTA del payload), §C-5 (evento `user_*` al transcript). API: `Orchestrator { sender: Arc<dyn WebhookSender>, registry: Arc<Registry>, webhook_url: String, app_secret: String, phone_number_id: String }` con `async fn send_user_input(&self, conv_id, input: UserInput) -> Result<String /*wamid*/, NvError>`. `UserInput` (serde, en nv-core o aquí — preferir nv-core si T4 no lo cubrió; si ya existe, reusar): `Text{body}` | `ButtonReply{id,title}` | `ListReply{id,title}`. Flujo: construir payload §C-2 (from = phone de la conversación SIN `+`, wamid nuevo, timestamp now) → `serde_json::to_vec` UNA vez → `sign_hmac_sha256(secret, &bytes)` → `sender.send(webhook_url, bytes, signature)` → append `ChatEvent{direction: from_user, kind: user_*}` al transcript → retornar wamid. Target responde no-2xx → `NvError::WebhookTarget{status}` (el evento de usuario queda registrado igual, con nota en `raw.error`).

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv-engine orchestrator 2>&1 | tail -3
```
→ verde. Tests wiremock obligatorios: (a) text → POST a `/api/v1/whatsapp/webhook` con header `X-Hub-Signature-256` que RE-VERIFICA contra el body crudo capturado (hmac independiente) y body que matchea estructura §C-2 con `text.body == "4"`; (b) button_reply → `interactive.button_reply.id == "motivo_precio"`; (c) list_reply; (d) target 500 → `Err(WebhookTarget{500})`.

**Scope** — SÍ: construcción payload + firma + envío + registro. NO: HTTP server, SSE, FaultPolicy.

---

### T10 — nv-http: state + route fake_meta (con FaultPolicy aplicada)

**Agente**: coder · **Dep**: T5, T7, T8 · **Paralela con**: T9 · **Precede a**: T11, T13

**Archivos** (todos nuevos):
- `crates/nv-http/src/state.rs` — `AppState { registry: Arc<Registry>, fault: Arc<RwLock<FaultPolicy>>, phone_number_id: String }` (RwLock de tokio o std con regla de guards §0).
- `crates/nv-http/src/routes/mod.rs`, `crates/nv-http/src/routes/fake_meta.rs`
- `crates/nv-http/tests/fake_meta_route.rs`

**Inputs**: §C-3, §C-4, riesgo ADR #4 (el delay de FaultPolicy es un `.await` — el guard del fault DEBE soltarse antes: clonar la decisión, soltar, recién `sleep`). Orden del handler `POST /graph/{version}/{phone_number_id}/messages`: leer body bytes → `fault.decide()` (guard corto) → si `Delay{ms}`: `tokio::time::sleep` → re-decidir como Pass; si `Respond{status}`: 500/429 + body error §C-3; si Pass: `process_outbound` (T8) → resolver conversación por `to` (`get_or_create_by_phone`, T7) → `append_event` → responder MetaResponse. Handler fino, lógica en engine.

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv-http fake_meta 2>&1 | tail -3
```
→ verde. Tests `Router::oneshot` (tower ServiceExt, sin levantar red): (a) POST text → 200 + `wamid.NV_` en body; (b) POST mark_as_read → `{"success":true}`; (c) con fault `{"status":500}` activa → 500 + `NVInjected`; (d) POST basura → 400 `NVParseError`; (e) POST text con `to` nuevo → 200 Y `registry` contiene la conversación auto-creada con 1 evento `from_bot`.

**Scope** — SÍ: route + state + wiring de fault/registry. NO: rutas /api ni SSE (T11), UI (T12), binario (T13).

---

### T11 — nv-http: routes api + SSE (snapshot+tail, keep-alive, resync)

**Agente**: coder · **Dep**: T9, T10 · **Precede a**: T12, T13

**Archivos** (todos nuevos):
- `crates/nv-http/src/routes/api.rs`, `crates/nv-http/src/routes/sse.rs`
- `crates/nv-http/tests/api_sse.rs`

**Inputs**: §C-1 (contrato EXACTO de endpoints y bodies), §C-4 (faults), §C-5 (SSE), riesgos ADR #1 y #3. `api.rs`: conversations (POST create, GET transcript), messages (POST → `orchestrator.send_user_input` → 202 con wamid; `NvError::WebhookTarget{status}` → 502), faults (POST set / GET / DELETE → mutan `AppState.fault`), health. `sse.rs`: `GET /api/conversations/{id}/events` → `Sse::new(BroadcastStream::new(registry.subscribe(id)))` mapeando `Err(Lagged)` → `Event::default().event("resync").data("{}")`, con `.keep_alive(KeepAlive::new().interval(15s).text("keep-alive"))`. El Router público de nv-http se ensambla aquí (`pub fn router(state: AppState) -> Router`) dejando `nest`/`merge` listo para que T12 monte UI y T13 solo haga wiring.

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv-http 2>&1 | tail -3
```
→ verde. Tests oneshot obligatorios: (a) POST /api/conversations → 201 con id; (b) POST messages text con `WebhookSender` mock capturante → 202 y el mock recibió bytes firmados; (c) GET transcript contiene el evento `user_text`; (d) faults: POST `{"status":429}` → GET lo refleja → DELETE → off; (e) SSE resync: inyectar overflow en el broadcast (256+ eventos con receiver vivo) y assert de que el stream emite evento `resync` (test del mapeo, no del socket); (f) health → `{"status":"ok"}`.

**Scope** — SÍ: API control + SSE + router ensamblado. NO: UI (T12), main/shutdown (T13).

---

### T12 — UI vanilla (3 archivos) + ui_assets (rust-embed)

**Agente**: coder · **Dep**: T11 · **Precede a**: T13

**Archivos** (todos nuevos):
- `ui/index.html`, `ui/app.js`, `ui/style.css` — SOLO estos 3, vanilla, sin build step (riesgo ADR #7)
- `crates/nv-http/src/ui_assets.rs` — `#[derive(RustEmbed)] #[folder = "../../ui/"]` + handler `GET /` (index.html) y `GET /assets/{*path}` con `mime_guess`; montaje en el router de T11 (edición mínima de `routes/mod.rs` o donde T11 dejó el hook — conflictos se resuelven porque T12 es lo único que toca esos puntos tras T11).

**Inputs**: §C-1, §C-5. Funcionalidad mínima: (1) campo phone + botón "nueva conversación" (POST /api/conversations); (2) al conectar: `fetch /transcript` (snapshot) y LUEGO `new EventSource(.../events)` (tail) — render de burbujas estilo WhatsApp (derecha usuario / izquierda bot, verde claro); (3) `kind:buttons` → botones clickeables que POSTean `{"kind":"button_reply",id,title}`; `kind:list` → lista desplegable clickeable (`list_reply`); (4) input texto libre → POST `{"kind":"text"}`; (5) handler `event: resync` → refetch transcript y re-render; (6) control mínimo de fault: botones "simular 500" / "limpiar fallo" (POST/DELETE /api/faults) — sirve para el criterio de éxito #3 sin curl. Dedup por `seq` (los deltas SSE se aplican sobre el snapshot ya renderizado: descartar `seq` ya vistos).

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv-http 2>&1 | tail -3 && \
ls ui/index.html ui/app.js ui/style.css && ! grep -rEl 'react|vue|svelte' ui/ | grep . && echo T12_OK
```
→ tests verdes (incluye oneshot `GET /` → 200 `text/html` que contiene `id="chat"`, y `GET /assets/app.js` → 200 `javascript`) Y 3 archivos Y cero frameworks. La verificación visual queda para T18 (gate humano).

**Scope** — SÍ: chat round-trip completo contra la API fija. NO: estilos elaborados, panel de estado del bot (out of scope estratégico), framework front. Si sentís que el vanilla "no alcanza" → PARAR y reportar (señal de API de control incompleta, riesgo ADR #7).

---

### T13 — nv binario: clap + config + tracing + wiring + graceful shutdown

**Agente**: coder · **Dep**: T10, T11, T12 · **Precede a**: T15

**Archivos** (todos nuevos):
- `crates/nv/src/main.rs` — clap derive (§C-7), tracing_subscriber (fmt, env-filter `NV_LOG`/`RUST_LOG`, default info), wiring: Registry → Orchestrator (ReqwestWebhookSender) → AppState → router → `axum::serve` con `tokio::net::TcpListener`.
- `crates/nv/src/config.rs` — resolución CLI > env > archivo JSON (serde_json; §C-7) + tests unitarios de precedence.
- `crates/nv/tests/shutdown.rs` — test de graceful shutdown.

**Inputs**: §C-7, riesgo ADR #3 (SSE NO debe bloquear el apagado): `CancellationToken` (tokio-util) + `axum::serve(...).with_graceful_shutdown(...)`. El keep-alive de 15s (T11) garantiza que los streams SSE tengan oportunidad de cerrarse; el test lo verifica: levantar server en puerto efímero con el token, conectar un cliente SSE (reqwest stream), cancelar el token → el handle del server completa en < 2s y el stream del cliente termina. `ctrl_c` (tokio::signal) dispara la cancelación en main.

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test -p nv 2>&1 | tail -3 && \
(cargo run -p nv -- --listen 127.0.0.1:9105 & echo $! > /tmp/nv.pid; sleep 4; \
 curl -s http://127.0.0.1:9105/api/health; kill $(cat /tmp/nv.pid))
```
→ `cargo test -p nv` verde (config precedence + shutdown < 2s con SSE conectado) Y el curl responde `{"status":"ok"}`.

**Scope** — SÍ: composition root completo. NO: lógica de dominio, Dockerfile, CI.

---

### T14 — LIMITACIONES.md + README (mock honesto, desde el día 1)

**Agente**: documentador · **Dep**: T1 · **Paralela con**: T3–T13 · **Precede a**: T15

**Archivos** (todos nuevos):
- `docs/LIMITACIONES.md`
- `README.md`

**Inputs**: estrategia §5 y §"Out of scope"; ADR §YAGNI. Formato precedente CivicSys: lista numerada `L-01 — <qué NO simula> — <por qué importa / cómo diverge del Meta real>`. Contenido MÍNIMO obligatorio (cada uno verificable contra código): L-01 media (download/upload); L-02 status webhooks (sent/delivered/read callbacks); L-03 errores/validaciones reales de Meta (acepta payloads que Meta rechazaría, p.ej. templates no aprobados); L-04 ventana 24h y pricing; L-05 rate limits reales (el 429 inyectado es decisión local, no de Meta); L-06 `send_template` de akiveo-api con `WHATSAPP_USE_TEMPLATES=false` NO pega HTTP (mock in-process del cliente) → esos mensajes nunca pasan por NV; L-07 solo `messages[0]` por webhook (Meta puede batch); L-08 normalización de teléfonos chilena la hace el target, NV no valida números; L-09 reacciones/typed/unknown types caen en `unsupported` (se registran, no se simulan); L-10 sesiones en memoria (reinicio = transcript vacío). README: qué es, quickstart (`cargo run -p nv`), tabla de config §C-7, cómo apuntar akiveo-api (§C-8 paso 2), link a LIMITACIONES.md, licencia MIT.

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && test -f docs/LIMITACIONES.md && test -f README.md && \
[ "$(grep -cE '^L-[0-9]+' docs/LIMITACIONES.md)" -ge 10 ] && \
grep -q 'host.docker.internal' README.md && echo T14_OK
```
→ `T14_OK` con ≥ 10 limitaciones numeradas.

**Scope** — SÍ: los 2 documentos. NO: marketing OSS, badges, docs de arquitectura (ya existe el ADR).

---

### T15 — Suite verde completa + clippy (criterio de éxito #4, parte tests)

**Agente**: test-runner · **Dep**: T13, T14 · **Precede a**: T16

**Archivos**: ninguno nuevo (fixes menores SOLO si un test falla por causa obvia y acotada; cualquier fix > 20 líneas → reportar al orquestador, no arreglar).

**Criterio de terminación (done_cmd)**:
```bash
cd /c/dev/tools/nitro-verde && cargo test --workspace 2>&1 | grep -E 'test result' && \
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2 && \
test -f docs/LIMITACIONES.md && echo T15_OK
```
→ todos los `test result: ok`, clippy exit 0 sin warnings, LIMITACIONES.md presente. Reportar resumen: N tests por crate, warnings encontrados y su fix.

**Scope** — SÍ: correr, reportar, fixes triviales. NO: agregar cobertura nueva, refactors.

---

### T16 — E2E: outbound del bot llega al fake-Meta (criterio de éxito #2)

**Agente**: test-runner · **Dep**: T15 · **Precede a**: T17, T18

**Archivos**: ninguno (verificación pura; opcional guardar evidencia en `docs/devlogs/2026-08-02-e2e-lite.md`).

**Inputs**: §C-8 (setup COMPLETO — ejecutar los 4 pasos) y §C-9. **El paso 0 de akiveo-api YA ESTÁ HECHO** (rama `feat/whatsapp-base-url-configurable`, `WHATSAPP_CLOUD_API_BASE_URL` configurable) — solo se USA, no se toca código de akiveo-api.

**Criterio de terminación (done_cmd)** (tras §C-8 pasos 1–4):
```bash
curl -s -X POST http://127.0.0.1:9100/api/conversations/$CONV/messages \
  -H 'Content-Type: application/json' -d '{"kind":"text","body":"4"}' && sleep 6 && \
curl -s http://127.0.0.1:9100/api/conversations/$CONV/transcript | grep -q '"direction":"from_bot"' && \
curl -s http://127.0.0.1:9100/api/conversations/$CONV/transcript | grep -q 'motivo_precio' && echo T16_OK
```
→ `T16_OK`: el outbound del bot (botones de motivo) llegó al fake-Meta vía `WHATSAPP_CLOUD_API_BASE_URL` — nada salió a `graph.facebook.com` (la URL del cliente apunta a NV por construcción; assert adicional: `docker compose logs --since 5m api 2>&1 | grep -i 'graph.facebook.com'` vacío). Si el transcript queda vacío → lead mal seedeado (§C-8 nota) o `host.docker.internal` no resuelve (fallback §C-8).

**Scope** — SÍ: verificar. NO: modificar akiveo-api, seedear escenarios nuevos, cleanup (out of scope).

---

### T17 — E2E: inyección de fallo 500 con retry visible (criterio de éxito #3)

**Agente**: test-runner · **Dep**: T16 · **Precede a**: T18

**Archivos**: ninguno (mismo devlog de evidencia que T16).

**Inputs**: §C-4, §C-8. El cliente real reintenta 429/5xx hasta 3 veces (backoff 1s/2s) y loguea `[WhatsApp] reintentar status=%d attempt=%d`.

**Criterio de terminación (done_cmd)** (stack y conversación de T16 vivos):
```bash
curl -s -X POST http://127.0.0.1:9100/api/faults -H 'Content-Type: application/json' -d '{"status":500}' && \
curl -s -X POST http://127.0.0.1:9100/api/conversations/$CONV/messages \
  -H 'Content-Type: application/json' -d '{"kind":"button_reply","id":"motivo_precio","title":"Me pareció caro"}' && \
sleep 10 && \
cd /c/dev/webzendevtech/freelance/akiveo/akiveo-api && \
[ "$(docker compose logs --since 3m api 2>&1 | grep -c 'reintentar status=500')" -ge 2 ] && echo RETRIES_OK && \
curl -s -X DELETE http://127.0.0.1:9100/api/faults && \
curl -s -X POST http://127.0.0.1:9100/api/conversations/$CONV/messages \
  -H 'Content-Type: application/json' -d '{"kind":"text","body":"4"}' && sleep 6 && \
curl -s http://127.0.0.1:9100/api/conversations/$CONV/transcript | grep -q '"direction":"from_bot"' && echo T17_OK
```
→ `RETRIES_OK` (≥ 2 líneas de retry visibles en logs de akiveo-api mientras el fake-Meta devolvía 500) Y tras limpiar el fallo el outbound vuelve a llegar (`T17_OK`). Nota: el lead ya avanzó de estado en T16 — el texto "4" post-clear puede no re-disparar botones; el assert post-clear es solo presencia de ALGÚN `from_bot` en transcript (ya existe de T16); lo crítico es `RETRIES_OK`.

**Scope** — SÍ: verificar. NO: tunear retries de akiveo, probar circuit breaker (necesita ≥10 muestras — fuera de alcance del criterio).

---

### T18 — E2E: chat completo con bot real (criterio de éxito #1) + gate humano UI

**Agente**: test-runner · **Dep**: T17

**Archivos**: `docs/devlogs/2026-08-02-e2e-lite.md` (evidencia: transcript completo pegado + resultado psql + checklist UI).

**Inputs**: §C-8, §C-9 (funnel EXACTO). Setup fresco recomendado: lead nuevo `+56900000002` (mismo UPDATE de §C-8 paso 3 con ese número y `estado_bot='esperando_nps'`) + conversación NV nueva, para un transcript limpio del funnel completo.

**Criterio de terminación (done_cmd)**:
```bash
# Con $CONV del lead +56900000002:
curl -s -X POST http://127.0.0.1:9100/api/conversations/$CONV/messages -H 'Content-Type: application/json' -d '{"kind":"text","body":"4"}' && sleep 6 && \
curl -s -X POST http://127.0.0.1:9100/api/conversations/$CONV/messages -H 'Content-Type: application/json' -d '{"kind":"button_reply","id":"motivo_precio","title":"Me pareció caro"}' && sleep 6 && \
curl -s -X POST http://127.0.0.1:9100/api/conversations/$CONV/messages -H 'Content-Type: application/json' -d '{"kind":"button_reply","id":"asesoria_agendar","title":"Agendamos"}' && sleep 6 && \
T=$(curl -s http://127.0.0.1:9100/api/conversations/$CONV/transcript) && \
echo "$T" | grep -q 'motivo_precio' && \
echo "$T" | grep -q 'Propuesta Cl' && \
echo "$T" | grep -q 'hacer MATCH' && \
echo "$T" | grep -qiE 'asesor.a est.tica|agendar' && \
cd /c/dev/webzendevtech/freelance/akiveo/akiveo-api && \
docker compose exec db psql -U akiveo -d akiveo -tAc \
  "SELECT estado_bot FROM leads_reactivacion WHERE whatsapp_e164='+56900000002'" \
  | grep -qE 'oferta_asesoria|link_asesoria_enviado' && echo T18_API_OK
```
→ `T18_API_OK`: transcript completo NPS → motivo → propuesta → gancho → agendar, con estado final del lead correcto en DB. **Gate humano pendiente**: abrir `http://127.0.0.1:9100/` en browser, repetir un paso del funnel clickeando botones (round-trip interactivo real desde la UI, no curl) y aprobar visualmente burbujas + SSE en vivo. El orquestador reporta `T18_API_OK` + checklist UI al humano.

**Scope** — SÍ: verificación API + evidencia + checklist UI para el humano. NO: Playwright/automatización de browser (no está en el stack de este ciclo), cleanup de leads (out of scope).

---

## §Cobertura de criterios de éxito (Estrategia §Criterios)

| # | Criterio estratégico | Tarea(s) que lo verifican |
|---|---|---|
| 1 | Chat E2E con bot real, transcript completo con botones clickeables | **T18** (+ gate humano UI) |
| 2 | Outbound del bot llega al fake-Meta, nada sale a graph.facebook.com | **T16** |
| 3 | Fallo 500 inyectado → retry del cliente real visible en logs | **T17** |
| 4 | `cargo test` verde + `docs/LIMITACIONES.md` publicado | **T15** (tests) + **T14** (doc) |

## §Cobertura de riesgos ADR mitigados en CÓDIGO

| Riesgo ADR | Mitigación | Tarea | done_cmd específico |
|---|---|---|---|
| #1 broadcast `Lagged` pierde mensajes | cap 256/conv + snapshot+tail + evento `resync` | T7 (overflow test), T11 (mapeo SSE→resync), T12 (UI refetch) | `cargo test -p nv-engine registry` (lagged_overflow) + test (e) de T11 |
| #2 firmar JSON re-serializado | serializar 1 vez a Vec<u8>, firmar esos bytes, enviar esos bytes; vector conocido | T6 (+ consumido por T9) | `cargo test -p nv-engine hmac` (assert `sha256=c42e43d5...`) |
| #3 SSE bloquea graceful shutdown | CancellationToken + keep-alive 15s + test de apagado < 2s con stream conectado | T13 (+ keep-alive en T11) | `cargo test -p nv shutdown` |
| #4 guards DashMap/RwLock a través de `.await` | regla §0 (clonar Arc/Sender, soltar, await) + test de estrés multi_thread | T7 (stress), T10 (delay sin guard), revisado en T9/T11 | `cargo test -p nv-engine registry` (stress_no_deadlock) + `cargo clippy --workspace -- -D warnings` (T15) |
| #6 serde_yaml_ng bus-factor | YAML aislado detrás de `load_scenario()` | T4 | grep exclusividad en done_cmd de T4 |
| #7 UI vanilla se come el núcleo | 3 archivos sin build; "necesito framework" = parar y reportar | T12 | `! grep -rEl 'react\|vue\|svelte' ui/` |
| (premortem) mock-espejo da verde falso | fixtures del contrato REAL (extraídos del cliente/webhook de akiveo-api) | T3 | `cargo test -p nv-core --test golden_fixtures` |

## §Orden de lanzamiento sugerido (al Orquestador)

1. **T1** (solo) → **T2** (solo). Cadena crítica, todo lo demás cuelga de acá.
2. Ola paralela (4 agentes): **T3 ∥ T4 ∥ T5 ∥ T14**.
3. Ola paralela (3 agentes): **T6 ∥ T7 ∥ T8**.
4. Ola paralela (2 agentes): **T9 ∥ T10**.
5. Secuencial: **T11 → T12 → T13**.
6. **T15** (suite + clippy).
7. Secuencial E2E (mismo stack docker): **T16 → T17 → T18**. Gate humano al final de T18 (UI) y antes de publicar a GitHub.

## §Dudas detectadas (reporto, NO resuelvo)

1. **`simulate_bot_funnel.py` ya no existe en el working tree de akiveo-api** (fue eliminado; la estrategia lo cita como referencia viva). Sigue recuperable vía `git show 1ded6ba:scripts/simulate_bot_funnel.py`. Los fixtures de T3 se basan en los payloads de ese script + el código VIGENTE del webhook/cliente, así que no bloquea; pero conviene decidir si se restaura el script en akiveo-api como referencia cruzada (la estrategia lo menciona como mitigación anti mock-espejo).
2. **Puerto 9100 y formato de config JSON** no están especificados en estrategia/ADR — los fijé yo (conservador: libre, bindeado a 127.0.0.1, JSON porque serde_json ya está en el stack y `toml` no). Si el orquestador prefiere otro puerto/formato, es cambio de 2 líneas en T13.
3. **`send_template` mockeado in-process** (`WHATSAPP_USE_TEMPLATES=false`): los templates nunca ejercitan NV en la config actual del stack. El E2E lo evita (funnel free-form), y queda documentado como L-06; pero si se quiere cubrir el template de outreach inicial por NV habría que setear `WHATSAPP_USE_TEMPLATES=true` en el stack de prueba — decisión de producto/QA, no táctica.
4. **Verificación UI (T18) sin playwright-tester**: el stack de agentes de este ciclo es coder/test-runner/documentador; la verificación visual queda como checklist manual para el gate humano. Si se quiere screenshot automatizado, hace falta sumar ese agente.
