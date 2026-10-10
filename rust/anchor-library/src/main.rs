#![forbid(unsafe_code)]

use std::{path::PathBuf, process::ExitCode};

use anchor_library::{EnvironmentStore, InstallRequest, Library};
use clap::{Parser, Subcommand};
use serde_json::Value;

#[derive(Parser)]
#[command(
    name = "anchor-library",
    about = "Operator Plugin installation, review and dependency environments"
)]
struct Cli {
    #[arg(long)]
    root: PathBuf,
    #[command(subcommand)]
    command: Operation,
}

#[derive(Subcommand)]
enum Operation {
    /// Review a candidate Plugin, or install it when `--review` is absent.
    Install {
        #[arg(
            long,
            conflicts_with = "directory",
            required_unless_present = "directory"
        )]
        source: Option<String>,
        #[arg(long, conflicts_with = "source", requires = "id")]
        directory: Option<PathBuf>,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        replace_existing: bool,
        /// Print what the candidate declares and would publish, then stop
        /// without touching the Library.
        #[arg(long)]
        review: bool,
    },
    /// Publish dependency environments and manage the deployment name table.
    Env {
        #[command(subcommand)]
        command: EnvOperation,
    },
}

#[derive(Subcommand)]
enum EnvOperation {
    /// Publish a prepared tree as one immutable, content-addressed environment.
    Publish {
        #[arg(long)]
        directory: PathBuf,
    },
    /// List published environments without re-hashing them.
    List,
    /// Re-hash one published environment.
    Show {
        #[arg(long)]
        id: String,
    },
    /// Re-hash every environment and check every name target.
    Verify,
    /// Read or change the deployment name table.
    Name {
        #[command(subcommand)]
        command: EnvNameOperation,
    },
}

#[derive(Subcommand)]
enum EnvNameOperation {
    List,
    /// Point a name at a published environment.
    Set {
        name: String,
        #[arg(long)]
        id: String,
        /// The version you saw; omit to assert the name does not exist yet.
        #[arg(long)]
        expect_version: Option<u64>,
    },
    /// Detach a name. This never deletes published content, and it always
    /// states the version that was observed.
    Remove {
        name: String,
        /// The version you saw; obtain it from `env name list`.
        #[arg(long)]
        expect_version: u64,
    },
}

/// Everything the CLI can print is JSON; `verify` also decides the exit status.
fn main() -> ExitCode {
    let cli = Cli::parse();
    let (value, status) = match run(cli) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    match serde_json::to_string(&value) {
        Ok(json) => {
            println!("{json}");
            status
        }
        Err(_) => {
            eprintln!("cannot encode the result as JSON");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(Value, ExitCode), String> {
    let root = cli.root;
    let library = Library::new(root.clone());
    let environments = EnvironmentStore::new(root);
    match cli.command {
        Operation::Install {
            source,
            directory,
            id,
            replace_existing,
            review,
        } => {
            let report = match (directory, review) {
                (Some(directory), true) => library
                    .review_directory(id.as_deref().unwrap(), directory)
                    .map(|review| serde_json::to_value(review).unwrap_or(Value::Null)),
                (Some(directory), false) => library
                    .install_directory(id.as_deref().unwrap(), directory, replace_existing)
                    .map(|outcome| serde_json::to_value(outcome).unwrap_or(Value::Null)),
                (None, true) => library
                    .review(&InstallRequest {
                        source: source.unwrap_or_default(),
                        id,
                        replace_existing,
                    })
                    .map(|review| serde_json::to_value(review).unwrap_or(Value::Null)),
                (None, false) => library
                    .install(&InstallRequest {
                        source: source.unwrap_or_default(),
                        id,
                        replace_existing,
                    })
                    .map(|outcome| serde_json::to_value(outcome).unwrap_or(Value::Null)),
            };
            report
                .map(|value| (value, ExitCode::SUCCESS))
                .map_err(|error| error.to_string())
        }
        Operation::Env { command } => {
            let outcome: Result<Value, anchor_library::EnvironmentError> = match command {
                EnvOperation::Publish { directory } => environments.publish(&directory).map(value),
                EnvOperation::List => environments.list().map(value),
                EnvOperation::Show { id } => environments.show(&id).map(value),
                EnvOperation::Verify => {
                    // A non-empty problem list is a failing check, not a result,
                    // and "the check could not run" is a third outcome.
                    let problems = environments.verify().map_err(|error| error.to_string())?;
                    let status = if problems.is_empty() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    };
                    return Ok((
                        serde_json::json!({"ok": problems.is_empty(), "problems": problems}),
                        status,
                    ));
                }
                EnvOperation::Name { command } => match command {
                    EnvNameOperation::List => environments.names().map(value),
                    EnvNameOperation::Set {
                        name,
                        id,
                        expect_version,
                    } => environments.set_name(&name, &id, expect_version).map(value),
                    EnvNameOperation::Remove {
                        name,
                        expect_version,
                    } => environments
                        .remove_name(&name, Some(expect_version))
                        .map(value),
                },
            };
            outcome
                .map(|value| (value, ExitCode::SUCCESS))
                .map_err(|error| error.to_string())
        }
    }
}

/// JSON for CLI output; a value that cannot be encoded must not look like success.
fn value<T: serde::Serialize>(input: T) -> Value {
    serde_json::to_value(input).unwrap_or(Value::Null)
}
