//! Construct retired development views for production read-side regressions.

use super::*;
use crate::dependency_provision::{
    DependencyAccess, DependencyProvisioner, resolve_requirements_with_access,
};

pub(crate) async fn legacy_view(
    provisioner: &DependencyProvisioner,
    library_root: &Path,
    scope: &Path,
    project: &str,
    requirements: &[String],
    development: &[String],
    access: &DependencyAccess,
) -> Result<DevelopmentPreparation, String> {
    let project = normalize_project(project)?;
    let scope = crate::plugin_library::PluginLibrary::new(library_root)
        .development_scope(scope, true)
        .map_err(|error| error.to_string())?
        .ok_or("development scope unavailable")?;
    {
        let dependencies = anchor_graph_host::resolved_library_root(library_root)
            .join(anchor_graph_host::DEPENDENCIES_DIRECTORY);
        let _writer = crate::plugin_dependencies::write_lock(&dependencies, access)
            .await
            .map_err(|error| error.to_string())?;
        scope.register().map_err(|error| error.to_string())?;
    }
    let all = requirements
        .iter()
        .chain(development)
        .cloned()
        .collect::<Vec<_>>();
    let resolved = resolve_requirements_with_access(&provisioner.config, None, &all, access)
        .await
        .map_err(|error| error.to_string())?;
    crate::plugin_dependencies::prepare_legacy_fixture(
        provisioner,
        library_root,
        &scope,
        &digest(project.as_bytes()),
        &resolved,
        access,
    )
    .await
    .map_err(|error| error.to_string())?;
    let fact = DevelopmentPreparation {
        format: 1,
        project,
        requirements: requirements.to_vec(),
        development_requirements: development.to_vec(),
        declaration_digest: declaration_digest(requirements, development),
        resolved,
    };
    let project_root = project_root(&scope, &fact.project);
    let immutable = project_root
        .join(crate::plugin_dependencies::resolved_view_key(
            &fact.resolved,
        ))
        .join(format!("preparation-{}.json", fact.declaration_digest));
    let bytes = serde_json::to_vec(&fact).map_err(|error| error.to_string())?;
    std::fs::write(immutable, &bytes).map_err(|error| error.to_string())?;
    std::fs::write(project_root.join("current.json"), bytes).map_err(|error| error.to_string())?;
    Ok(fact)
}
