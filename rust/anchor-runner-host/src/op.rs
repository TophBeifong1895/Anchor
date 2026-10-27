//! Original Graph shell commands and the deterministic routing helper.

use crate::channel_control::{self, SendError};
use anchor_runtime::{ReadOnlyInput, SandboxEnvironment, SandboxRequest};
use std::path::Path;

const ROUTE_HELPER: &str = "/tools/anchor-runtime/anchor-route";
const CHANNEL_SEND_HELPER: &str = "/tools/anchor-runtime/anchor-channel-send";

/// Where a delegated bot's control surface appears inside an `Op.run` sandbox.
/// A Run only gets the entries its Graph is authorized for.
pub(crate) const CHANNEL_MOUNT: &str = "/channel";

/// The Run identity a delegated send is attributed to. It comes from the Host,
/// through the sandbox environment, so a node cannot choose its own idempotency
/// key any more than it can choose which bot it was granted.
struct SendIdentity {
    run_id: String,
    node_id: String,
    invocation: u64,
}

impl SendIdentity {
    fn from_environment() -> Result<Self, String> {
        Self::from_parts(
            std::env::var("ANCHOR_RUN_ID").ok(),
            std::env::var("ANCHOR_NODE").ok(),
            std::env::var("ANCHOR_INVOCATION").ok(),
        )
    }

    fn from_parts(
        run_id: Option<String>,
        node_id: Option<String>,
        invocation: Option<String>,
    ) -> Result<Self, String> {
        let run_id = run_id
            .filter(|value| !value.trim().is_empty())
            .ok_or("anchor-channel-send must run as a Graph node")?;
        let node_id = node_id
            .filter(|value| !value.trim().is_empty())
            .ok_or("anchor-channel-send must run as a Graph node")?;
        let invocation = invocation
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|invocation| *invocation > 0)
            .ok_or("anchor-channel-send must run as a Graph node invocation")?;
        Ok(Self {
            run_id,
            node_id,
            invocation,
        })
    }
}

pub(crate) fn shell_command(command: &str, allowed: &[String]) -> Result<Vec<String>, String> {
    if command.trim().is_empty() || command.contains('\0') {
        return Err("Op.run must be a non-empty shell command without NUL bytes".into());
    }
    if !allowed.iter().any(|name| name == "sh") {
        return Err("Op.run requires `sh` in ANCHOR_RUNNER_ALLOWED_COMMANDS".into());
    }
    Ok(vec!["sh".into(), "-c".into(), command.into()])
}

pub(crate) fn route_helper_mount() -> Result<ReadOnlyInput, String> {
    Ok(ReadOnlyInput::new(
        std::env::current_exe().map_err(|error| format!("route helper unavailable: {error}"))?,
        ROUTE_HELPER,
    ))
}

/// The same binary under a second name: `anchor-channel-send` is the only way
/// an `Op.run` node reaches a delegated bot's gateway.
pub(crate) fn channel_send_helper_mount() -> Result<ReadOnlyInput, String> {
    Ok(ReadOnlyInput::new(
        std::env::current_exe()
            .map_err(|error| format!("channel send helper unavailable: {error}"))?,
        CHANNEL_SEND_HELPER,
    ))
}

pub(crate) fn add_route_helper(request: &mut SandboxRequest, mount: ReadOnlyInput) {
    request.readonly_inputs.push(mount);
    let directories = std::iter::once("/tools/anchor-runtime".to_owned())
        .chain(
            request
                .tool_dirs
                .iter()
                .map(|path| path.to_string_lossy().into_owned()),
        )
        .chain(["/usr/bin".into(), "/bin".into()])
        .collect::<Vec<_>>()
        .join(":");
    request
        .environment
        .push(SandboxEnvironment::new("PATH", directories));
}

/// The two mounts one delegated bot contributes: its control descriptor and its
/// control socket. The descriptor is read by the helper for the gateway token,
/// so both files are what the Run is granted; the Host decides the set, never
/// the Graph.
pub(crate) fn channel_mounts(bot: &str, descriptor: &Path, socket: &Path) -> [ReadOnlyInput; 2] {
    let root = format!("{CHANNEL_MOUNT}/{bot}");
    [
        ReadOnlyInput::new(descriptor, format!("{root}/control.json")),
        ReadOnlyInput::new(socket, format!("{root}/control.sock")),
    ]
}

