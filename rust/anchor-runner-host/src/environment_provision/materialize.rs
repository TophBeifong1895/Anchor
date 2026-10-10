//! Making a built tree publishable: no symlinks, and a real interpreter.
//!
//! The Library refuses a candidate that contains anything but regular files and
//! directories, because a published environment must be usable exactly where it
//! is published. A virtual environment violates that twice: `bin/python` is a
//! symlink to the deployment's interpreter, and the alias names point at each
//! other. Copying the tree with those links intact would publish content that
//! resolves somewhere else.
//!
//! So the build's last step is to materialize:
//!
//! - the environment's own `bin/python` becomes a copy of the interpreter the
//!   plan named, so the tree carries the interpreter it was built with;
//! - the alias names become tiny wrappers that exec *that* file, so `python3`
//!   can never fall through to the deployment's interpreter on PATH;
//! - every other symlink is replaced by what it points at;
//! - the venv's `lib64 -> lib` layout link is dropped, because
//!   `lib/pythonX.Y/site-packages` is the real path and duplicating `lib` would
//!   double the published content.
//!
//! Afterwards the tree is scanned again: a symlink that survived is an error,
//! not something to publish and hope about.

use std::path::{Path, PathBuf};

use super::config::ProvisionError;

/// Replace every symlink in `tree` with real content.
pub(crate) fn materialize(tree: &Path, interpreter: &Path) -> Result<(), ProvisionError> {
    let interpreter = std::fs::canonicalize(interpreter).map_err(|error| {
        ProvisionError::Failed(format!("the planned interpreter is unavailable: {error}"))
    })?;
    let python = tree.join("bin/python");
    // A handful of passes is enough for the links this creates; more than that
    // means the tree links to itself and should not be published at all.
    for _ in 0..8 {
        let links = collect_symlinks(tree)?;
        if links.is_empty() {
            break;
        }
        for (path, target) in links {
            let is_alias = path != python
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("python"))
                && target == interpreter
                && python.exists();
            if is_alias {
                // A wrapper, not a copy: three copies of a 60 MiB interpreter
                // would triple every published environment for one name.
                write_wrapper(&path)?;
                continue;
            }
            replace_with_target(&path, &target)?;
        }
    }
    let remaining = collect_symlinks(tree)?;
    if !remaining.is_empty() {
        return Err(ProvisionError::Failed(format!(
            "the environment still links to {} after materializing; it cannot be published as it is",
            remaining[0].0.display()
        )));
    }
    let python = tree.join("bin/python");
    if !python.is_file() {
        return Err(ProvisionError::Failed(
            "the built environment has no `bin/python` of its own".to_owned(),
        ));
    }
    Ok(())
}

