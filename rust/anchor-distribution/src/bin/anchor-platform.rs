#![forbid(unsafe_code)]
use anchor_distribution::{PlatformInstallRequest, install_platform, platform_command_with_listen};
use clap::{Parser, Subcommand};
use std::{os::unix::process::CommandExt, path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(version, about = "Install and start a private Anchor platform")]
struct Arguments {
    #[command(subcommand)]
    command: Operation,
}

#[derive(Subcommand)]
enum Operation {
    /// Verify a reviewed platform archive and create a fresh private installation.
    Install {
        #[arg(long)]
        archive: PathBuf,
        #[arg(long)]
        sha256: String,
        #[arg(long)]
        install_root: PathBuf,
        /// Private KEY=value data containing model URL, API key and model name.
        #[arg(long)]
        model_config: PathBuf,
    },
    /// Start the Host from the installed release using its private model configuration.
    Start {
        /// Loopback IP and port for the local WebUI/API listener.
        #[arg(long, default_value = "127.0.0.1:8077")]
        listen: String,
    },
}

fn main() -> ExitCode {
    let result = match Arguments::parse().command {
        Operation::Install {
            archive,
            sha256,
            install_root,
            model_config,
        } => install_platform(&PlatformInstallRequest {
            archive,
            sha256,
            install_root,
            model_config,
        })
        .and_then(|report| Ok(serde_json::to_string(&report)?))
        .map(|report| println!("{report}")),
        Operation::Start { listen } => std::env::current_exe()
            .map_err(anchor_distribution::PackageError::Io)
            .and_then(|executable| platform_command_with_listen(&executable, &listen))
            .and_then(|mut command| Err(anchor_distribution::PackageError::Io(command.exec()))),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("anchor-platform: {error}");
            ExitCode::FAILURE
        }
    }
}
