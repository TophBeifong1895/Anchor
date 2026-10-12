//! Supervise Bubblewrap and slirp4netns as one sandbox process tree.
//!
//! The user command starts only after private networking is ready. Host
//! loopback is never forwarded, and helper failure never falls back to the
//! host's network namespace. The caller owns timeout and cancellation.

#[path = "../network_process.rs"]
mod network_process;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match network_process::run(std::env::args_os().skip(1).collect()).await {
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) => {
            eprintln!("anchor-sandbox-network: {error}");
            std::process::ExitCode::from(125)
        }
    }
}
