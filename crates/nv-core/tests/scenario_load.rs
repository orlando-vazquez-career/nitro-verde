//! Carga el escenario de ejemplo del workspace (táctica T4).
//!
//! NOTA (riesgo ADR #6): este test NO puede nombrar la librería YAML —
//! pasa exclusivamente por `load_scenario()`, única puerta al parser.

use std::path::{Path, PathBuf};

use nv_core::conversation::UserInput;
use nv_core::error::NvError;
use nv_core::scenario::load_scenario;

fn workspace_scenario_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scenarios/ejemplo-flujo-feliz.yaml")
        .canonicalize()
        .expect("scenarios/ejemplo-flujo-feliz.yaml existe en la raíz del workspace")
}

#[test]
fn carga_ejemplo_flujo_feliz() {
    let sc = load_scenario(&workspace_scenario_path()).unwrap();

    assert_eq!(sc.name, "flujo-feliz-reactivacion-besplus");
    assert_eq!(sc.phone, "56900000001");
    assert_eq!(sc.steps.len(), 3);

    // Paso 1: texto "4" → espera botones del bot (funnel §C-9).
    assert_eq!(sc.steps[0].send, UserInput::Text { body: "4".into() });
    let e0 = sc.steps[0].expect.as_ref().unwrap();
    assert_eq!(e0.kind_contains.as_deref(), Some("buttons"));

    // Paso 2: click motivo_precio → espera buttons + gancho «MATCH».
    assert_eq!(
        sc.steps[1].send,
        UserInput::ButtonReply {
            id: "motivo_precio".into(),
            title: "Me pareció caro".into()
        }
    );
    let e1 = sc.steps[1].expect.as_ref().unwrap();
    assert_eq!(e1.kind_contains.as_deref(), Some("buttons"));
    assert_eq!(e1.text_contains.as_deref(), Some("MATCH"));

    // Paso 3: click asesoria_agendar → espera el link de agendado.
    assert_eq!(
        sc.steps[2].send,
        UserInput::ButtonReply {
            id: "asesoria_agendar".into(),
            title: "Agendamos".into()
        }
    );
    let e2 = sc.steps[2].expect.as_ref().unwrap();
    assert_eq!(e2.text_contains.as_deref(), Some("agendar"));
}

#[test]
fn archivo_inexistente_es_config_error() {
    let r = load_scenario(Path::new("no-existe-este-escenario.yaml"));
    assert!(matches!(r, Err(NvError::Config(_))), "got: {r:?}");
}

#[test]
fn yaml_roto_es_parse_error() {
    let mut path = std::env::temp_dir();
    path.push(format!("nv-scenario-roto-{}.yaml", std::process::id()));
    std::fs::write(&path, "name: [no-cierra").unwrap();

    let r = load_scenario(&path);
    std::fs::remove_file(&path).ok();

    assert!(matches!(r, Err(NvError::Parse(_))), "got: {r:?}");
}
