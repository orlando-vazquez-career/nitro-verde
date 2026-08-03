//! MediaStore (T19): adjuntos del usuario simulado en memoria.
//!
//! El endpoint multipart (`POST /api/conversations/{id}/media`, nv-http)
//! guarda acá los bytes crudos; el target (akiveo-api) los descubre después
//! por el camino Meta-real de dos pasos: `GET /graph/{version}/{media_id}`
//! (info: url absoluta, mime, sha256, file_size) y `GET /media/{media_id}`
//! (bytes crudos). Es el download que hace `download_media(media_id)` del
//! cliente WhatsApp de akiveo-api en su flujo OCR.
//!
//! Regla de guards (riesgo ADR #4): todo el API es sync y devuelve datos
//! owned (`MediaEntry` clonado); ningún guard de DashMap cruza un `.await`
//! porque acá no hay `.await`. El sha256 se calcula UNA vez al guardar
//! (`put`), sobre los bytes exactos que después se sirven.
//!
//! Limitación declarada (L-13): no hay validación de tamaño ni de tipo de
//! archivo, y el store crece en memoria sin eviction (es un mock de
//! desarrollo: los adjuntos viven lo que vive el proceso).

use dashmap::DashMap;

use nv_core::ids;

/// Entrada guardada: bytes crudos + mime declarado + sha256 real de los bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaEntry {
    pub bytes: Vec<u8>,
    pub mime: String,
    /// Hex lowercase del SHA-256 de `bytes` (como el `sha256` que Meta
    /// devuelve en la info de media).
    pub sha256: String,
}

impl MediaEntry {
    /// Tamaño en bytes (`file_size` de la info Meta-like).
    pub fn file_size(&self) -> usize {
        self.bytes.len()
    }
}

/// Store en memoria de adjuntos, clave `media_<16hex>` (§C-6 via `ids`).
#[derive(Debug, Default)]
pub struct MediaStore {
    inner: DashMap<String, MediaEntry>,
}

impl MediaStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Guarda los bytes y devuelve el `media_id` generado. El sha256 se
    /// calcula acá, una sola vez, sobre los bytes exactos que se sirven.
    pub fn put(&self, bytes: Vec<u8>, mime: impl Into<String>) -> String {
        use sha2::Digest;
        let sha256 = hex::encode(sha2::Sha256::digest(&bytes));
        let media_id = ids::new_media_id();
        self.inner.insert(
            media_id.clone(),
            MediaEntry {
                bytes,
                mime: mime.into(),
                sha256,
            },
        );
        media_id
    }

    /// Copia owned de la entrada (el guard de DashMap muere antes de
    /// devolver — jamás cruza un `.await` del caller, riesgo ADR #4).
    pub fn get(&self, media_id: &str) -> Option<MediaEntry> {
        self.inner.get(media_id).map(|entry| entry.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_roundtrip_bytes_mime_y_sha256_real() {
        let store = MediaStore::new();
        // sha256("abc") — vector conocido, regenerable con:
        // `python -c "import hashlib; print(hashlib.sha256(b'abc').hexdigest())"`
        let media_id = store.put(b"abc".to_vec(), "image/jpeg");

        assert!(media_id.starts_with("media_"), "media_id: {media_id}");
        assert_eq!(media_id.len(), "media_".len() + 16, "media_id: {media_id}");

        let entry = store.get(&media_id).expect("recién guardado");
        assert_eq!(entry.bytes, b"abc");
        assert_eq!(entry.mime, "image/jpeg");
        assert_eq!(
            entry.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(entry.file_size(), 3);
    }

    #[test]
    fn ids_son_unicos_y_get_desconocido_es_none() {
        let store = MediaStore::new();
        let a = store.put(b"uno".to_vec(), "application/pdf");
        let b = store.put(b"dos".to_vec(), "application/pdf");
        assert_ne!(a, b, "cada adjunto lleva media_id nuevo (§C-6)");
        // un id desconocido no pisa ni confunde entradas
        assert_eq!(store.get("media_0000000000000000"), None);
        // y los dos coexisten con sus bytes exactos
        assert_eq!(store.get(&a).unwrap().bytes, b"uno");
        assert_eq!(store.get(&b).unwrap().bytes, b"dos");
    }

    #[test]
    fn get_devuelve_copia_owned() {
        let store = MediaStore::new();
        let media_id = store.put(b"original".to_vec(), "image/png");
        let mut entry = store.get(&media_id).unwrap();
        entry.bytes.clear();
        // Mutar la copia no toca lo guardado.
        assert_eq!(store.get(&media_id).unwrap().bytes, b"original");
    }
}
