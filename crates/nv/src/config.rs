//! Config del binario `nv` (§C-7): precedence CLI > env > archivo JSON.
//!
//! El archivo es JSON leído con serde_json (stack fijo; NO toml). La
//! resolución es una función pura ([`resolve`]) que recibe el env como
//! closure — los tests de precedence no tocan el env del proceso.

use std::path::Path;

use serde::Deserialize;

/// `--listen` / `NV_LISTEN`.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:9100";
/// `--webhook-url` / `NV_WEBHOOK_URL`.
pub const DEFAULT_WEBHOOK_URL: &str = "http://localhost:8002/api/v1/whatsapp/webhook";
/// `--app-secret` / `NV_APP_SECRET`.
pub const DEFAULT_APP_SECRET: &str = "nv-dev-secret";
/// `--phone-number-id` / `NV_PHONE_NUMBER_ID`.
pub const DEFAULT_PHONE_NUMBER_ID: &str = "NV_PHONE_ID";
/// `--public-url` / `NV_PUBLIC_URL`: base pública con la que el TARGET
/// alcanza a NV (la `url` absoluta de la info de media, T19). Default
/// docker: akiveo-api corre en contenedor y ve al host así.
pub const DEFAULT_PUBLIC_URL: &str = "http://host.docker.internal:9100";

/// Overrides de CLI (flags `--*` ya parseados). `None` = no pasado.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub listen: Option<String>,
    pub webhook_url: Option<String>,
    pub app_secret: Option<String>,
    pub phone_number_id: Option<String>,
    pub public_url: Option<String>,
}

/// Contenido del archivo de config JSON (`--config` / `NV_CONFIG`).
/// Todos los campos opcionales: lo ausente sigue la cadena de precedence.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FileConfig {
    pub listen: Option<String>,
    pub webhook_url: Option<String>,
    pub app_secret: Option<String>,
    pub phone_number_id: Option<String>,
    pub public_url: Option<String>,
}

/// Config efectiva, ya resuelta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub listen: String,
    pub webhook_url: String,
    pub app_secret: String,
    pub phone_number_id: String,
    pub public_url: String,
}

/// Resuelve la config efectiva: CLI > env > archivo > default (§C-7).
///
/// `env` es un lookup de variable (`NV_LISTEN`, etc.) inyectado: en prod es
/// `std::env::var`, en tests un mapa en memoria.
pub fn resolve(
    overrides: &Overrides,
    env: &dyn Fn(&str) -> Option<String>,
    file: Option<&FileConfig>,
) -> Config {
    // Cadena de precedence para un campo: CLI → env → archivo → default.
    let pick = |cli: &Option<String>, env_key: &str, file_val: Option<&String>, default: &str| {
        cli.clone()
            .or_else(|| env(env_key))
            .or_else(|| file_val.cloned())
            .unwrap_or_else(|| default.to_string())
    };
    let empty = FileConfig::default();
    let file = file.unwrap_or(&empty);
    Config {
        listen: pick(&overrides.listen, "NV_LISTEN", file.listen.as_ref(), DEFAULT_LISTEN),
        webhook_url: pick(
            &overrides.webhook_url,
            "NV_WEBHOOK_URL",
            file.webhook_url.as_ref(),
            DEFAULT_WEBHOOK_URL,
        ),
        app_secret: pick(
            &overrides.app_secret,
            "NV_APP_SECRET",
            file.app_secret.as_ref(),
            DEFAULT_APP_SECRET,
        ),
        phone_number_id: pick(
            &overrides.phone_number_id,
            "NV_PHONE_NUMBER_ID",
            file.phone_number_id.as_ref(),
            DEFAULT_PHONE_NUMBER_ID,
        ),
        public_url: pick(
            &overrides.public_url,
            "NV_PUBLIC_URL",
            file.public_url.as_ref(),
            DEFAULT_PUBLIC_URL,
        ),
    }
}

