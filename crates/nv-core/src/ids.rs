//! Generador de IDs y timestamps (§C-6), sin `rand` (fuera del stack).
//!
//! Un `AtomicU64` global sembrado con los nanosegundos del reloj, incremento
//! por mensaje y xorshift final para opacidad. El mismo generador sirve para
//! `wamid.NV_<16hex>` y para `conv_<16hex>`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Semilla por proceso (nanos del reloj al primer uso).
static SEED: OnceLock<u64> = OnceLock::new();
/// Contador monótono global.
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn seed() -> u64 {
    *SEED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            // Fallback fijo si el reloj del sistema está roto (no debería pasar).
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
    })
}

/// Siguiente valor del generador: único por proceso (el contador lo garantiza)
/// y opaco hacia afuera (xorshift sobre la mezcla semilla + contador).
fn next_id() -> u64 {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut x = seed() ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

/// `wamid.NV_<16hex>` — id de mensaje estilo Meta (§C-6).
pub fn new_wamid() -> String {
    format!("wamid.NV_{:016x}", next_id())
}

/// `conv_<16hex>` — id de conversación (§C-6).
pub fn new_conversation_id() -> String {
    format!("conv_{:016x}", next_id())
}

/// `media_<16hex>` — id de adjunto (T19). Lo emite `MediaStore::put` y es la
/// clave de `GET /graph/{version}/{media_id}` (info) y `GET /media/{media_id}`
/// (bytes).
pub fn new_media_id() -> String {
    format!("media_{:016x}", next_id())
}

/// Timestamp wire del webhook (§C-2): unix epoch en segundos, como string.
pub fn now_timestamp() -> String {
    unix_seconds().to_string()
}

/// Unix epoch en segundos (0 si el reloj está roto).
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Timestamp RFC3339 UTC (`2026-08-02T22:00:00Z`) para `ChatEvent.ts` (§C-5).
/// Implementado a mano: chrono/time están fuera del stack fijo.
pub fn now_rfc3339() -> String {
    rfc3339_from_unix(unix_seconds())
}

/// Convierte unix seconds a `YYYY-MM-DDTHH:MM:SSZ` (algoritmo civil-from-days
/// de Howard Hinnant, dominio público).
fn rfc3339_from_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let mut year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    if month <= 2 {
        year += 1;
    }

    format!("{year:04}-{month:02}-{day:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn wamid_formato_wire() {
        let id = new_wamid();
        assert!(id.starts_with("wamid.NV_"), "id: {id}");
        let hex_part = &id["wamid.NV_".len()..];
        assert_eq!(hex_part.len(), 16, "id: {id}");
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "id: {id}"
        );
    }

    #[test]
    fn conversation_id_formato_wire() {
        let id = new_conversation_id();
        assert!(id.starts_with("conv_"), "id: {id}");
        let hex_part = &id["conv_".len()..];
        assert_eq!(hex_part.len(), 16, "id: {id}");
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "id: {id}"
        );
    }

    #[test]
    fn media_id_formato_wire() {
        let id = new_media_id();
        assert!(id.starts_with("media_"), "id: {id}");
        let hex_part = &id["media_".len()..];
        assert_eq!(hex_part.len(), 16, "id: {id}");
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "id: {id}"
        );
    }

    #[test]
    fn ids_son_unicos() {
        let ids: HashSet<String> = (0..1_000).map(|_| new_wamid()).collect();
        assert_eq!(ids.len(), 1_000);
    }

    #[test]
    fn timestamp_es_unix_plausible() {
        let ts: u64 = now_timestamp().parse().expect("timestamp numérico");
        assert!(ts > 1_700_000_000, "timestamp: {ts}");
    }

    #[test]
    fn rfc3339_vectores_conocidos() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_unix(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(rfc3339_from_unix(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(rfc3339_from_unix(1_785_700_000), "2026-08-02T19:46:40Z");
    }

    #[test]
    fn now_rfc3339_tiene_forma() {
        let ts = now_rfc3339();
        assert_eq!(ts.len(), 20, "ts: {ts}");
        assert!(ts.ends_with('Z'), "ts: {ts}");
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[10..11], "T");
    }
}
