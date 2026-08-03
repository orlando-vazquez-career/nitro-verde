//! Escenarios COMO DATOS (YAML): día 1 headless-ready, SIN runner.
//!
//! Un escenario describe un flujo de conversación contra el bot real
//! (p.ej. el funnel de referencia §C-9) como pasos `send`/`expect`. El
//! runner headless que los ejecute vendrá después (usa la arista
//! `nv` → `nv-engine`, sin nv-http); hoy los escenarios solo se cargan
//! y se validan.
//!
//! Riesgo ADR #6: `load_scenario()` es la ÚNICA función del workspace que
//! toca `serde_yaml_ng` — si el crate muere, la migración es 1 diff acá.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::conversation::UserInput;
use crate::error::NvError;

/// Escenario completo: nombre, teléfono del usuario simulado y pasos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    /// Teléfono del usuario simulado, SIN `+` (convención §C-2).
    pub phone: String,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// Un paso del flujo: qué envía el usuario y qué se espera del bot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Input del usuario (misma forma serde que el body §C-1).
    pub send: UserInput,
    /// Aserción sobre la respuesta del bot (opcional).
    #[serde(default)]
    pub expect: Option<Expect>,
}

/// Aserción declarativa sobre la respuesta del bot tras un `send`.
/// Todos los campos son opcionales; el runner decide el matching.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expect {
    /// El `kind` del evento del bot contiene este substring (p.ej. "buttons").
    #[serde(default)]
    pub kind_contains: Option<String>,
    /// El `text` del evento del bot contiene este substring (p.ej. "MATCH").
    #[serde(default)]
    pub text_contains: Option<String>,
}

/// Carga un escenario YAML desde `path`.
///
/// IO falla → `NvError::Config`; YAML/shape inválido → `NvError::Parse`
/// (el wire de escenarios no es contrato Meta: `Parse` acá significa
/// "archivo de escenario inválido").
pub fn load_scenario(path: &Path) -> Result<Scenario, NvError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| NvError::Config(format!("no se pudo leer escenario {}: {e}", path.display())))?;
    let scenario: Scenario = serde_yaml_ng::from_str(&content).map_err(|e| {
        NvError::Parse(format!("escenario YAML inválido {}: {e}", path.display()))
    })?;
    Ok(scenario)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::EventKind;

    #[test]
    fn yaml_minimo_parsea_como_datos() {
        // Forma canónica del formato (misma que scenarios/ejemplo-flujo-feliz.yaml).
        let yaml = r#"
name: mini
phone: "56900000001"
steps:
  - send:
      kind: text
      body: "4"
    expect:
      kind_contains: buttons
"#;
        let sc: Scenario = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(sc.name, "mini");
        assert_eq!(sc.phone, "56900000001");
        assert_eq!(sc.steps.len(), 1);
        assert_eq!(sc.steps[0].send, UserInput::Text { body: "4".into() });
        let expect = sc.steps[0].expect.as_ref().unwrap();
        assert_eq!(expect.kind_contains.as_deref(), Some("buttons"));
        assert_eq!(expect.text_contains, None);
    }

    #[test]
    fn step_sin_expect_es_valido() {
        let yaml = r#"
name: sin-expect
phone: "56900000001"
steps:
  - send:
      kind: button_reply
      id: motivo_precio
      title: Me pareció caro
"#;
        let sc: Scenario = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(sc.steps[0].expect, None);
        assert_eq!(sc.steps[0].send.event_kind(), EventKind::UserButtonReply);
    }

    #[test]
    fn send_button_reply_parsea_variante() {
        let yaml = r#"
kind: list_reply
id: lista_x
title: Opción X
"#;
        let input: UserInput = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(
            input,
            UserInput::ListReply {
                id: "lista_x".into(),
                title: "Opción X".into()
            }
        );
    }

    #[test]
    fn yaml_malformado_falla() {
        let r: Result<Scenario, _> = serde_yaml_ng::from_str("name: [no-cierra");
        assert!(r.is_err());
    }

    #[test]
    fn shape_invalida_falla() {
        // `steps` con un `send` sin `kind`: serde rechaza la variante.
        let yaml = r#"
name: malo
phone: "56900000001"
steps:
  - send:
      body: "4"
"#;
        let r: Result<Scenario, _> = serde_yaml_ng::from_str(yaml);
        assert!(r.is_err());
    }
}