/// Every symlink in the tree, with what it resolves to.
fn collect_symlinks(tree: &Path) -> Result<Vec<(PathBuf, PathBuf)>, ProvisionError> {
    let mut found = Vec::new();
    let mut pending = vec![tree.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|error| {
            ProvisionError::Failed(format!("{} is not readable: {error}", directory.display()))
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| ProvisionError::Failed(error.to_string()))?;
            if metadata.file_type().is_symlink() {
                let target = std::fs::canonicalize(&path).map_err(|error| {
                    ProvisionError::Failed(format!(
                        "{} is a broken link and cannot be published: {error}",
                        path.display()
                    ))
                })?;
                found.push((path, target));
            } else if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn replace_with_target(path: &Path, target: &Path) -> Result<(), ProvisionError> {
    let metadata =
        std::fs::metadata(target).map_err(|error| ProvisionError::Failed(error.to_string()))?;
    if metadata.is_dir() {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        // The venv layout's only directory link, and the one whose copy would
        // duplicate every installed package.
        if name == "lib64" && target.file_name().is_some_and(|target| target == "lib") {
            return std::fs::remove_file(path)
                .map_err(|error| ProvisionError::Failed(error.to_string()));
        }
        let staged = path.with_extension("anchor-materialized");
        let _ = std::fs::remove_dir_all(&staged);
        copy_directory(target, &staged)?;
        std::fs::remove_file(path).map_err(|error| ProvisionError::Failed(error.to_string()))?;
        return std::fs::rename(&staged, path)
            .map_err(|error| ProvisionError::Failed(error.to_string()));
    }
    if !metadata.is_file() {
        return Err(ProvisionError::Failed(format!(
            "{} points at something that is neither a file nor a directory",
            path.display()
        )));
    }
    copy_file(path, target)
}

/// Replace the link at `path` with the contents of `target`.
fn copy_file(path: &Path, target: &Path) -> Result<(), ProvisionError> {
    let staged = path.with_extension("anchor-materialized");
    std::fs::copy(target, &staged).map_err(|error| ProvisionError::Failed(error.to_string()))?;
    std::fs::remove_file(path).map_err(|error| ProvisionError::Failed(error.to_string()))?;
    std::fs::rename(&staged, path).map_err(|error| ProvisionError::Failed(error.to_string()))
}

fn copy_directory(from: &Path, to: &Path) -> Result<(), ProvisionError> {
    std::fs::create_dir_all(to).map_err(|error| ProvisionError::Failed(error.to_string()))?;
    for entry in std::fs::read_dir(from)
        .map_err(|error| ProvisionError::Failed(error.to_string()))?
        .flatten()
    {
        let source = entry.path();
        let destination = to.join(entry.file_name());
        // A link *inside* a directory being materialized is refused rather than
        // followed: following it would copy whatever it points at — including a
        // link back to an ancestor, which would recurse until the disk complains
        // — into published content. Top-level links are the ones this pass is
        // here to resolve, and they are handled before this point.
        let metadata = std::fs::symlink_metadata(&source)
            .map_err(|error| ProvisionError::Failed(error.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(ProvisionError::Failed(format!(
                "{} is a link inside a directory being materialized; published content must not contain indirection",
                source.display()
            )));
        }
        if metadata.is_dir() {
            copy_directory(&source, &destination)?;
        } else {
            std::fs::copy(&source, &destination)
                .map_err(|error| ProvisionError::Failed(error.to_string()))?;
        }
    }
    Ok(())
}

/// A real file that execs this environment's own interpreter.
fn write_wrapper(path: &Path) -> Result<(), ProvisionError> {
    let staged = path.with_extension("anchor-materialized");
    std::fs::write(
        &staged,
        "#!/bin/sh\nexec \"$(dirname \"$0\")/python\" \"$@\"\n",
    )
    .map_err(|error| ProvisionError::Failed(error.to_string()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&staged)
            .map_err(|error| ProvisionError::Failed(error.to_string()))?
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&staged, permissions)
            .map_err(|error| ProvisionError::Failed(error.to_string()))?;
    }
    std::fs::remove_file(path).map_err(|error| ProvisionError::Failed(error.to_string()))?;
    std::fs::rename(&staged, path).map_err(|error| ProvisionError::Failed(error.to_string()))
}

#[cfg(test)]
mod materialize_tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// A build tree with an interpreter of its own, which is the one thing every
    /// publishable environment must have.
    fn tree(root: &Path) -> PathBuf {
        let tree = root.join("tree");
        std::fs::create_dir_all(tree.join("bin")).unwrap();
        std::fs::write(tree.join("bin/python"), "#!/bin/sh\necho 3.12\n").unwrap();
        tree
    }

    /// The interpreter the plan named; materialize copies it into the tree.
    fn interpreter(root: &Path) -> PathBuf {
        let path = root.join("planned-python");
        std::fs::write(&path, "#!/bin/sh\necho 3.12\n").unwrap();
        path
    }

    /// A broken link cannot be resolved, so the tree cannot be published: there
    /// is no content to replace it with.
    #[test]
    fn a_broken_link_is_refused_rather_than_published() {
        let root = tempfile::tempdir().unwrap();
        let tree = tree(root.path());
        symlink("../missing-target", tree.join("bin/python3")).unwrap();

        let error = materialize(&tree, &interpreter(root.path())).unwrap_err();
        assert!(
            matches!(error, ProvisionError::Failed(ref reason) if reason.contains("broken link")),
            "{error:?}"
        );
    }

    /// A link the materializer cannot replace in place — one nested inside a
    /// directory it would have to copy — is a refusal, not content published with
    /// indirection in it.
    #[test]
    fn a_link_nested_inside_a_copied_directory_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let tree = tree(root.path());
        let outside = root.path().join("outside");
        std::fs::create_dir_all(outside.join("nested")).unwrap();
        symlink("/etc/hostname", outside.join("nested/inner")).unwrap();
        // The link's name is not `lib64`, so the target directory would be
        // copied — and copying it would carry a link into published content.
        symlink(&outside, tree.join("lib")).unwrap();

        let error = materialize(&tree, &interpreter(root.path())).unwrap_err();
        assert!(
            matches!(error, ProvisionError::Failed(ref reason)
                if reason.contains("link inside a directory being materialized")),
            "{error:?}"
        );
        assert!(
            std::fs::symlink_metadata(tree.join("lib")).is_ok(),
            "the refused tree is left as it was"
        );
    }

    /// The venv layout's `lib64 -> lib` is dropped, not copied: `lib` already
    /// holds the real path, and duplicating it would double the published tree.
    #[test]
    fn the_lib64_layout_link_is_dropped_instead_of_published() {
        let root = tempfile::tempdir().unwrap();
        let tree = tree(root.path());
        std::fs::create_dir_all(tree.join("lib/python3.12/site-packages")).unwrap();
        std::fs::write(tree.join("lib/python3.12/site-packages/marker.txt"), "real").unwrap();
        symlink("lib", tree.join("lib64")).unwrap();

        materialize(&tree, &interpreter(root.path())).unwrap();

        assert!(
            std::fs::symlink_metadata(tree.join("lib64")).is_err(),
            "the layout link is dropped, not published"
        );
        assert_eq!(
            std::fs::read_to_string(tree.join("lib/python3.12/site-packages/marker.txt")).unwrap(),
            "real",
            "the real `lib` content stays exactly where it was"
        );
        assert!(tree.join("bin/python").is_file());
    }
}
