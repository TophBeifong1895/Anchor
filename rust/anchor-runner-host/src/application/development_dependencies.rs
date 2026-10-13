//! Session project dependencies for ordinary sandbox commands. The Host supplies
//! the Pilot scope; the model supplies only a confined project and requirements.

#[cfg(test)]
use super::{ApplicationError, RunApplication};
use crate::dependency_provision::ResolvedRequirements;
#[cfg(test)]
use crate::dependency_provision::{DependencyAccess, resolve_requirements_with_access};
use anchor_library::{DevelopmentScope, Library};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::io::Write;
use std::path::{Component, Path, PathBuf};

#[cfg(test)]
fn invalid(error: impl std::fmt::Display) -> ApplicationError {
    ApplicationError::Invalid(error.to_string())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn normalize_project(project: &str) -> Result<String, String> {
    if project.is_empty() || project.len() > 512 || project.contains(['\0', '\\']) {
        return Err("project must be a workspace-relative directory".into());
    }
    let mut normalized = PathBuf::new();
    for part in Path::new(project).components() {
        match part {
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            _ => {
                return Err(
                    "project must be a workspace-relative directory without parent traversal"
                        .into(),
                );
            }
        }
    }
    Ok(if normalized.as_os_str().is_empty() {
        ".".into()
    } else {
        normalized.to_string_lossy().into_owned()
    })
}

fn project_root(scope: &DevelopmentScope, project: &str) -> PathBuf {
    scope
        .root()
        .join("projects")
        .join(digest(project.as_bytes()))
}

pub(super) fn legacy_preparation(
    scope: &DevelopmentScope,
    project: &str,
) -> Result<Option<DevelopmentPreparation>, String> {
    let fact = crate::assistant::read_json::<DevelopmentPreparation>(
        &project_root(scope, project).join("current.json"),
    )?;
    if let Some(fact) = &fact {
        checked_fact(fact, project)?;
    }
    Ok(fact)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DevelopmentPreparation {
    format: u32,
    project: String,
    requirements: Vec<String>,
    development_requirements: Vec<String>,
    declaration_digest: String,
    resolved: ResolvedRequirements,
}

impl DevelopmentPreparation {
    pub(super) fn interpreter_path(&self) -> &Path {
        &self.resolved.interpreter_path
    }
    pub(crate) fn result(&self, network: bool) -> Value {
        json!({"prepared":true,"project":self.project,
            "requirements":self.requirements,"development_requirements":self.development_requirements,
            "declaration_digest":self.declaration_digest,"lock_digest":self.resolved.lock_digest,
            "packages":self.resolved.packages,"interpreter":self.resolved.interpreter,"network_allowed":network})
    }
}

fn declaration_digest(requirements: &[String], development: &[String]) -> String {
    digest(
        &serde_json::to_vec(&(
            "anchor.development-dependencies.v1",
            requirements,
            development,
        ))
        .expect("declaration encoding"),
    )
}

#[cfg(test)]
struct Requirements {
    runtime: Vec<String>,
    development: Vec<String>,
    all: Vec<String>,
}

#[cfg(test)]
fn checked_requirements(
    requirements: &[String],
    development: &[String],
) -> Result<Requirements, ApplicationError> {
    if requirements.len() + development.len() > 64
        || requirements.iter().chain(development).any(|entry| {
            entry.is_empty()
                || entry.len() > 128
                || entry.trim() != entry
                || entry.chars().any(char::is_control)
        })
    {
        return Err(invalid(
            "development dependencies require at most 64 plain requirements of up to 128 characters",
        ));
    }
    let mut requirements = requirements.to_vec();
    let mut development = development.to_vec();
    requirements.sort();
    requirements.dedup();
    development.sort();
    development.dedup();
    let mut all = requirements
        .iter()
        .chain(&development)
        .cloned()
        .collect::<Vec<_>>();
    all.sort();
    all.dedup();
    if all.is_empty() {
        return Err(invalid(
            "name at least one runtime or development requirement",
        ));
    }
    Ok(Requirements {
        runtime: requirements,
        development,
        all,
    })
}

fn checked_fact(fact: &DevelopmentPreparation, project: &str) -> Result<(), String> {
    if fact.format != 1
        || fact.project != project
        || fact.declaration_digest
            != declaration_digest(&fact.requirements, &fact.development_requirements)
        || fact.resolved.lock_digest != digest(fact.resolved.lock.as_bytes())
    {
        return Err("development dependency preparation identity changed".into());
    }
    Ok(())
}

#[cfg(test)]
fn combined(requirements: &[String], development: &[String]) -> Vec<String> {
    let mut all = requirements
        .iter()
        .chain(development)
        .cloned()
        .collect::<Vec<_>>();
    all.sort();
    all.dedup();
    all
}

#[cfg(test)]
fn save_current(path: &Path, fact: &DevelopmentPreparation) -> Result<(), ApplicationError> {
    let parent = path.parent().expect("project preparation parent");
    crate::create_durable_directory(parent).map_err(invalid)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(invalid)?;
    temporary
        .write_all(&serde_json::to_vec(fact).map_err(invalid)?)
        .map_err(invalid)?;
    temporary.as_file().sync_all().map_err(invalid)?;
    temporary.persist(path).map_err(invalid)?;
    std::fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(invalid)
}

#[cfg(test)]
impl RunApplication {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare_development_dependencies(
        &self,
        library_root: &Path,
        scope: &Path,
        project: &str,
        requirements: &[String],
        development_requirements: &[String],
        access: &DependencyAccess,
    ) -> Result<DevelopmentPreparation, ApplicationError> {
        access.check().map_err(invalid)?;
        let project = normalize_project(project).map_err(invalid)?;
        let Requirements {
            runtime: requirements,
            development: development_requirements,
            all,
        } = checked_requirements(requirements, development_requirements)?;
        let declaration_digest = declaration_digest(&requirements, &development_requirements);
        let scope = Library::new(library_root)
            .development_scope(scope, true)
            .map_err(invalid)?
            .ok_or_else(|| invalid("development scope unavailable"))?;
        // The same lock order applies to preparation and deletion: scope lease,
        // then project lock, then the shared dependency writer. Selection needs
        // only the shared scope lease because the current fact switches atomically.
        let dependencies = anchor_graph_host::resolved_library_root(library_root)
            .join(anchor_graph_host::DEPENDENCIES_DIRECTORY);
        {
            let _writer = crate::plugin_dependencies::write_lock(&dependencies, access)
                .await
                .map_err(invalid)?;
            scope.register().map_err(invalid)?;
        }
        let project_root = project_root(&scope, &project);
        crate::create_durable_directory(&project_root).map_err(invalid)?;
        let _project_lease = crate::plugin_dependencies::write_lock(&project_root, access)
            .await
            .map_err(invalid)?;
        let current = project_root.join("current.json");
        let existing =
            crate::assistant::read_json::<DevelopmentPreparation>(&current).map_err(invalid)?;
        if let Some(fact) = &existing {
            checked_fact(fact, &project).map_err(invalid)?;
        }
        let provisioner = self.dependency_provisioner()?;
        let interpreter = provisioner.config.python.canonicalize().map_err(invalid)?;
        let interpreter_digest = digest(&std::fs::read(&interpreter).map_err(invalid)?);
        let resolved = match existing {
            Some(fact)
                if combined(&fact.requirements, &fact.development_requirements) == all
                    && fact.resolved.interpreter_path == interpreter
                    && fact.resolved.interpreter_digest == interpreter_digest =>
            {
                fact.resolved
            }
            _ => resolve_requirements_with_access(&provisioner.config, None, &all, access)
                .await
                .map_err(invalid)?,
        };
        crate::plugin_dependencies::prepare_development(
            &provisioner,
            library_root,
            &scope,
            &digest(project.as_bytes()),
            &resolved,
            access,
        )
        .await
        .map_err(invalid)?;
        access.check().map_err(invalid)?;
        let fact = DevelopmentPreparation {
            format: 1,
            project,
            requirements,
            development_requirements,
            declaration_digest,
            resolved,
        };
        let immutable = project_root
            .join(crate::plugin_dependencies::resolved_view_key(
                &fact.resolved,
            ))
            .join(format!("preparation-{}.json", fact.declaration_digest));
        crate::assistant::save_immutable(&immutable, &fact).map_err(invalid)?;
        save_current(&current, &fact)?;
        Ok(fact)
    }
}

pub(crate) struct LeasedDevelopmentView {
    pub(crate) view: crate::plugin_view::PreparedView,
    _scope: DevelopmentScope,
}

pub(crate) fn select_development_view(
    library_root: &Path,
    scope: &Path,
    project: &str,
) -> Result<LeasedDevelopmentView, String> {
    let project = normalize_project(project)?;
    let scope = Library::new(library_root)
        .development_scope(scope, false)
        .map_err(|error| error.to_string())?
        .ok_or("this Session has no prepared development dependencies")?;
    let fact = crate::assistant::read_json::<DevelopmentPreparation>(
        &project_root(&scope, &project).join("current.json"),
    )?
    .ok_or_else(|| format!("project `{project}` has no prepared development dependencies"))?;
    checked_fact(&fact, &project)?;
    let view = project_root(&scope, &project)
        .join(crate::plugin_dependencies::resolved_view_key(
            &fact.resolved,
        ))
        .join("view");
    let immutable = crate::assistant::read_json::<DevelopmentPreparation>(
        &view
            .parent()
            .expect("view identity parent")
            .join(format!("preparation-{}.json", fact.declaration_digest)),
    )?
    .ok_or("selected development preparation has no durable ready fact")?;
    if immutable != fact {
        return Err("selected development preparation disagrees with its ready fact".into());
    }
    if view
        .join("bin/python")
        .canonicalize()
        .map_err(|error| error.to_string())?
        != fact.resolved.interpreter_path
    {
        return Err("selected development view names a different shared interpreter".into());
    }
    if digest(&std::fs::read(&fact.resolved.interpreter_path).map_err(|error| error.to_string())?)
        != fact.resolved.interpreter_digest
    {
        return Err("shared interpreter changed after development preparation".into());
    }
    let store =
        anchor_graph_host::store_root(&anchor_graph_host::resolved_library_root(library_root));
    let observed = crate::plugin_view::referenced_store_directories(&view, &store)?;
    let expected = fact
        .resolved
        .packages
        .iter()
        .map(|package| {
            store
                .join(crate::dependency_provision::normalize_name(&package.name))
                .join(&package.version)
        })
        .collect::<std::collections::BTreeSet<_>>();
    if observed
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        != expected
    {
        return Err("selected development view does not reference its prepared package set".into());
    }
    let view =
        crate::plugin_view::PreparedView::for_candidate(&view, &store, &fact.declaration_digest)?;
    Ok(LeasedDevelopmentView {
        view,
        _scope: scope,
    })
}

pub(crate) fn release_development_scope(library_root: &Path, scope: &Path) -> Result<(), String> {
    Library::new(library_root)
        .release_development_scope(scope)
        .map_err(|error| error.to_string())?;
    // The Library tombstone now prevents any stale caller acquiring a scope
    // lease. The private tree is outside its reference scan and is Host-owned.
    let development = scope.join("development");
    match std::fs::symlink_metadata(&development) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            crate::goose_acp::pilot::directory(&development)?;
            std::fs::remove_dir_all(development).map_err(|e| e.to_string())
        }
        Ok(_) => Err("private development storage is no longer a Host-owned directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests;
