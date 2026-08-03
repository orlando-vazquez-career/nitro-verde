---
verdict_id: vrd_nitroverde_2026-08-02
date: 2026-08-02
domain: nitro-verde
question: ¿Avanzar tal cual, ajustar el diseño, o no construir NitroVerde (mock Meta WhatsApp Cloud API + chat browser localhost, Rust, MIT)?
gate_humano: accepted_with_amendment
status: accepted
---

# Veredicto — NitroVerde: construir el núcleo recortado con gate de validación a 60 días

## Decisión

**Ajustar el diseño**: construir el núcleo (cambio de base URL configurable + endpoint fake-Meta + orchestrator + UI chat mínima con replies interactivos), con firma HMAC del orchestrator y documento de limitaciones desde el día 1; diferir panel (d), escenarios (e), cleanup (f) y la publicación MIT hasta superar un gate de validación con fecha: **60 días, ≥2 iteraciones reales del funnel, ≥1 bug capturado que `simulate_bot_funnel.py` no veía** — si no, se archiva.

## Scoring agregado (5 reviewers, homogeneous:true, orden permutado)

| blind_id | Rol | Rigor | Evidencia | Blind spots | Total |
|---|---|---|---|---|---|
| k7x2 | Contrarian | 4.4 | 4.0 | 4.8 | **13.2** |
| z5n4 | Outsider | 4.2 | 4.4 | 3.6 | 12.2 |
| f2j6 | Ejecutor | 4.4 | 4.8 | 3.0 | 12.2 |
| q8w1 | Expansionista | 4.0 | 4.6 | 3.2 | 11.8 |
| m3p9 | Primeros Principios | 3.8 | 3.6 | 3.6 | 11.0 |

Clash no disparado (gap top-2 = 1.00 ≥ 0.3; σ máx por eje = 0.49 < 1.0; posiciones top convergentes).

## Razonamiento (resumen)

El Contrarian ganó el panel con mecanismos falsables (mock-espejo que da verde falso; triple acoplamiento que se pudre), pero su propia duda honesta lo limita: el stub in-process del script Python **nunca ejercita el cliente HTTP real** (retries + circuit breaker) y el mock a nivel HTTP sí. Eso inclina a construir en Rust en vez de extender el script. El consenso "base URL primero" tiene evidencia file:line. "Test numbers de Meta + ngrok" descartado por violar el requisito "sin tocar Meta". El premortem nombró el fallo dominante — muerte lenta con coartada ("todavía no lo validamos, por eso no lo publicamos") — y el gate de 60 días es su mitigación, incorporada al veredicto. Inyección de fallos (500s/429s/timeouts) es tarea del núcleo, no fase posterior.

## Amendment del gate humano (2026-08-02)

1. Investigar arquitectura escalable (concurrencia, escalabilidad, legibilidad) antes de construir — nada de monolito ilegible que se cae a los 100 users.
2. Publicar en GitHub `orlando-vazquez-career` cuando la versión lite esté lista — adelanta la publicación que el veredicto difería al gate de 60 días; **el gate de validación interna sigue vigente** para mantener o archivar el proyecto.

## Claims falsables (→ forecasts.jsonl)

| # | Claim | p | Resuelve |
|---|---|---|---|
| 1 | Núcleo E2E (fake-Meta + orchestrator + ≥1 reply interactivo round-trip) contra akiveo-api en localhost | 0.75 | 2026-08-31 |
| 2 | A 60 días del núcleo: ≥2 iteraciones reales del funnel + ≥1 bug que el script no veía | 0.45 | 2026-10-02 |
| 3 | Repo publicado como OSS (MIT) en GitHub | 0.30 | 2026-11-02 |

## Disenso registrado

- **Primeros Principios** (confianza 6/10): el núcleo debía ser Python. Descartado: el mock a nivel HTTP es lenguaje-independiente y el expertise Rust del usuario lo hace igual de barato. Su punto "MIT sin demanda" se absorbió difiriendo la publicación (luego adelantada por el humano, manteniendo el gate interno).
- **Contrarian** (7/10): extender `simulate_bot_funnel.py` era la vía. Descartado: su propio Fallo 2 (falsa confianza) aplica más fuerte al stub in-process que al mock HTTP con inyección de fallos.
- **Outsider** (7/10): vía oficial Meta no descartada con motivos. Descartado: viola "sin tocar Meta" (R2 y R4 lo marcaron).
- **Expansionista** (7/10): modo headless en CI como apuesta fuerte. Diferido, no negado: se habilita si el gate se supera.

Si alguno vuelve a aparecer en 3 meses, considerar reabrir.

## Observaciones post-hoc

_(append-only — los datos nuevos que confirmen o corrijan el veredicto se agregan acá con fecha)_
