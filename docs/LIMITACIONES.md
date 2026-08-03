# LIMITACIONES — NitroVerde lite (mock honesto)

NitroVerde es un **mock local** de la WhatsApp Cloud API para probar akiveo-api end-to-end sin tocar Meta. Un mock honesto declara desde el día 1 qué **NO** simula: un verde local certifica que tu código funciona contra NitroVerde, **no** contra Meta. La verificación final siempre es en staging/producción.

Cada limitación tiene severidad y workaround. Formato: `L-XX — <qué NO simula> — <por qué importa / cómo diverge del Meta real>`.

---

L-01 — **Media OUTBOUND (bot → usuario) y upload del bot** — El fake-Meta no simula que el **bot** envíe imágenes/documentos (`type:"image"`/`"document"` en `POST /messages` cae en `unsupported`, ver L-09) ni el upload `POST /{phone_number_id}/media` de la Cloud API. La media **INBOUND** (usuario → bot) SÍ se simula desde T19: adjuntar imagen/PDF dispara el webhook `type:"image"|"document"` Meta-real y el target descarga los bytes por el camino de dos pasos (`GET /graph/{version}/{media_id}` → info con `url` absoluta → `GET /media/{media_id}` → bytes), que es lo que hace `download_media` en el flujo OCR de akiveo-api. **Severidad: media** (bajó desde T19: el OCR de receta ya se ejercita end-to-end). *Workaround:* probar media outbound en staging contra Meta real.

L-02 — **Status webhooks (`sent` / `delivered` / `read`)** — NV nunca envía callbacks de estado de mensaje al target. El Meta real los emite para cada outbound; si tu código reacciona a `statuses[]`, ese código queda sin ejercitar. **Severidad: media.** *Workaround:* verificar handlers de status en staging; no asumir entrega por un 200 del fake-Meta.

L-03 — **Errores y validaciones reales de Meta** — NV acepta payloads que Meta rechazaría: templates no aprobados o inexistentes, botones con títulos >20 chars, listas con >10 filas, `to` malformados, etc. Un typo en el nombre de un template **pasa en local y falla en prod** (los templates de producción viven en Meta). **Severidad: ALTA.** *Workaround:* validar nombres de template contra Meta Business Manager antes de subir; smoke test en staging.

L-04 — **Ventana de 24 h y pricing** — NV no modela la ventana de servicio de 24 horas ni cobra/bloquea por categoría de conversación (marketing/utility/service). Meta rechaza free-form fuera de ventana; NV lo acepta siempre. **Severidad: media.** *Workaround:* respetar la ventana por diseño en el bot; verificar comportamiento fuera de ventana en staging.

L-05 — **Rate limits reales de Meta** — El `429` que NV devuelve es una decisión local de la FaultPolicy (`POST /api/faults`), no el throttling de Meta. Los límites reales (~80 msg/s por número, límites por WABA) no existen aquí. **Severidad: baja.** *Workaround:* el retry con backoff del cliente se ejercita vía fault injection; los límites de escala solo se conocen en prod.

L-06 — **`send_template` con `WHATSAPP_USE_TEMPLATES=false` nunca pega HTTP** — En akiveo-api, con `WHATSAPP_USE_TEMPLATES=false` el cliente se auto-mockea in-process: el template **nunca pasa por NV** ni por red. Los templates no se ejercitan end-to-end aunque el flujo esté verde. **Severidad: ALTA.** *Workaround:* para ejercitar templates contra NV, levantar la API con `WHATSAPP_USE_TEMPLATES=true`; si no, cubrir templates con tests unitarios del cliente.

L-07 — **Solo `messages[0]` por webhook** — NV construye payloads con exactamente un mensaje en `entry[0].changes[0].value.messages[0]`. Meta puede batchear varios mensajes (y varios `changes` / `entry`) en un solo POST. **Severidad: baja.** El webhook real de akiveo-api también lee solo `messages[0]`, así que divergen juntos — pero conviene saberlo antes de cambiar el parser. *Workaround:* ninguno necesario mientras el target mantenga esa lectura.

L-08 — **Normalización y validación de teléfonos** — NV no valida números: acepta cualquier string en `to`/`phone` y crea conversaciones al vuelo. La normalización chilena (E.164, quitar `+`, prefijo 56/9) la hace el target (akiveo-api), no NV. **Severidad: baja.** *Workaround:* si necesitas probar números inválidos, ese test es del target, no del mock.

L-09 — **Reacciones, contacts, location y tipos desconocidos caen en `unsupported`** — Si llega un `type` que NV no simula (reaction, location, contacts, sticker, etc.), responde `200` y registra un evento `kind:"unsupported"` en el transcript con el `raw` original: no paniquea, pero **no lo simula**. **Severidad: baja.** *Workaround:* revisar el transcript para detectar qué tipos está emitiendo tu código sin cobertura de mock.

L-10 — **Sesiones en memoria (sin persistencia)** — Conversaciones, transcripts y FaultPolicy viven en memoria del proceso. Reiniciar `nv` = transcripts vacíos y faults limpios. **Severidad: baja** (es un mock de desarrollo, stateless por diseño). *Workaround:* no reiniciar NV en medio de una corrida E2E; si lo haces, recrear la conversación (`POST /api/conversations`).

L-11 — **El verde local no certifica contra Meta real** — El mock se modeló leyendo la documentación de la Cloud API, no contra tráfico capturado de Meta. Divergencias de detalle (campos extra, códigos de error exactos, headers) pueden existir. **Severidad: ALTA (meta-limitación).** *Workaround:* toda feature verificada en local se re-verifica en staging/prod antes de cerrarse; si aparece una divergencia, se corrige el mock y se documenta aquí.

L-12 — **Firma HMAC siempre válida** — NV firma todos los webhooks correctamente (`X-Hub-Signature-256` con `NV_APP_SECRET`). No hay modo "firma inválida" para probar el rechazo 401 del target. **Severidad: baja.** *Workaround:* el rechazo por firma se cubre con tests unitarios del webhook en akiveo-api.

L-13 — **Media inbound sin validaciones de tamaño/tipo y store efímero** — El upload de media (T19) NO valida tamaño máximo ni el tipo real del archivo: el `mime_type` se toma del header del multipart tal cual (Meta sniffea y limita — ~5 MB imágenes, ~100 MB documentos —; NV acepta todo) y los adjuntos viven en memoria sin eviction hasta reiniciar el proceso. Lo que SÍ es real: el `sha256` de la info se calcula sobre los bytes exactos que se sirven, y la `url` de la info es absoluta según `NV_PUBLIC_URL` (si el target corre en docker, el default `http://host.docker.internal:9100` es el correcto; si cambiás `--listen`, ajustala). **Severidad: baja.** *Workaround:* validar tamaños/tipos en el target o con tests de akiveo-api; no usar NV para probar los límites de Meta.

---

**Regla de la casa:** si descubres una divergencia nueva entre NV y Meta real, no la dejes en un chat — agrega aquí su `L-XX` con severidad y workaround.