/// A read-only mount of this binary named anchor-route needs no installed
/// Anchor source, console script, or interpreter.
pub(crate) fn route_cli_args() -> Option<Vec<String>> {
    let mut args = std::env::args();
    if Path::new(&args.next()?).file_name()? == "anchor-route" {
        return Some(args.collect());
    }
    if args.next().as_deref() == Some("anchor-route") {
        return Some(args.collect());
    }
    None
}

pub(crate) fn run_route_cli(args: &[String]) -> i32 {
    let (target, reason) = match parse_route_args(args) {
        Ok(Some(arguments)) => arguments,
        Ok(None) => {
            println!("usage: anchor-route --to TARGET [--reason REASON]");
            return 0;
        }
        Err(error) => {
            eprintln!("anchor-route: {error}");
            return 2;
        }
    };
    let routes = std::env::var("ANCHOR_ROUTES").unwrap_or_default();
    let allowed = routes
        .split(',')
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if allowed.is_empty() {
        eprintln!("anchor-route: this node has no route to choose");
        return 1;
    }
    if !allowed.contains(&target.as_str()) {
        eprintln!(
            "anchor-route: {target:?} is not a way out of this node. Choose one of: {}",
            allowed.join(", ")
        );
        return 1;
    }
    println!("ANCHOR_ROUTE: {target}");
    if !reason.is_empty() {
        println!("{reason}");
    }
    0
}

pub(crate) fn channel_send_cli_args() -> Option<Vec<String>> {
    let mut args = std::env::args();
    if Path::new(&args.next()?).file_name()? == "anchor-channel-send" {
        return Some(args.collect());
    }
    if args.next().as_deref() == Some("anchor-channel-send") {
        return Some(args.collect());
    }
    None
}

/// `anchor-channel-send --bot BOT --userid USERID`, content on stdin.
///
/// The bot set is not a command argument's promise: the Host mounts only the
/// gateways this Run's Graph was delegated, so a missing `/channel/<bot>` is the
/// authorization check. The gateway then enforces the recipient list on its own.
pub(crate) fn run_channel_send_cli(args: &[String]) -> i32 {
    let content = {
        let mut content = String::new();
        match std::io::Read::read_to_string(&mut std::io::stdin(), &mut content) {
            Ok(_) => content,
            Err(error) => {
                eprintln!("anchor-channel-send: cannot read content: {error}");
                return 2;
            }
        }
    };
    let result = SendIdentity::from_environment()
        .and_then(|identity| channel_send(args, &content, Path::new(CHANNEL_MOUNT), &identity));
    match result {
        Ok(receipt) => {
            println!("{receipt}");
            0
        }
        Err(error) => {
            eprintln!("anchor-channel-send: {error}");
            2
        }
    }
}

fn channel_send(
    args: &[String],
    content: &str,
    mount: &Path,
    identity: &SendIdentity,
) -> Result<String, String> {
    let request = parse_channel_args(args)?;
    let Some(request) = request else {
        return Ok("usage: anchor-channel-send --bot BOT --userid USERID < content".into());
    };
    if !anchor_runtime::identifier::is_name(&request.bot) {
        return Err(format!("{:?} is not a Graph name", request.bot));
    }
    let root = mount.join(&request.bot);
    if !root.is_dir() {
        return Err(format!(
            "no bot is delegated to this Graph under {}",
            root.display()
        ));
    }
    let content = content.trim_end_matches('\n');
    if content.trim().is_empty() {
        return Err("content is empty".into());
    }
    let descriptor = root.join("control.json");
    let raw =
        std::fs::read(&descriptor).map_err(|_| "this Run was not granted that bot".to_owned())?;
    let value: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|_| "the granted bot descriptor is unreadable".to_owned())?;
    let token = value
        .get("token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or("the granted bot descriptor has no control token")?;
    let request_id = channel_request_id(identity, &request.bot, &request.userid);
    let delivery = channel_control::send_frame(&request_id, &request.userid, content, token)
        .and_then(|frame| {
            channel_control::send_blocking(&root.join("control.sock"), &frame, &request_id)
        });
    match delivery {
        Ok(_) => {}
        Err(SendError::Refused(reason) | SendError::Gateway(reason)) => return Err(reason),
        Err(SendError::Uncertain) => {
            return Err("channel delivery was not confirmed; do not resend automatically".into());
        }
    }
    Ok(serde_json::json!({
        "accepted": true,
        "request_id": request_id,
    })
    .to_string())
}

