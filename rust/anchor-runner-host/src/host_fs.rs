//! Filesystem primitives shared by Host storage and runtime services.

use anchor_runtime::graph::InvocationKey;
use sha2::{Digest, Sha256};
use std::path::Path;

/// The existing lowercase SHA-256 filename stem, without each caller's prefix.
pub(crate) fn invocation_stem(key: &InvocationKey) -> String {
    format!("{:x}", Sha256::digest(key.durable_key().as_bytes()))
}

/// Create durable private directories without following symlinks in the path.
/// Error text is retained from the original Pilot helper for existing callers.
pub(crate) fn directory(root: &Path) -> Result<(), String> {
    use rustix::fs::{Mode, OFlags, fsync, mkdirat, open, openat};
    use std::path::Component;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut parent = open("/", flags, Mode::empty()).map_err(|_| "Pilot directory unavailable")?;
    for component in root.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                match mkdirat(&parent, name, Mode::RWXU) {
                    Ok(()) => {
                        fsync(&parent).map_err(|_| "Pilot directory could not be persisted")?
                    }
                    Err(error) if error == rustix::io::Errno::EXIST => {}
                    Err(_) => return Err("Pilot directory unavailable".into()),
                }
                parent = openat(&parent, name, flags, Mode::empty())
                    .map_err(|_| "Pilot directory must not contain symlinks")?;
            }
            _ => return Err("Pilot directory must be an absolute host path".into()),
        }
    }
    Ok(())
}
