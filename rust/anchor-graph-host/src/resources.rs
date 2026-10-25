//! Filesystem resource inventory and containment shared by bundle and Plugin readers.

use crate::{MutablePaths, invalid};
use anchor_runtime::graph::GraphError;
pub(crate) use anchor_runtime::identifier::is_plugin_id as valid_reference;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

pub(crate) fn read_object(path: &Path) -> Result<serde_json::Map<String, Value>, GraphError> {
    read_object_bytes(path).map(|(object, _)| object)
}

pub(crate) fn read_object_bytes(
    path: &Path,
) -> Result<(serde_json::Map<String, Value>, Vec<u8>), GraphError> {
    let bytes = std::fs::read(path)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(GraphError::SnapshotDecode)?;
    let object = value
        .as_object()
        .cloned()
        .ok_or_else(|| invalid(format!("{} must contain a JSON object", path.display())))?;
    Ok((object, bytes))
}
pub(crate) fn reject_symlink_components(root: &Path, path: &Path) -> Result<(), GraphError> {
    let root = root.canonicalize()?;
    if !path.starts_with(&root) {
        return Err(invalid("Plugin path escapes catalog root".into()));
    }
    let mut current = root.clone();
    for component in path.strip_prefix(&root).unwrap().components() {
        if let Component::Normal(part) = component {
            current.push(part);
            if std::fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(invalid(format!(
                    "Plugin symlinks are not supported: {}",
                    current.display()
                )));
            }
        }
    }
    Ok(())
}
fn collect_files(
    root: &Path,
    current: &Path,
    mutable: &MutablePaths,
    out: &mut Vec<String>,
) -> Result<(), GraphError> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if mutable.covers(&relative_of(root, &path)) {
            continue;
        }
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            return Err(invalid(format!(
                "Plugin symlinks are not supported: {}",
                path.display()
            )));
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".env" || name.starts_with(".env.") || name == ".mcp-auth" {
            return Err(invalid(format!(
                "Plugin-local credentials are not supported: {}",
                path.display()
            )));
        }
        if ty.is_dir() {
            collect_files(root, &path, mutable, out)?;
        } else if ty.is_file() {
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

/// The Plugin-relative slash path of one entry, as `covers` expects.
pub(crate) fn relative_of(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn inside_path(base: &Path, relative: &str, require_file: bool) -> Result<PathBuf, String> {
    let candidate = Path::new(relative);
    let base = base
        .canonicalize()
        .map_err(|error| format!("Plugin directory cannot be resolved: {error}"))?;
    let joined = base.join(candidate);
    reject_symlink_components(&base, &joined).map_err(|error| error.to_string())?;
    let resolved = base
        .join(candidate)
        .canonicalize()
        .map_err(|error| format!("resource cannot be resolved: {error}"))?;
    if !resolved.starts_with(&base) {
        return Err("resource escapes the Plugin bundle".into());
    }
    if require_file && !resolved.is_file() {
        return Err("resource is not a file".into());
    }
    if !require_file && !resolved.is_dir() {
        return Err("resource is not a directory".into());
    }
    Ok(resolved)
}

pub(crate) fn inside_directory(base: &Path, relative: &str) -> Result<PathBuf, String> {
    inside_path(base, relative, false)
}

pub(crate) fn inside_file(base: &Path, relative: &str) -> Result<PathBuf, String> {
    inside_path(base, relative, true)
}

pub(crate) fn collect_skill_files(
    root: &Path,
    current: &Path,
    out: &mut Vec<String>,
) -> Result<(), GraphError> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(invalid(format!(
                "Plugin symlinks are not supported: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            collect_skill_files(root, &path, out)?;
        } else if metadata.is_file() && path.file_name().is_some_and(|name| name == "SKILL.md") {
            out.push(
                path.strip_prefix(root)
                    .map_err(|error| invalid(error.to_string()))?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

/// Every code resource of a Plugin tree.
///
/// A declared mutable path is pruned *before* anything looks at it: it is the
/// Plugin's own state (invariant 9), so neither enumeration nor the symlink and
/// credential checks below may touch it — a symlink or a `.env` the Plugin itself
/// put in its data directory is its business, not a malformed Plugin.
pub(crate) fn resource_files(
    root: &Path,
    mutable: &MutablePaths,
) -> Result<Vec<String>, GraphError> {
    let mut resources = Vec::new();
    collect_files(root, root, mutable, &mut resources)?;
    resources.sort();
    Ok(resources)
}

pub(crate) fn normalize_inside(base: &Path, relative: &str) -> Option<PathBuf> {
    let mut path = base.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !path.pop() || !path.starts_with(base) {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    path.starts_with(base).then_some(path)
}

#[cfg(unix)]
pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
pub(crate) fn is_executable(path: &Path) -> bool {
    path.is_file()
}
