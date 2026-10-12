//! Create an installable candidate; installation remains the Library's review
//! and install operation, and this command never writes into an existing Plugin.
use rustix::fs::{Mode, OFlags, RenameFlags, open, renameat_with};
use std::{
    fs::{self, File},
    io::{self, Write},
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::Path,
};

pub fn package_plugin(destination: impl AsRef<Path>) -> io::Result<()> {
    let destination = std::path::absolute(destination)?;
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("destination has no parent"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| io::Error::other("destination has no name"))?;
    let directory = File::from(open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let pinned = std::path::PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let staging = tempfile::Builder::new()
        .prefix(".anchor-network-")
        .tempdir_in(&pinned)?;
    for path in ["bin", "skills/network"] {
        fs::create_dir_all(staging.path().join(path))?;
    }
    for (path, bytes) in [
        (
            "plugin.json",
            include_bytes!("../../../plugins/network/plugin.json").as_slice(),
        ),
        (
            "skills/network/SKILL.md",
            include_bytes!("../../../plugins/network/skills/network/SKILL.md").as_slice(),
        ),
    ] {
        let mut file = File::create(staging.path().join(path))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    let mut executable = File::open("/proc/self/exe")?;
    let mut binary = File::create(staging.path().join("bin/anchor-network-tools"))?;
    io::copy(&mut executable, &mut binary)?;
    binary.set_permissions(fs::Permissions::from_mode(0o755))?;
    binary.sync_all()?;
    for path in ["bin", "skills/network", "skills", ""] {
        File::open(staging.path().join(path))?.sync_all()?;
    }
    renameat_with(
        &directory,
        staging.path().file_name().unwrap(),
        &directory,
        name,
        RenameFlags::NOREPLACE,
    )?;
    directory.sync_all()?;
    let _ = staging.keep();
    Ok(())
}