struct ChannelRequest {
    bot: String,
    userid: String,
}

fn parse_channel_args(args: &[String]) -> Result<Option<ChannelRequest>, String> {
    let mut bot = None;
    let mut userid = None;
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        let value = match argument.as_str() {
            "--help" | "-h" => return Ok(None),
            "--bot" | "--userid" => args
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))?
                .clone(),
            _ if argument.starts_with("--bot=") => argument[6..].to_owned(),
            _ if argument.starts_with("--userid=") => argument[9..].to_owned(),
            _ => return Err(format!("unrecognized argument {argument:?}")),
        };
        if argument == "--bot" || argument.starts_with("--bot=") {
            bot = Some(value);
        } else {
            userid = Some(value);
        }
    }
    Ok(Some(ChannelRequest {
        bot: bot.ok_or("--bot is required")?,
        userid: userid.ok_or("--userid is required")?,
    }))
}

/// Stable per (Run, node, invocation, bot, recipient) and independent of the
/// message body.
///
/// A retried node keeps its invocation number, so it recomputes the same id and
/// the gateway answers with a duplicate receipt instead of sending again. Two
/// different Runs never collide, so a weekly report that happens to repeat last
/// week's wording is still delivered — hashing the text would silently drop it
/// as a duplicate.
fn channel_request_id(identity: &SendIdentity, bot: &str, userid: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"anchor-channel-delegated-send-v1\0");
    digest.update((identity.run_id.len() as u64).to_be_bytes());
    digest.update(identity.run_id.as_bytes());
    digest.update((identity.node_id.len() as u64).to_be_bytes());
    digest.update(identity.node_id.as_bytes());
    digest.update(identity.invocation.to_be_bytes());
    for field in [bot, userid] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field.as_bytes());
    }
    format!("channel-delegated-{:x}", digest.finalize())
}

