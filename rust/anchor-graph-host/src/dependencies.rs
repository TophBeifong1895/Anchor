//! What a Plugin declares it needs at run time, and where that declaration's
//! shared store and views live.
//!
//! The declaration belongs to the Plugin (`plugin.json`), never to a node or a
//! run. Its identity is the declaration itself: the shared store holds one copy
//! of every (package, version) and a *view* is the light-weight Python
//! environment one declaration maps to. Keying the view by the declaration —
//! not by the whole Plugin digest — is what keeps a Skill edit from rebuilding
//! an environment, while a declaration edit still changes the Plugin digest and
//! therefore still needs a human to approve it again.
//!
//! This module owns the manifest field, its validation and the layout both the
//! installer and the launcher must agree on.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::GraphError;

/// The token a Plugin's manifest uses to name its own dependency view.
///
/// It is a *deployment* path, not a path inside the Plugin, so the catalog keeps
/// it as written and the host that grants the view substitutes it.
pub const VIEW_TOKEN: &str = "ANCHOR_PLUGIN_VIEW";

/// The directories a deployment's dependency content lives in, under the
/// Library root that already holds `plugins/`.
pub const DEPENDENCIES_DIRECTORY: &str = "dependencies";
pub const STORE_DIRECTORY: &str = "store";
pub const VIEWS_DIRECTORY: &str = "views";

/// Top-level `plugin.json` fields this catalog understands.
///
/// Anything else is refused: a mistyped field (`"dependencie"`) would otherwise
/// be silently ignored, which is exactly how an author ends up believing a
/// declaration took effect when it did not. Common descriptive metadata is
/// listed so a Plugin that merely documents itself is not rejected for it.
const MANIFEST_FIELDS: &[&str] = &[
    "name",
    "description",
    "version",
    "author",
    "homepage",
    "repository",
    "license",
    "keywords",
    "interface",
    "skills",
    "mcpServers",
    "dependencies",
    "mutable",
    "hooks",
    "commands",
    "agents",
    "apps",
];

/// The only dependency section this platform implements.
const PYTHON: &str = "python";
/// A declaration is written into a `requirements` file one line per entry, so a
/// line break (or any other control character) must never get in.
const MAX_REQUIREMENTS: usize = 64;
const MAX_REQUIREMENT_LENGTH: usize = 128;

/// What one Plugin declares it needs.
///
/// Only `python` exists today; the section is nested by language so a second
/// language does not have to rename a field Plugin authors already wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PluginDependencies {
    pub python: Vec<String>,
}

impl PluginDependencies {
    pub fn is_empty(&self) -> bool {
        self.python.is_empty()
    }

    /// The identity of this declaration.
    ///
    /// Requirement order does not change what is solved, so it must not change
    /// which view a declaration maps to either. The text is versioned so a later
    /// addition cannot silently collide with a digest computed today.
    pub fn declaration_digest(&self) -> String {
        let mut requirements = self.python.clone();
        requirements.sort();
        let mut hasher = Sha256::new();
        hasher.update(b"anchor.plugin-dependencies.v1\n");
        hasher.update(PYTHON.as_bytes());
        hasher.update(b"\n");
        for requirement in &requirements {
            hasher.update(requirement.as_bytes());
            hasher.update(b"\n");
        }
        format!("{:x}", hasher.finalize())
    }
}

/// The paths a Plugin declares as its own mutable state.
///
/// The platform provides the location and nothing else: it never copies,
/// rewrites or verifies these paths, and the code digest does not cover them.
/// That is why the declaration is explicit — the platform does not predict what a
/// Plugin does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MutablePaths {
    paths: Vec<String>,
}

impl MutablePaths {
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Whether one Plugin-relative path is inside a declared mutable path.
    ///
    /// A declaration that ends in `/` covers the whole subtree; one that does not
    /// covers exactly that path.
    pub fn covers(&self, relative: &str) -> bool {
        let relative = relative.trim_start_matches("./");
        self.paths.iter().any(|declared| {
            if let Some(subtree) = declared.strip_suffix('/') {
                relative == subtree || relative.starts_with(declared)
            } else {
                relative == declared
            }
        })
    }

    /// The declared paths as absolute paths under one Plugin directory.
    ///
    /// Fails closed on anything that would leave the Plugin directory: a
    /// declaration that is not a plain relative path, or one whose existing
    /// components include a symlink (which could point anywhere). The caller gets
    /// a reason, never a widened path.
    pub fn resolve(&self, plugin_directory: &Path) -> Result<Vec<PathBuf>, String> {
        let mut resolved = Vec::with_capacity(self.paths.len());
        for declared in &self.paths {
            let relative = Path::new(declared);
            if relative.is_absolute()
                || declared.contains('\0')
                || relative
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return Err(format!("`{declared}` is not a plain relative mutable path"));
            }
            // Every existing component must be a real directory: a symlink here
            // would let a declaration write outside the Plugin directory.
            let mut current = plugin_directory.to_path_buf();
            for component in relative.components() {
                let std::path::Component::Normal(name) = component else {
                    continue;
                };
                current.push(name);
                match std::fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(format!(
                            "mutable path `{declared}` contains the symlink {}",
                            current.display()
                        ));
                    }
                    Ok(_) => {}
                    // The rest does not exist yet; the host may create it.
                    Err(_) => break,
                }
            }
            let canonical = plugin_directory
                .canonicalize()
                .map_err(|error| format!("Plugin directory cannot be resolved: {error}"))?;
            if !current.starts_with(&canonical) {
                return Err(format!(
                    "mutable path `{declared}` escapes the Plugin directory"
                ));
            }
            resolved.push(current);
        }
        Ok(resolved)
    }
}

