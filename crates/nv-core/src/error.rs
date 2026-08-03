//! Errores de NitroVerde (compartidos por todos los crates).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum NvError {
    /// JSON malformado o shape fuera del contrato wire (C-3: `NVParseError`).
    #[error("parse error: {0}")]
    Parse(String),

    /// El webhook target respondió no-200 (C-1: `502 {"error":"webhook_target"}`).
    #[error("webhook target responded with status {status}")]
    WebhookTarget { status: u16 },

    /// Config inválida (CLI/env/archivo, C-7).
    #[error("config error: {0}")]
    Config(String),

    /// Error en la política de inyección de fallos (C-4).
    #[error("fault error: {0}")]
    Fault(String),

    /// `conversation_id` inexistente en el registry.
    #[error("conversation not found: {0}")]
    ConversationNotFound(String),
}

/// Alias de conveniencia para resultados de dominio.
pub type NvResult<T> = Result<T, NvError>;
