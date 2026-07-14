use std::path::Path;

use elastos_common::{CapsuleManifest, ElastosError, Result};
use wasmparser::{ComponentTypeRef, Encoding, Parser, Payload};

const ALLOWED_BUS_IMPORTS: &[&str] = &[
    "elastos:bus/types@1.0.0",
    "elastos:bus/runtime@1.0.0",
    "elastos:bus/identity@1.0.0",
    "elastos:bus/capabilities@1.0.0",
    "elastos:bus/providers@1.0.0",
];

const FORBIDDEN_AUTHORITY_MARKERS: &[&str] = &[
    "wasi",
    "env",
    "preopen",
    "socket",
    "sock",
    "fifo",
    "fd_",
    "path_",
    "http",
    "tcp",
    "udp",
    "ELASTOS_API",
    "ELASTOS_TOKEN",
    "ELASTOS_CARRIER_FIFOS",
    "_carrier",
];

pub(crate) async fn validate_component_capsule(
    capsule_dir: &Path,
    manifest: &CapsuleManifest,
) -> Result<()> {
    if !manifest.is_component_capsule() {
        return Ok(());
    }

    let entrypoint = capsule_dir.join(&manifest.entrypoint);
    let bytes = tokio::fs::read(&entrypoint).await.map_err(|err| {
        ElastosError::InvalidManifest(format!(
            "failed to read component entrypoint {}: {}",
            manifest.entrypoint, err
        ))
    })?;
    validate_component_imports(&bytes).map_err(ElastosError::InvalidManifest)
}

fn validate_component_imports(bytes: &[u8]) -> std::result::Result<(), String> {
    let mut saw_root_header = false;

    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|err| format!("failed to parse component entrypoint: {err}"))? {
            Payload::Version { encoding, .. } if !saw_root_header => {
                saw_root_header = true;
                if encoding != Encoding::Component {
                    return Err("component entrypoint must be a WebAssembly component".to_string());
                }
            }
            Payload::ComponentImportSection(section) => {
                for import in section {
                    let import = import.map_err(|err| {
                        format!("failed to parse component import section: {err}")
                    })?;
                    let name = import.name.0;
                    if name.starts_with("import-type-") {
                        if !matches!(import.ty, ComponentTypeRef::Type(_)) {
                            return Err(format!(
                                "component generated type import \"{}\" must be a type import",
                                name
                            ));
                        }
                        continue;
                    }
                    if name == "import-func-run" {
                        if !matches!(import.ty, ComponentTypeRef::Func(_)) {
                            return Err("component generated run import must be a function import"
                                .to_string());
                        }
                        continue;
                    }
                    if !ALLOWED_BUS_IMPORTS.contains(&name) {
                        return Err(format!(
                            "component import \"{}\" is not allowed; allowed imports: {}",
                            name,
                            ALLOWED_BUS_IMPORTS.join(", ")
                        ));
                    }
                    if !matches!(import.ty, ComponentTypeRef::Instance(_)) {
                        return Err(format!(
                            "component import \"{}\" must be an interface instance import",
                            name
                        ));
                    }
                }
            }
            Payload::ImportSection(section) => {
                for import in section {
                    let import = import.map_err(|err| {
                        format!("failed to parse component core import section: {err}")
                    })?;
                    validate_core_import(import.module, import.name)?;
                }
            }
            _ => {}
        }
    }

    if !saw_root_header {
        return Err("component entrypoint is empty".to_string());
    }

    Ok(())
}

fn validate_core_import(module: &str, name: &str) -> std::result::Result<(), String> {
    let lower = format!("{module}::{name}").to_ascii_lowercase();
    if FORBIDDEN_AUTHORITY_MARKERS
        .iter()
        .any(|marker| lower.contains(&marker.to_ascii_lowercase()))
    {
        return Err(format!(
            "component core module import \"{module}::{name}\" is forbidden"
        ));
    }

    if ALLOWED_BUS_IMPORTS.contains(&module) {
        return Ok(());
    }
    if module.is_empty() && (name == "$imports" || name.bytes().all(|byte| byte.is_ascii_digit())) {
        return Ok(());
    }

    Err(format!(
        "component core module import \"{module}::{name}\" is not allowed; use elastos:bus component imports only"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        Component, ComponentImportSection, ComponentTypeRef as EncodedComponentTypeRef,
        ComponentTypeSection, InstanceType, PrimitiveValType,
    };

    fn component_with_imports(imports: &[(&str, EncodedComponentTypeRef)]) -> Vec<u8> {
        let mut types = ComponentTypeSection::new();
        types.instance(&InstanceType::new());

        let mut import_section = ComponentImportSection::new();
        for (name, ty) in imports {
            import_section.import(name, *ty);
        }

        let mut component = Component::new();
        component.section(&types);
        component.section(&import_section);
        component.finish()
    }

    #[test]
    fn accepts_allowed_component_imports() {
        let bytes = component_with_imports(&[
            (
                "elastos:bus/runtime@1.0.0",
                EncodedComponentTypeRef::Instance(0),
            ),
            (
                "elastos:bus/providers@1.0.0",
                EncodedComponentTypeRef::Instance(0),
            ),
        ]);

        validate_component_imports(&bytes).unwrap();
    }

    #[test]
    fn rejects_core_wasm_module() {
        let err = validate_component_imports(b"\0asm\x01\0\0\0").unwrap_err();
        assert!(err.contains("must be a WebAssembly component"));
    }

    #[test]
    fn rejects_extra_component_import() {
        let bytes = component_with_imports(&[(
            "wasi:filesystem/types@0.2.0",
            EncodedComponentTypeRef::Instance(0),
        )]);

        let err = validate_component_imports(&bytes).unwrap_err();
        assert!(err.contains("not allowed"));
        assert!(err.contains("elastos:bus/runtime@1.0.0"));
    }

    #[test]
    fn rejects_unimplemented_bus_import() {
        let bytes = component_with_imports(&[(
            "elastos:bus/streams@1.0.0",
            EncodedComponentTypeRef::Instance(0),
        )]);

        let err = validate_component_imports(&bytes).unwrap_err();
        assert!(err.contains("not allowed"));
        assert!(err.contains("elastos:bus/streams@1.0.0"));
    }

    #[test]
    fn rejects_nested_core_module_imports() {
        let mut module = wasm_encoder::Module::new();
        let mut types = wasm_encoder::TypeSection::new();
        types.ty().function([], []);
        module.section(&types);
        let mut imports = wasm_encoder::ImportSection::new();
        imports.import(
            "wasi_snapshot_preview1",
            "fd_write",
            wasm_encoder::EntityType::Function(0),
        );
        module.section(&imports);

        let mut component = Component::new();
        component.section(&wasm_encoder::ModuleSection(&module));

        let err = validate_component_imports(&component.finish()).unwrap_err();
        assert!(err.contains("core module import"));
        assert!(err.contains("wasi_snapshot_preview1::fd_write"));
    }

    #[test]
    fn rejects_allowed_name_with_wrong_import_kind() {
        let mut types = ComponentTypeSection::new();
        types
            .function()
            .params(std::iter::empty::<(&str, PrimitiveValType)>())
            .result(None);

        let mut imports = ComponentImportSection::new();
        imports.import(
            "elastos:bus/runtime@1.0.0",
            EncodedComponentTypeRef::Func(0),
        );

        let mut component = Component::new();
        component.section(&types);
        component.section(&imports);

        let err = validate_component_imports(&component.finish()).unwrap_err();
        assert!(err.contains("must be an interface instance import"));
    }
}
