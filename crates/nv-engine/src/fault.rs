//! FaultPolicy: inyección de fallos on-demand en el fake-Meta (§C-4).
//!
//! Núcleo per veredicto MNEMA: 500 / 429 / delay on-demand.
//! Puro, sin tokio: el caller (nv-http) ejecuta el `Delay`. `decide()` se
//! invoca UNA vez por request entrante al fake-Meta, ANTES de parsear.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Decisión de inyección para un request entrante al fake-Meta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultDecision {
    /// Responder normal (fallo off, o la probabilidad no disparó).
    Pass,
    /// Responder de inmediato con este status (500/429) + body de error §C-3.
    Respond { status: u16 },
    /// Esperar `ms` y luego responder normal (el sleep lo ejecuta el caller).
    Delay { ms: u64 },
}

fn default_probability() -> f64 {
    1.0
}

fn is_default_probability(p: &f64) -> bool {
    *p == 1.0
}

/// Política de inyección activa. Forma EXACTA del body de `POST /api/faults`:
///
/// ```json
/// {"status":500}  {"status":429}  {"delay_ms":5000}  {"status":500,"probability":0.5}
/// ```
///
/// Un campo; `probability` opcional (default `1.0`). `{}` (= `Default`) = off.
/// Si vinieran ambos campos (fuera de contrato), `status` tiene prioridad.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaultPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
    #[serde(
        default = "default_probability",
        skip_serializing_if = "is_default_probability"
    )]
    pub probability: f64,
}

impl Default for FaultPolicy {
    /// Off: sin status ni delay (probability default 1.0, irrelevante).
    fn default() -> Self {
        Self { status: None, delay_ms: None, probability: default_probability() }
    }
}

impl FaultPolicy {
    /// Decide qué hacer con UN request entrante. Ver semántica en §C-4.
    pub fn decide(&self) -> FaultDecision {
        if self.status.is_none() && self.delay_ms.is_none() {
            return FaultDecision::Pass;
        }
        if !roll_probability(self.probability) {
            return FaultDecision::Pass;
        }
        if let Some(status) = self.status {
            return FaultDecision::Respond { status };
        }
        if let Some(ms) = self.delay_ms {
            return FaultDecision::Delay { ms };
        }
        FaultDecision::Pass
    }
}

/// Estado del generador (§0: sin `rand`; xorshift sobre `AtomicU64`
/// sembrado con `SystemTime` nanos). 0 = aún no sembrado.
static RNG_STATE: AtomicU64 = AtomicU64::new(0);

/// Incremento de Weyl (constante áurea) para avanzar el estado.
const WEYL: u64 = 0x9E37_79B9_7F4A_7C15;

fn next_random_u64() -> u64 {
    if RNG_STATE.load(Ordering::Relaxed) == 0 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(WEYL);
        // Semilla impar distinta de cero; si otro hilo ganó la carrera, da igual.
        let _ = RNG_STATE.compare_exchange(0, nanos | 1, Ordering::Relaxed, Ordering::Relaxed);
    }
    // xorshift64* sobre el valor de Weyl acumulado.
    let mut z = RNG_STATE.fetch_add(WEYL, Ordering::Relaxed).wrapping_add(WEYL);
    z ^= z >> 12;
    z ^= z << 25;
    z ^= z >> 27;
    z.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// Dispara con probabilidad `p` (clamp fuera de [0,1]: <=0 nunca, >=1 siempre).
fn roll_probability(p: f64) -> bool {
    if p <= 0.0 {
        return false;
    }
    if p >= 1.0 {
        return true;
    }
    next_random_u64() < (p * u64::MAX as f64) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_policy_off_siempre_pass() {
        let off = FaultPolicy::default();
        assert_eq!(off, FaultPolicy { status: None, delay_ms: None, probability: 1.0 });
        for _ in 0..100 {
            assert_eq!(off.decide(), FaultDecision::Pass);
        }
        // `{}` del body de POST /api/faults también es off.
        let from_json: FaultPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(from_json, off);
        for _ in 0..100 {
            assert_eq!(from_json.decide(), FaultDecision::Pass);
        }
    }

    #[test]
    fn fault_probability_1_siempre_respond_500() {
        let policy: FaultPolicy =
            serde_json::from_str(r#"{"status":500,"probability":1.0}"#).unwrap();
        for _ in 0..100 {
            assert_eq!(policy.decide(), FaultDecision::Respond { status: 500 });
        }
    }

    #[test]
    fn fault_probability_0_siempre_pass() {
        let policy: FaultPolicy =
            serde_json::from_str(r#"{"status":500,"probability":0.0}"#).unwrap();
        for _ in 0..100 {
            assert_eq!(policy.decide(), FaultDecision::Pass);
        }
    }

    #[test]
    fn fault_delay_ms_decide_delay() {
        let policy: FaultPolicy = serde_json::from_str(r#"{"delay_ms":5000}"#).unwrap();
        for _ in 0..100 {
            assert_eq!(policy.decide(), FaultDecision::Delay { ms: 5000 });
        }
    }

    #[test]
    fn fault_probability_parcial_distribuye() {
        // 0.5 sobre 1000 decisiones: caer fuera de [300,700] es ~12 sigma.
        let policy: FaultPolicy =
            serde_json::from_str(r#"{"status":429,"probability":0.5}"#).unwrap();
        let fires = (0..1000)
            .filter(|_| policy.decide() == FaultDecision::Respond { status: 429 })
            .count();
        assert!(
            (300..=700).contains(&fires),
            "esperaba ~500 disparos, hubo {fires}"
        );
    }

    #[test]
    fn fault_serde_round_trip_c4() {
        // Los 4 bodies de §C-4 + off.
        let bodies = [
            r#"{"status":500}"#,
            r#"{"status":429}"#,
            r#"{"delay_ms":5000}"#,
            r#"{"status":500,"probability":0.5}"#,
            r#"{}"#,
        ];
        for body in bodies {
            let policy: FaultPolicy = serde_json::from_str(body).unwrap();
            let json = serde_json::to_string(&policy).unwrap();
            let reparsed: FaultPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(policy, reparsed, "round-trip falló para {body}");
        }

        // Formas esperadas de cada body.
        let p: FaultPolicy = serde_json::from_str(r#"{"status":500}"#).unwrap();
        assert_eq!(
            p,
            FaultPolicy { status: Some(500), delay_ms: None, probability: 1.0 }
        );
        assert_eq!(p.decide(), FaultDecision::Respond { status: 500 });
        // Serialización minimal (skip de defaults): el JSON re-serializado
        // conserva la forma del body original.
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"status":500}"#);

        let p: FaultPolicy = serde_json::from_str(r#"{"status":500,"probability":0.5}"#).unwrap();
        assert_eq!(p.probability, 0.5);
    }
}
