# Estrategia 01 — NitroVerde versión lite (núcleo)

**Fecha**: 2026-08-02 · **Origen**: veredicto MNEMA `vrd_nitroverde_2026-08-02` (gate humano aceptado con amendment) · **Estado**: aprobada para Arquitectura

## Objetivo

Construir la versión lite de **NitroVerde**: un mock de la Meta WhatsApp Cloud API + chat browser que permite conversar en vivo con el bot real de akiveo-api en localhost, sin tocar los servidores de Meta. Repo propio en `C:\dev\tools\nitro-verde`, licencia MIT, binario Rust único.

## Contexto (por qué existe esto)

- Validar flujos de WhatsApp en local es necesidad **recurrente** (el funnel del bot cambió el 2026-08-02 y se validó con `akiveo-api/scripts/simulate_bot_funnel.py`, script one-off no interactivo, 11/11 checks verdes).
- El stub in-process del script **no ejercita el cliente HTTP real** (retries + circuit breaker de `WhatsAppCloudClient`); un mock a nivel HTTP sí — esa es la justificación técnica del veredicto para construir la tool en vez de extender el script.
- Competidores OSS existentes (`fdarian/whap`, `@whatsapp-cloudapi/emulator`) cubren el endpoint mock pero **no** el chat browser con replies interactivos round-trip ni el panel de estado de la app.

## Alcance (versión lite — lo que SÍ entra)

1. **Paso 0 (en akiveo-api)**: `WHATSAPP_CLOUD_API_BASE_URL` configurable en `app/config.py` + pasaje a los 2 call sites del cliente. Rama nueva `feat/whatsapp-base-url-configurable` (NO en la rama del PR pendiente).
2. **fake-Meta endpoint**: `POST /graph/v22.0/{phone_number_id}/messages` — parsea text/interactive/template/mark_as_read, responde `wamid.NV_*` fake, broadcast a SSE.
3. **Orchestrator**: input del usuario → payload webhook Meta-like (text / button_reply / list_reply) firmado con HMAC `X-Hub-Signature-256` → POST al webhook target (config).
4. **UI chat browser**: burbujas estilo WhatsApp, botones/listas clickeables (round-trip interactivo), SSE push. HTML/JS vanilla embebido.
5. **Documento de limitaciones** ("mock honesto", precedente CivicSys L-01..L-18): qué NO simula (media, status webhooks, errores Meta, templates reales) — desde el día 1.
6. **Inyección de fallos**: endpoints del fake-Meta capaces de devolver 500/429/timeout bajo demanda (para ejercitar retries/circuit breaker del cliente) — es tarea del núcleo, no fase posterior (veredicto).
7. **Diseño headless-ready**: los escenarios se definen como datos (YAML) desde el día 1, aunque el runner headless venga después.

## Out of scope (versión lite — lo que NO entra)

- Panel de estado del bot (queries a la DB del target).
- Escenarios preset con seed de leads (la simulación manual con SQL/scripts existentes alcanza).
- Cleanup automático de datos de prueba.
- Modo headless ejecutable (solo el diseño de datos).
- Publicación/marketing OSS más allá de subir el repo a GitHub cuando la lite esté lista (amendment humano al veredicto).

## Actores y gates

- **Orquestador** (Kimi en consola): estrategia, arquitectura (ADR), táctica delegada, juicio de cierre.
- **Arquitecto** (subagente): investigación de patrones escalables (en curso).
- **Táctico** (subagente): descomposición atómica del plan aprobado.
- **Ejecutores** (coder subagentes): implementación acotada y verificada.
- **Guardrails**: code-reviewer + security-auditor + test-runner al cerrar.
- **Gate humano**: después del ADR (diseño) y antes de publicar en GitHub.

## Criterios de éxito (binarios)

1. Chat end-to-end: desde la UI de NitroVerde se conversa con el bot real de akiveo-api (docker :8002) pasando por el webhook real — transcript completo NPS → motivo → propuesta → gancho → agendar, con botones clickeables.
2. El outbound del bot (cupón, propuesta, gancho MATCH) llega al fake-Meta de NitroVerde con `WHATSAPP_CLOUD_API_BASE_URL` apuntando a él — nada sale a `graph.facebook.com`.
3. Inyección de fallo: con el fake-Meta devolviendo 500, se observa el retry del cliente real en los logs de akiveo-api.
4. `cargo test` verde + `docs/LIMITACIONES.md` publicado en el repo.

## Riesgos (del premortem MNEMA) y mitigaciones

- **Mock-espejo que da verde falso** → doc de limitaciones + inyección de fallos + la simulación Python in-process sigue viva como referencia cruzada.
- **Muerte lenta sin gate** → gate 60 días con número y fecha (2 usos reales + 1 bug capturado que el script no veía, o se archiva).
- **La UI se come el núcleo** → UI deliberadamente mínima (vanilla JS, sin framework); headless-ready como datos, no como runner.
- **Acoplamiento que se pudre** → NitroVerde no toca la DB de akiveo en la versión lite (panel diferido); dependencia única = contrato webhook + Cloud API.

## Bloqueantes conocidos

- El cambio de `WHATSAPP_CLOUD_API_BASE_URL` en akiveo-api (paso 0) — chico, pero toca repo ajeno en rama nueva.
- Credenciales/secret para HMAC en dev: el webhook de akiveo-api tiene bypass (`WHATSAPP_ENFORCE_HMAC=false`); NitroVerde firma igual para habilitar entornos enforce.

## Próximas fases

1. **Arquitectura**: ADR con la estructura escalable (investigación web del arquitecto como insumo).
2. **Táctica**: descomposición atómica (delegable al agente táctico).
3. **Ejecución**: paso 0 en akiveo-api + scaffold + fake-Meta + orchestrator + UI.
4. **Guardrails + Integration**: tests, review, doc de limitaciones, transcript de verificación.
5. **Publicación**: repo a GitHub `orlando-vazquez-career` cuando la lite cumpla los 4 criterios.