fn parse_route_args(args: &[String]) -> Result<Option<(String, String)>, String> {
    let mut target = None;
    let mut reason = String::new();
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        let value = match argument.as_str() {
            "--help" | "-h" => return Ok(None),
            "--to" | "--reason" => args
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))?
                .clone(),
            _ if argument.starts_with("--to=") => argument[5..].to_owned(),
            _ if argument.starts_with("--reason=") => argument[9..].to_owned(),
            _ => return Err(format!("unrecognized argument {argument:?}")),
        };
        if argument == "--to" || argument.starts_with("--to=") {
            target = Some(value);
        } else {
            reason = value;
        }
    }
    Ok(Some((target.ok_or("--to is required")?, reason)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    /// One connected gateway: reads the request and answers with `response`.
    fn gateway(response: impl Into<String>) -> (tempfile::TempDir, String) {
        let response = response.into();
        let root = tempfile::tempdir().unwrap();
        let bot = root.path().join("report-bot");
        std::fs::create_dir_all(&bot).unwrap();
        let socket = bot.join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::write(
            bot.join("control.json"),
            serde_json::json!({"socket": socket.display().to_string(), "token": "t".repeat(40)})
                .to_string(),
        )
        .unwrap();
        let path = root.path().display().to_string();
        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut line = String::new();
            let _ = BufReader::new(&stream).read_line(&mut line);
            let mut writer = &stream;
            let _ = writer.write_all(response.as_bytes());
            let _ = writer.write_all(b"\n");
            let _ = writer.flush();
        });
        (root, path)
    }

    fn args(bot: &str, userid: &str) -> Vec<String> {
        vec!["--bot".into(), bot.into(), "--userid".into(), userid.into()]
    }

    fn identity(run: &str, node: &str, invocation: u64) -> SendIdentity {
        SendIdentity {
            run_id: run.into(),
            node_id: node.into(),
            invocation,
        }
    }

    #[test]
    fn an_ungranted_bot_has_no_mount_and_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let error = channel_send(
            &args("report-bot", "alice"),
            "hello",
            root.path(),
            &identity("run-1", "notify", 1),
        )
        .unwrap_err();
        assert!(
            error.contains("no bot is delegated to this Graph"),
            "{error}"
        );
    }

    #[test]
    fn a_rejected_request_never_reports_success() {
        let (root, _) = gateway(r#"{"error":"recipient is not allowed"}"#);
        let _ = root;
        let error = channel_send(
            &args("report-bot", "mallory"),
            "hello",
            root.path(),
            &identity("run-1", "notify", 1),
        )
        .unwrap_err();
        assert!(error.contains("not allowed"), "{error}");
    }

    #[test]
    fn an_unaccepted_reply_is_uncertain_and_has_no_receipt() {
        let (root, _) = gateway(r#"{"accepted":false}"#);
        let _ = root;
        let error = channel_send(
            &args("report-bot", "alice"),
            "hello",
            root.path(),
            &identity("run-1", "notify", 1),
        )
        .unwrap_err();
        assert!(error.contains("not confirmed"), "{error}");
    }

    /// An acceptance that names another request may belong to a different
    /// send, so it is not this delivery's receipt.
    #[test]
    fn an_ack_for_another_request_is_not_a_receipt() {
        let (root, _) = gateway(r#"{"accepted":true,"request_id":"channel-delegated-other"}"#);
        let error = channel_send(
            &args("report-bot", "alice"),
            "hello",
            root.path(),
            &identity("run-1", "notify", 1),
        )
        .unwrap_err();
        assert!(error.contains("not confirmed"), "{error}");
    }

    #[test]
    fn an_ack_for_this_request_is_the_receipt() {
        let sender = identity("run-1", "notify", 1);
        let expected = channel_request_id(&sender, "report-bot", "alice");
        let (root, _) = gateway(
            serde_json::json!({"accepted": true, "request_id": expected, "duplicate": true})
                .to_string(),
        );
        let receipt: serde_json::Value = serde_json::from_str(
            &channel_send(&args("report-bot", "alice"), "hello", root.path(), &sender).unwrap(),
        )
        .unwrap();
        assert_eq!(
            receipt,
            serde_json::json!({"accepted": true, "request_id": expected})
        );
    }

    #[test]
    fn a_retried_node_reuses_its_request_id() {
        let first = channel_request_id(&identity("run-1", "notify", 1), "report-bot", "alice");
        assert_eq!(
            first,
            channel_request_id(&identity("run-1", "notify", 1), "report-bot", "alice")
        );
        assert_ne!(
            first,
            channel_request_id(&identity("run-2", "notify", 1), "report-bot", "alice")
        );
        assert_ne!(
            first,
            channel_request_id(&identity("run-1", "notify", 2), "report-bot", "alice")
        );
        assert_ne!(
            first,
            channel_request_id(&identity("run-1", "notify", 1), "report-bot", "bob")
        );
        assert_ne!(
            first,
            channel_request_id(&identity("run-1", "notify", 1), "other-bot", "alice")
        );
    }

    /// The regression this guards: a weekly report that repeats last week's
    /// wording is a new delivery. Hashing the body would turn it into a
    /// duplicate the gateway acknowledges without sending.
    #[test]
    fn identical_text_in_a_new_run_is_a_new_delivery() {
        let last_week = channel_request_id(&identity("run-1", "notify", 1), "bot", "alice");
        let this_week = channel_request_id(&identity("run-2", "notify", 1), "bot", "alice");
        assert_ne!(last_week, this_week);
    }

    #[test]
    fn the_helper_refuses_to_run_outside_a_graph_node() {
        // Without a Run identity the helper cannot mint an idempotency key, so
        // invoking it outside an Op.run must fail rather than guess one.
        assert!(SendIdentity::from_parts(None, None, None).is_err());
        assert!(
            SendIdentity::from_parts(Some("run-1".into()), Some("notify".into()), None).is_err()
        );
        assert!(
            SendIdentity::from_parts(
                Some("run-1".into()),
                Some("notify".into()),
                Some("0".into())
            )
            .is_err()
        );
        assert!(
            SendIdentity::from_parts(Some(" ".into()), Some("notify".into()), Some("1".into()))
                .is_err()
        );
        assert!(
            SendIdentity::from_parts(
                Some("run-1".into()),
                Some("notify".into()),
                Some("1".into())
            )
            .is_ok()
        );
    }

    #[test]
    fn a_graph_name_cannot_escape_the_mount_root() {
        for hostile in ["..", "../other", "a/b", ""] {
            let error = channel_send(
                &args(hostile, "alice"),
                "hello",
                Path::new("/nonexistent"),
                &identity("run-1", "notify", 1),
            )
            .unwrap_err();
            assert!(
                error.contains("not a Graph name") || error.contains("--bot is required"),
                "{hostile}: {error}"
            );
        }
    }
}
