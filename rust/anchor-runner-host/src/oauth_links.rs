//! The product association between a browser OAuth transaction and its source
//! Pilot Turn. Tokens and protocol transactions remain in the Library store.
use anchor_library::OAuthBinding;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PilotOAuthLink {
    pub(crate) format: u32,
    pub(crate) owner: String,
    pub(crate) session: String,
    pub(crate) created_at: String,
    pub(crate) turn: String,
    pub(crate) binding: OAuthBinding,
    pub(crate) digest: String,
    pub(crate) authorization_digest: String,
    pub(crate) expires_at: u64,
    pub(crate) configured: bool,
    #[serde(default)]
    pub(crate) cancelled: bool,
}

fn path(data: &Path, state: &str) -> Result<PathBuf, String> {
    if state.is_empty() || state.len() > 128 {
        return Err("invalid OAuth transaction identity".into());
    }
    Ok(data
        .join("platform/oauth-links")
        .join(format!("{:x}.json", Sha256::digest(state.as_bytes()))))
}

pub(crate) fn save(data: &Path, state: &str, link: &PilotOAuthLink) -> Result<(), String> {
    save_path(&path(data, state)?, link)
}

fn save_path(path: &Path, link: &PilotOAuthLink) -> Result<(), String> {
    let parent = path.parent().expect("OAuth link parent");
    crate::host_fs::directory(parent)?;
    check_directory(parent)?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "OAuth association staging failed")?;
    fs::set_permissions(file.path(), fs::Permissions::from_mode(0o600))
        .map_err(|_| "OAuth association permissions failed")?;
    file.write_all(&serde_json::to_vec(link).map_err(|_| "OAuth association encoding failed")?)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| "OAuth association persistence failed")?;
    file.persist(path)
        .map_err(|_| "OAuth association publication failed")?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "OAuth association directory sync failed".into())
}

fn check_directory(path: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "OAuth association directory unavailable")?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err("OAuth association directory requires Host ownership and mode 700".into());
    }
    Ok(())
}

pub(crate) fn load(data: &Path, state: &str) -> Result<Option<PilotOAuthLink>, String> {
    let path = path(data, state)?;
    read(&path)
}

fn read(path: &Path) -> Result<Option<PilotOAuthLink>, String> {
    use rustix::fs::{Mode, OFlags, open, openat};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory =
        open("/", flags, Mode::empty()).map_err(|_| "OAuth association root unavailable")?;
    let parent = path
        .parent()
        .ok_or("OAuth association parent unavailable")?;
    if !parent.is_absolute() {
        return Err("OAuth association root must be absolute".into());
    }
    for component in parent.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => {
                directory = match openat(&directory, name, flags, Mode::empty()) {
                    Ok(directory) => directory,
                    Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
                    Err(_) => {
                        return Err("OAuth association parent must not contain symlinks".into());
                    }
                }
            }
            _ => return Err("OAuth association path is invalid".into()),
        }
    }
    let metadata =
        rustix::fs::fstat(&directory).map_err(|_| "OAuth association directory unavailable")?;
    if metadata.st_uid != rustix::process::geteuid().as_raw() || metadata.st_mode & 0o777 != 0o700 {
        return Err("OAuth association directory requires Host ownership and mode 700".into());
    }
    let file: fs::File = match openat(
        &directory,
        path.file_name()
            .ok_or("OAuth association filename unavailable")?,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => file.into(),
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
        Err(_) => return Err("OAuth association unavailable".into()),
    };
    let metadata = file
        .metadata()
        .map_err(|_| "OAuth association unavailable")?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err("OAuth association must be a Host-owned mode 600 regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "OAuth association unavailable")?;
    if bytes.len() > 65536 {
        return Err("OAuth association is too large".into());
    }
    let link: PilotOAuthLink =
        serde_json::from_slice(&bytes).map_err(|_| "OAuth association malformed")?;
    if link.format != 1
        || link.owner.is_empty()
        || link.session.is_empty()
        || link.turn.is_empty()
        || link.binding.owner() != crate::credential_store::binding_owner(&link.owner)
    {
        return Err("OAuth association ownership is invalid".into());
    }
    Ok(Some(link))
}

pub(crate) fn delete_session(
    data: &Path,
    owner: &str,
    session: &anchor_platform_session::Session,
) -> Result<(), String> {
    let root = data.join("platform/oauth-links");
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("OAuth associations unavailable".into()),
    };
    check_directory(&root)?;
    for entry in entries {
        let path = entry.map_err(|_| "OAuth association unavailable")?.path();
        if let Some(mut link) = read(&path)?
            && link.owner == owner
            && link.session == session.id
            && link.created_at == session.created_at.to_rfc3339()
        {
            // Retain only the protocol association metadata until its callback is
            // consumed/expired. Removing it would make a late Pilot callback look
            // like an unrelated HTTP authorization with no source Session.
            link.cancelled = true;
            save_path(&path, &link)?;
        }
    }
    Ok(())
}
