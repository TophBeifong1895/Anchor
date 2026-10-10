//! Fixtures the builder's tests share.
//!
//! Every case runs the real `uv` and the real Bubblewrap adapter against a
//! locally crafted wheel, so the parts that can go wrong — hashes, the sandbox's
//! network boundary, relocation of a published tree — are exercised rather than
//! simulated. When the host has no `uv`/`python3`/`bwrap`, the case says so
//! instead of pretending to have checked something.

use std::path::{Path, PathBuf};

use base64::Engine;
use sha2::{Digest, Sha256};

use super::config::{EnvironmentProvisionConfig, PackageSource};

pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// A stored (uncompressed) ZIP, which is what a wheel is.
pub(crate) fn write_zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<(String, u32, u32, u32)> = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x0021u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.push((name.clone(), crc, size, offset));
    }
    let central_offset = out.len() as u32;
    for (name, crc, size, offset) in &central {
        out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x0021u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
    }
    let central_size = out.len() as u32 - central_offset;
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(central.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u16).to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// One small, valid wheel that provides an importable package and a console
/// script, so the published tree can be exercised the way a Run would.
pub(crate) fn fixture_wheel(directory: &Path, name: &str, version: &str) -> PathBuf {
    let dist = format!("{name}-{version}");
    let mut files: Vec<(String, Vec<u8>)> = vec![
        (
            format!("{name}/__init__.py"),
            format!("VALUE = {version:?}\n").into_bytes(),
        ),
        (
            format!("{name}/__main__.py"),
            b"from . import VALUE\n\n\ndef main_or_echo():\n    print('hello', VALUE)\n\n\nif __name__ == '__main__':\n    main_or_echo()\n"
                .to_vec(),
        ),
        (
            format!("{dist}.dist-info/METADATA"),
            format!("Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n").into_bytes(),
        ),
        (
            format!("{dist}.dist-info/WHEEL"),
            b"Wheel-Version: 1.0\nGenerator: probe\nRoot-Is-Purelib: true\nTag: py3-none-any\n"
                .to_vec(),
        ),
        (
            format!("{dist}.dist-info/entry_points.txt"),
            format!("[console_scripts]\n{name}-tool = {name}.__main__:main_or_echo\n").into_bytes(),
        ),
    ];
    let mut record = String::new();
    for (path, data) in &files {
        let digest = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(data));
        record.push_str(&format!("{path},sha256={digest},{}\n", data.len()));
    }
    record.push_str(&format!("{dist}.dist-info/RECORD,,\n"));
    files.push((format!("{dist}.dist-info/RECORD"), record.into_bytes()));
    let wheel = directory.join(format!("{name}-{version}-py3-none-any.whl"));
    std::fs::write(&wheel, write_zip(&files)).unwrap();
    wheel
}

pub(crate) fn tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| std::fs::canonicalize(candidate).ok())
}

/// The deployment's authorized preparation, as a test deployment would set it.
pub(crate) struct Toolchain {
    pub(crate) uv: PathBuf,
    pub(crate) python: PathBuf,
}

pub(crate) fn toolchain() -> Option<Toolchain> {
    let uv = tool("uv");
    // The base interpreter must be one a Run's sandbox already mounts, so this
    // deliberately prefers the distribution's own Python over a user-local one.
    let python = [
        "/usr/bin/python3.12",
        "/usr/bin/python3.11",
        "/usr/bin/python3.10",
        "/usr/bin/python3.9",
        "/usr/bin/python3.8",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|candidate| candidate.is_file())
    .and_then(|candidate| std::fs::canonicalize(candidate).ok());
    match (uv, python) {
        (Some(uv), Some(python)) => Some(Toolchain { uv, python }),
        _ => {
            eprintln!(
                "skipping the environment builder test: uv is not on PATH or no /usr/bin/python3.8+ exists"
            );
            None
        }
    }
}

pub(crate) fn config(
    root: &Path,
    toolchain: &Toolchain,
    wheels: PathBuf,
) -> EnvironmentProvisionConfig {
    EnvironmentProvisionConfig {
        uv_digest: super::config::sha256_file(&toolchain.uv).unwrap(),
        uv: toolchain.uv.clone(),
        python: toolchain.python.clone(),
        sources: vec![PackageSource::Directory(wheels)],
        build_root: root.join("build"),
        bwrap: std::env::var_os("ANCHOR_TEST_BWRAP")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("bwrap")),
    }
}
