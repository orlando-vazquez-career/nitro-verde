# NitroVerde

**NitroVerde** es un mock local de la **WhatsApp Cloud API** (fake-Meta) + simulador de usuario, para probar bots de WhatsApp (akiveo-api) end-to-end desde el día 1, sin cuenta de Meta, sin tokens y sin internet. Recibe el outbound del bot como si fuera Meta, inyecta fallos (429/500/delay) a pedido, y envía las respuestas del usuario simulado al webhook real del target con firma HMAC válida.

Un verde contra NitroVerde certifica tu código **contra el mock, no contra Meta**. Leé [`docs/LIMITACIONES.md`](docs/LIMITACIONES.md) antes de confiar en un test verde.

## Quickstart

```bash
cargo run -p nv
# → escuchando en http://127.0.0.1:9100

curl -s http://127.0.0.1:9100/api/health
# → {"status":"ok"}
```

La UI embebida se sirve en `http://127.0.0.1:9100/` (transcript en vivo vía SSE).

## Arquitectura

Workspace cargo de 4 crates, dependencias en un solo sentido (`nv` → `nv-http` → `nv-engine` → `nv-core`):

- **`nv-core`** — dominio puro: contrato wire de Meta (`meta`), conversaciones, `ChatEvent`, generador de wamids. Sin tokio/axum/reqwest.
- **`nv-engine`** — orchestrator: construye payloads webhook Meta-fieles, firma HMAC-SHA256 y los POSTea al target (único crate con reqwest). Sin axum.
- **`nv-http`** — servidor axum: endpoints `/graph/*` (fake-Meta), `/api/*` (conversaciones, faults, SSE), UI embebida. Sin reqwest.
- **`nv`** — binario: CLI (clap), config, composition root, shutdown graceful.

Detalle completo en el ADR: [`docs/plans/arquitectura/01-arquitectura-codigo-lite.md`](docs/plans/arquitectura/01-arquitectura-codigo-lite.md).

## Configuración

Precedencia: **CLI > env > archivo** (`--config <path>`, JSON).

| CLI | Env | Default |
|---|---|---|
| `--listen` | `NV_LISTEN` | `127.0.0.1:9100` |
| `--webhook-url` | `NV_WEBHOOK_URL` | `http://localhost:8002/api/v1/whatsapp/webhook` |
| `--app-secret` | `NV_APP_SECRET` | `nv-dev-secret` |
| `--phone-number-id` | `NV_PHONE_NUMBER_ID` | `NV_PHONE_ID` |
| `--public-url` | `NV_PUBLIC_URL` | `http://host.docker.internal:9100` |
| `--config <path>` | `NV_CONFIG` | (ninguno) |

`NV_APP_SECRET` debe matchear el `WHATSAPP_APP_SECRET` del target: akiveo-api valida la firma `X-Hub-Signature-256` sobre los bytes crudos del webhook. `NV_PUBLIC_URL` es la base con la que el **target** alcanza a NV: se usa para armar la `url` absoluta de la info de media (`GET /graph/{version}/{media_id}`); si el target corre en docker, el default es el correcto.

## Apuntar akiveo-api al fake-Meta

Con NitroVerde corriendo en el host y akiveo-api en docker, el contenedor ve al host como `host.docker.internal`:

```bash
cd /c/dev/webzendevtech/freelance/akiveo/akiveo-api
# .env:
WHATSAPP_CLOUD_API_BASE_URL=http://host.docker.internal:9100/graph

docker compose up -d api
```

El outbound del bot (`send_text`, `send_interactive_*`) pega HTTP real contra `http://host.docker.internal:9100/graph/v22.0/<phone_number_id>/messages`. `WHATSAPP_USE_TEMPLATES=false` es correcto para el funnel free-form — ojo: con ese flag, `send_template` se auto-mockea in-process y **nunca pasa por NV** (ver L-06 en [`docs/LIMITACIONES.md`](docs/LIMITACIONES.md)).

Para simular el usuario respondiendo:

```bash
# 1) Crear conversación (phone SIN '+'):
CONV=$(curl -s -X POST http://127.0.0.1:9100/api/conversations \
  -H 'Content-Type: application/json' -d '{"phone":"56900000001"}' \
  | grep -o '"conversation_id":"[^"]*"' | cut -d'"' -f4)

# 2) Enviar input del usuario → dispara webhook firmado al target:
curl -s -X POST http://127.0.0.1:9100/api/conversations/$CONV/messages \
  -H 'Content-Type: application/json' -d '{"kind":"text","body":"4"}'

# 3) Adjuntar una receta (imagen o PDF) → webhook type:"image"/"document":
curl -s -X POST http://127.0.0.1:9100/api/conversations/$CONV/media \
  -F kind=image -F caption="mi receta" -F file=@receta.jpg
# → {"accepted":true,"wamid":"wamid.NV_...","media_id":"media_..."}
```

El target puede entonces bajar el adjunto por el camino Meta-real de dos pasos: `GET /graph/v22.0/{media_id}` (info JSON con `url` absoluta, `mime_type`, `sha256` real y `file_size`) y `GET /media/{media_id}` (bytes crudos) — exactamente lo que hace `download_media` en el flujo OCR de akiveo-api. Desde la UI embebida también se puede: botón 📎 junto al input, preview del archivo y Enviar.

Inyección de fallos en el fake-Meta: `POST /api/faults` con `{"status":429}`, `{"status":500}` o `{"delay_ms":5000}`; `DELETE /api/faults` para limpiar.

## Docs

- [`docs/LIMITACIONES.md`](docs/LIMITACIONES.md) — qué NO simula el mock (lectura obligatoria).
- [`docs/plans/estrategia/01-lite-nucleo.md`](docs/plans/estrategia/01-lite-nucleo.md) — estrategia y alcance.
- [`docs/plans/arquitectura/01-arquitectura-codigo-lite.md`](docs/plans/arquitectura/01-arquitectura-codigo-lite.md) — ADR técnico.
- [`docs/plans/tactica/01-lite-nucleo.md`](docs/plans/tactica/01-lite-nucleo.md) — contrato de ejecución (endpoints §C-1..C-9).

## Licencia

MIT.