/// Read `mutable` from an already-parsed manifest.
/// The declared mutable paths of one manifest, for callers outside the catalog.
///
/// A caller that has to decide what is *code* in a tree the catalog is not reading
/// (the installer validating an installed Plugin, for instance) uses the same
/// parsing and the same rules as the catalog, so the two can never disagree.
pub fn mutable_paths_from_manifest(value: &Value) -> Result<MutablePaths, String> {
    let manifest = value
        .as_object()
        .ok_or_else(|| "a Plugin manifest must be an object".to_owned())?;
    parse_mutable("the Plugin", manifest).map_err(|error| error.to_string())
}

pub(crate) fn parse_mutable(
    id: &str,
    manifest: &Map<String, Value>,
) -> Result<MutablePaths, GraphError> {
    let Some(value) = manifest.get("mutable") else {
        return Ok(MutablePaths::default());
    };
    let entries = value.as_array().ok_or_else(|| {
        invalid(format!(
            "Plugin {id}: mutable must be a list of Plugin-relative paths"
        ))
    })?;
    let mut paths = Vec::with_capacity(entries.len());
    for entry in entries {
        let text = entry.as_str().ok_or_else(|| {
            invalid(format!(
                "Plugin {id}: mutable must be a list of Plugin-relative paths"
            ))
        })?;
        if !valid_mutable_path(text) {
            return Err(invalid(format!(
                "Plugin {id}: `{text}` is not a Plugin-relative mutable path (relative, no `..`, not `plugin.json`)"
            )));
        }
        paths.push(text.to_owned());
    }
    Ok(MutablePaths { paths })
}

