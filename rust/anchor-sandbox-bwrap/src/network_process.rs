use std::{
    ffi::OsString,
    io::{self, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    process::Command,
};

const INFO_FD: i32 = 300;
const BLOCK_FD: i32 = 301;
const DNS_FD: i32 = 302;
const READY_FD: i32 = 303;
const EXIT_FD: i32 = 304;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

struct Invocation {
    bwrap: OsString,
    slirp: OsString,
    arguments: Vec<OsString>,
    parent_info: Option<i32>,
}

fn parse(arguments: Vec<OsString>) -> io::Result<Invocation> {
    if arguments.len() < 5
        || arguments[0] != "--bwrap"
        || arguments[2] != "--slirp"
        || arguments[4] != "--"
    {
        return Err(io::Error::other(
            "expected --bwrap PATH --slirp PATH -- BWRAP_ARGUMENTS",
        ));
    }
    let mut forwarded = Vec::new();
    let mut parent_info = None;
    let mut index = 5;
    while index < arguments.len() {
        if arguments[index] == "--" {
            forwarded.extend_from_slice(&arguments[index..]);
            break;
        }
        if arguments[index] == "--share-net" || arguments[index] == "--block-fd" {
            return Err(io::Error::other(
                "network namespace and startup gate belong to the sandbox adapter",
            ));
        }
        if arguments[index] == "--info-fd" {
            if parent_info.is_some() {
                return Err(io::Error::other("duplicate startup descriptor"));
            }
            let value = arguments
                .get(index + 1)
                .and_then(|v| v.to_str())
                .and_then(|v| v.parse::<i32>().ok());
            parent_info = Some(
                value
                    .filter(|fd| *fd >= 3 && *fd < INFO_FD)
                    .ok_or_else(|| io::Error::other("invalid startup descriptor"))?,
            );
            index += 2;
        } else {
            forwarded.push(arguments[index].clone());
            index += 1;
        }
    }
    if !forwarded.iter().any(|v| v == "--") || !forwarded.iter().any(|v| v == "--unshare-all") {
        return Err(io::Error::other(
            "a private Bubblewrap namespace and command are required",
        ));
    }
    Ok(Invocation {
        bwrap: arguments[1].clone(),
        slirp: arguments[3].clone(),
        arguments: forwarded,
        parent_info,
    })
}

fn pair() -> io::Result<(UnixStream, std::os::unix::net::UnixStream)> {
    let (parent, child) = std::os::unix::net::UnixStream::pair()?;
    parent.set_nonblocking(true)?;
    Ok((UnixStream::from_std(parent)?, child))
}

fn inherit(source: i32, destination: i32) -> io::Result<()> {
    if unsafe { libc::dup2(source, destination) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(destination, libc::F_SETFD, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn die_with_parent(parent: libc::pid_t) -> io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::getppid() } != parent {
        return Err(io::Error::other("sandbox supervisor exited during startup"));
    }
    Ok(())
}

fn dns_file() -> io::Result<OwnedFd> {
    let fd = unsafe { libc::memfd_create(c"anchor-sandbox-resolver".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(b"nameserver 10.0.2.3\noptions timeout:2 attempts:2\n")?;
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    Ok(file.into())
}

pub(super) async fn run(arguments: Vec<OsString>) -> io::Result<u8> {
    let invocation = parse(arguments)?;
    let (mut info, info_child) = pair()?;
    let (mut gate, gate_child) = pair()?;
    let dns = dns_file()?;
    let command_index = invocation.arguments.iter().position(|v| v == "--").unwrap();
    let mut command = Command::new(&invocation.bwrap);
    command
        .args(&invocation.arguments[..command_index])
        .args([
            "--info-fd",
            &INFO_FD.to_string(),
            "--block-fd",
            &BLOCK_FD.to_string(),
            "--ro-bind-data",
            &DNS_FD.to_string(),
            "/etc/resolv.conf",
        ])
        .args(&invocation.arguments[command_index..])
        .kill_on_drop(true);
    let info_fd = info_child.as_raw_fd();
    let gate_fd = gate_child.as_raw_fd();
    let dns_fd = dns.as_raw_fd();
    let parent = unsafe { libc::getpid() };
    let parent_info = invocation.parent_info;
    unsafe {
        command.as_std_mut().pre_exec(move || {
            die_with_parent(parent)?;
            inherit(info_fd, INFO_FD)?;
            inherit(gate_fd, BLOCK_FD)?;
            inherit(dns_fd, DNS_FD)?;
            if let Some(fd) = parent_info {
                libc::close(fd);
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    drop((info_child, gate_child, dns));
    let startup = async {
        let mut bytes = Vec::new();
        (&mut info).take(4097).read_to_end(&mut bytes).await?;
        if bytes.len() > 4096 {
            return Err(io::Error::other("Bubblewrap startup record is too large"));
        }
        let record: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let pid = record["child-pid"]
            .as_u64()
            .filter(|v| *v > 0 && *v <= i32::MAX as u64)
            .ok_or_else(|| io::Error::other("Bubblewrap did not report its namespace process"))?;
        Ok((bytes, pid))
    };
    let (startup_bytes, pid) = match tokio::time::timeout(STARTUP_TIMEOUT, startup).await {
        Ok(Ok(value)) => value,
        result => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(match result {
                Ok(Err(error)) => error,
                _ => io::Error::other("Bubblewrap network startup timed out"),
            });
        }
    };
    let (mut ready, ready_child) = pair()?;
    let (exit, exit_child) = pair()?;
    let mut network = Command::new(&invocation.slirp);
    network
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .args([
            "--configure",
            "--disable-host-loopback",
            "--netns-type=path",
            "--ready-fd",
            &READY_FD.to_string(),
            "--exit-fd",
            &EXIT_FD.to_string(),
        ])
        .arg(format!("--userns-path=/proc/{pid}/ns/user"))
        .arg(format!("/proc/{pid}/ns/net"))
        .arg("tap0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let ready_fd = ready_child.as_raw_fd();
    let exit_fd = exit_child.as_raw_fd();
    unsafe {
        network.as_std_mut().pre_exec(move || {
            die_with_parent(parent)?;
            inherit(ready_fd, READY_FD)?;
            inherit(exit_fd, EXIT_FD)?;
            if let Some(fd) = parent_info {
                libc::close(fd);
            }
            Ok(())
        });
    }
    let mut network = match network.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
    };
    drop((ready_child, exit_child));
    let ready_result = tokio::time::timeout(STARTUP_TIMEOUT, ready.read_u8()).await;
    if !matches!(ready_result, Ok(Ok(_))) {
        let _ = child.kill().await;
        let _ = network.kill().await;
        let _ = child.wait().await;
        let _ = network.wait().await;
        return Err(io::Error::other(
            "slirp4netns did not configure isolated networking",
        ));
    }
    if let Some(fd) = parent_info {
        let mut parent = unsafe { std::fs::File::from_raw_fd(fd) };
        parent.write_all(&startup_bytes)?;
    }
    gate.write_all(b"1").await?;
    drop(gate);
    let result = tokio::select! {
        status = child.wait() => status,
        _ = network.wait() => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(io::Error::other("isolated network helper exited while the sandbox was running"));
        }
    };
    drop(exit);
    let _ = network.kill().await;
    let _ = network.wait().await;
    Ok(u8::try_from(result?.code().unwrap_or(1)).unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn startup_and_network_flags_are_owned_by_adapter() {
        assert!(
            parse(args(&[
                "--bwrap",
                "/b",
                "--slirp",
                "/s",
                "--",
                "--unshare-all",
                "--share-net",
                "--",
                "true"
            ]))
            .is_err()
        );
        assert!(
            parse(args(&[
                "--bwrap", "/b", "--slirp", "/s", "--", "--", "true"
            ]))
            .is_err()
        );
        let invocation = parse(args(&[
            "--bwrap",
            "/b",
            "--slirp",
            "/s",
            "--",
            "--unshare-all",
            "--info-fd",
            "200",
            "--",
            "sh",
            "-c",
            "echo --share-net",
        ]))
        .unwrap();
        assert_eq!(invocation.parent_info, Some(200));
        assert_eq!(invocation.arguments.last().unwrap(), "echo --share-net");
    }
}