/// Lee y parsea el archivo de config JSON.
pub fn load_file(path: &Path) -> anyhow::Result<FileConfig> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("leyendo config {}: {e}", path.display()))?;
    let file: FileConfig = serde_json::from_str(&text)
        .map_err(|e| anyhow::anyhow!("parseando config JSON {}: {e}", path.display()))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Env fake: lookup sobre un mapa en memoria (sin tocar std::env).
    fn fake_env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_cuando_no_hay_nada() {
        let cfg = resolve(&Overrides::default(), &no_env, None);
        assert_eq!(cfg.listen, "127.0.0.1:9100");
        assert_eq!(cfg.webhook_url, "http://localhost:8002/api/v1/whatsapp/webhook");
        assert_eq!(cfg.app_secret, "nv-dev-secret");
        assert_eq!(cfg.phone_number_id, "NV_PHONE_ID");
        assert_eq!(cfg.public_url, "http://host.docker.internal:9100");
    }

    #[test]
    fn cli_le_gana_a_env_archivo_y_default() {
        let overrides = Overrides {
            listen: Some("127.0.0.1:9999".to_string()),
            ..Default::default()
        };
        let env = fake_env(&[("NV_LISTEN", "127.0.0.1:8888")]);
        let file = FileConfig {
            listen: Some("127.0.0.1:7777".to_string()),
            ..Default::default()
        };
        let cfg = resolve(&overrides, &env, Some(&file));
        assert_eq!(cfg.listen, "127.0.0.1:9999");
    }

    #[test]
    fn env_le_gana_a_archivo_y_default() {
        let env = fake_env(&[
            ("NV_LISTEN", "127.0.0.1:8888"),
            ("NV_APP_SECRET", "env-secret"),
        ]);
        let file = FileConfig {
            listen: Some("127.0.0.1:7777".to_string()),
            app_secret: Some("file-secret".to_string()),
            ..Default::default()
        };
        let cfg = resolve(&Overrides::default(), &env, Some(&file));
        assert_eq!(cfg.listen, "127.0.0.1:8888");
        assert_eq!(cfg.app_secret, "env-secret");
    }

    #[test]
    fn archivo_le_gana_al_default() {
        let file = FileConfig {
            webhook_url: Some("http://bot:8002/hook".to_string()),
            phone_number_id: Some("FILE_PHONE".to_string()),
            ..Default::default()
        };
        let cfg = resolve(&Overrides::default(), &no_env, Some(&file));
        assert_eq!(cfg.webhook_url, "http://bot:8002/hook");
        assert_eq!(cfg.phone_number_id, "FILE_PHONE");
        // Lo no presente en el archivo cae al default.
        assert_eq!(cfg.listen, DEFAULT_LISTEN);
        assert_eq!(cfg.app_secret, DEFAULT_APP_SECRET);
    }

    #[test]
    fn precedence_completa_campo_a_campo() {
        // listen de CLI, webhook_url de env, app_secret de archivo,
        // phone_number_id de default — cada fuente en su nivel.
        let overrides = Overrides {
            listen: Some("127.0.0.1:1111".to_string()),
            ..Default::default()
        };
        let env = fake_env(&[("NV_WEBHOOK_URL", "http://env/hook")]);
        let file = FileConfig {
            listen: Some("127.0.0.1:3333".to_string()),
            webhook_url: Some("http://file/hook".to_string()),
            app_secret: Some("file-secret".to_string()),
            phone_number_id: Some("FILE_PHONE".to_string()),
            public_url: None,
        };
        let cfg = resolve(&overrides, &env, Some(&file));
        assert_eq!(cfg.listen, "127.0.0.1:1111", "CLI > todos");
        assert_eq!(cfg.webhook_url, "http://env/hook", "env > archivo");
        assert_eq!(cfg.app_secret, "file-secret", "archivo > default");
        assert_eq!(cfg.phone_number_id, "FILE_PHONE", "archivo > default");
        assert_eq!(cfg.public_url, DEFAULT_PUBLIC_URL, "sin fuente → default");
    }

    #[test]
    fn archivo_json_forma_exacta_c7() {
        let file: FileConfig = serde_json::from_str(
            r#"{"listen":"127.0.0.1:9101","webhook_url":"http://x/hook","app_secret":"s","phone_number_id":"P"}"#,
        )
        .unwrap();
        assert_eq!(file.listen.as_deref(), Some("127.0.0.1:9101"));
        // `{}` es válido y equivale a "sin archivo".
        let empty: FileConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.listen, None);
    }
}
