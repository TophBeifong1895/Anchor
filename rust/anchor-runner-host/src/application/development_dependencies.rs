//! Session project dependencies for ordinary sandbox commands. The Host supplies
//! the Pilot scope; the model supplies only a confined project and requirements.

use crate::dependency_provision::ResolvedRequirements;
use anchor_library::{DevelopmentScope, Library};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

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
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