/// The shape rule for one declared mutable path.
fn valid_mutable_path(value: &str) -> bool {
    if value.is_empty() || value.len() > 200 || value.contains('\0') {
        return false;
    }
    let trimmed = value.strip_suffix('/').unwrap_or(value);
    if trimmed.is_empty() || trimmed == "plugin.json" {
        return false;
    }
    let path = Path::new(trimmed);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

/// Refuse a manifest field this catalog does not act on.
pub(crate) fn reject_unknown_fields(
    id: &str,
    manifest: &Map<String, Value>,
) -> Result<(), GraphError> {
    if let Some(key) = manifest
        .keys()
        .find(|key| !MANIFEST_FIELDS.contains(&key.as_str()))
    {
        return Err(invalid(format!(
            "Plugin {id}: unsupported manifest field: {key}"
        )));
    }
    Ok(())
}

/// Read `dependencies` from an already-parsed manifest.
///
/// The shape is checked here because it is part of what a human approves (the
/// declaration is inside the Plugin digest). Whether a requirement is *solvable*
/// is uv's answer, not this parser's, and is reported verbatim at install time.
pub(crate) fn parse(
    id: &str,
    manifest: &Map<String, Value>,
) -> Result<PluginDependencies, GraphError> {
    let Some(value) = manifest.get("dependencies") else {
        return Ok(PluginDependencies::default());
    };
    let sections = value.as_object().ok_or_else(|| {
        invalid(format!(
            "Plugin {id}: dependencies must be an object of language sections"
        ))
    })?;
    if let Some(key) = sections.keys().find(|key| key.as_str() != PYTHON) {
        return Err(invalid(format!(
            "Plugin {id}: unsupported dependencies section: {key}"
        )));
    }
    let Some(python) = sections.get(PYTHON) else {
        return Ok(PluginDependencies::default());
    };
    let entries = python.as_array().ok_or_else(|| {
        invalid(format!(
            "Plugin {id}: dependencies.python must be a list of requirements"
        ))
    })?;
    if entries.len() > MAX_REQUIREMENTS {
        return Err(invalid(format!(
            "Plugin {id}: dependencies.python names more than {MAX_REQUIREMENTS} requirements"
        )));
    }
    let mut requirements = Vec::with_capacity(entries.len());
    for entry in entries {
        let text = entry.as_str().ok_or_else(|| {
            invalid(format!(
                "Plugin {id}: dependencies.python must be a list of requirements"
            ))
        })?;
        if !valid_requirement_shape(text) {
            return Err(invalid(format!(
                "Plugin {id}: `{text}` is not a plain Python requirement (a package name, optionally with version specifiers)"
            )));
        }
        requirements.push(text.to_owned());
    }
    Ok(PluginDependencies {
        python: requirements,
    })
}

/// The structural half of the requirement rule: one line, no option, no path,
/// no URL, no environment marker.
///
/// The authoritative charset check runs where the resolver writes the ask
/// (`anchor-runner-host`), so this stays a shape check and there is exactly one
/// implementation of "what a requirement may say".
fn valid_requirement_shape(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REQUIREMENT_LENGTH
        && value.trim() == value
        && !value.starts_with('-')
        && !value.contains(['\n', '\r', '\0', '/', '\\'])
        && !value.chars().any(char::is_control)
}

/// Where the shared store of unpacked (package, version) bytes lives.
pub fn store_root(library_root: &Path) -> PathBuf {
    library_root
        .join(DEPENDENCIES_DIRECTORY)
        .join(STORE_DIRECTORY)
}

/// The library root the layout is derived from, canonicalized as far as it exists.
///
/// A deployment names one library root, and every half of the platform derives
/// the view path from it: the installer before the store exists, the launcher
/// afterwards. Canonicalizing the deepest existing ancestor (and re-attaching the
/// rest) makes both agree even when the deployment's root contains a symlink and
/// the `dependencies/` directory has not been created yet.
pub fn resolved_library_root(library_root: &Path) -> PathBuf {
    let mut candidate = library_root.to_path_buf();
    loop {
        if let Ok(resolved) = candidate.canonicalize() {
            let rest = library_root
                .strip_prefix(&candidate)
                .unwrap_or(Path::new(""));
            return resolved.join(rest);
        }
        if !candidate.pop() {
            return library_root.to_path_buf();
        }
    }
}

/// Where every Plugin version's view lives.
pub fn views_root(library_root: &Path) -> PathBuf {
    library_root
        .join(DEPENDENCIES_DIRECTORY)
        .join(VIEWS_DIRECTORY)
}

/// The view of one Plugin version, addressed by its declaration.
///
/// The path is what `${ANCHOR_PLUGIN_VIEW}` expands to, so the installer and the
/// launcher derive it through this one function.
pub fn plugin_view_path(library_root: &Path, id: &str, declaration_digest: &str) -> PathBuf {
    views_root(library_root).join(id).join(declaration_digest)
}

fn invalid(message: String) -> GraphError {
    GraphError::InvalidSnapshot(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn a_mistyped_manifest_field_is_refused_instead_of_ignored() {
        let error = reject_unknown_fields(
            "demo",
            &manifest(json!({"name":"Demo","dependencie":{"python":["idna==3.7"]}})),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("unsupported manifest field"),
            "{error}"
        );
        // A declared-but-unexecuted kind is known, so it stays reportable.
        assert!(
            reject_unknown_fields("demo", &manifest(json!({"name":"Demo","hooks":{}}))).is_ok()
        );
    }

    #[test]
    fn a_declaration_is_read_only_from_its_own_section() {
        let parsed = parse(
            "demo",
            &manifest(json!({"dependencies":{"python":["idna==3.7","requests>=2,<3"]}})),
        )
        .unwrap();
        assert_eq!(parsed.python, vec!["idna==3.7", "requests>=2,<3"]);
        assert!(!parsed.is_empty());
        assert!(
            parse("demo", &manifest(json!({"name":"Demo"})))
                .unwrap()
                .is_empty()
        );

        // Unknown language section, wrong types and a requirement that is an
        // option or a path are all refused.
        for bad in [
            json!({"dependencies":{"node":["left-pad"]}}),
            json!({"dependencies":{"python":"idna==3.7"}}),
            json!({"dependencies":{"python":[7]}}),
            json!({"dependencies":["idna==3.7"]}),
            json!({"dependencies":{"python":["-r other.txt"]}}),
            json!({"dependencies":{"python":["../escape"]}}),
            json!({"dependencies":{"python":["pkg @ https://example.com/pkg.whl"]}}),
            json!({"dependencies":{"python":[""]}}),
            json!({"dependencies":{"python":["idna==3.7\nother"]}}),
            json!({"dependencies":{"python":[" idna==3.7"]}}),
        ] {
            assert!(
                parse("demo", &manifest(bad.clone())).is_err(),
                "{bad} was accepted"
            );
        }
    }

    #[test]
    fn the_declaration_digest_ignores_requirement_order() {
        let one = parse(
            "demo",
            &manifest(json!({"dependencies":{"python":["a==1","b==2"]}})),
        )
        .unwrap();
        let other = parse(
            "demo",
            &manifest(json!({"dependencies":{"python":["b==2","a==1"]}})),
        )
        .unwrap();
        assert_eq!(one.declaration_digest(), other.declaration_digest());
        let changed = parse(
            "demo",
            &manifest(json!({"dependencies":{"python":["a==1"]}})),
        )
        .unwrap();
        assert_ne!(one.declaration_digest(), changed.declaration_digest());
        assert_eq!(one.declaration_digest().len(), 64);

        // The paths both halves of the platform agree on.
        let root = Path::new("/library");
        assert_eq!(
            store_root(root),
            PathBuf::from("/library/dependencies/store")
        );
        assert_eq!(
            plugin_view_path(root, "demo", &one.declaration_digest()),
            views_root(root).join("demo").join(one.declaration_digest())
        );
    }
}
