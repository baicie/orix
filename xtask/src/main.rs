//! xtask — development automation for orix.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::process::{Command, Stdio};

#[derive(Debug, Parser)]
#[command(name = "cargo xtask")]
struct Cli {
    #[command(subcommand)]
    command: Task,
}

#[derive(Debug, Subcommand)]
enum Task {
    /// Run formatting, linting, tests, docs, and dependency checks.
    Check,
    /// Format all Rust code.
    Fmt,
    /// Run clippy with warnings denied.
    Lint,
    /// Run workspace tests.
    Test,
    /// Build docs.
    Doc,
    /// Run security and dependency policy checks if tools are installed.
    Security,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Task::Check => {
            run("cargo", &["fmt", "--all", "--", "--check"])?;
            run(
                "cargo",
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--",
                    "-D",
                    "clippy::unwrap_used",
                    "-D",
                    "clippy::dbg_macro",
                    "-W",
                    "clippy::expect_used",
                    "-W",
                    "clippy::panic",
                    "-D",
                    "clippy::todo",
                    "-D",
                    "clippy::large_enum_variant",
                    "-D",
                    "clippy::manual_ok_or",
                    "-D",
                    "unsafe_code",
                    "-W",
                    "missing_docs",
                ],
            )?;
            run("cargo", &["test", "--workspace", "--all-features"])?;
            run(
                "cargo",
                &["doc", "--workspace", "--all-features", "--no-deps"],
            )?;
        }
        Task::Fmt => run("cargo", &["fmt", "--all"])?,
        Task::Lint => run(
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "clippy::unwrap_used",
                "-D",
                "clippy::dbg_macro",
                "-W",
                "clippy::expect_used",
                "-W",
                "clippy::panic",
                "-D",
                "clippy::todo",
                "-D",
                "clippy::large_enum_variant",
                "-D",
                "clippy::manual_ok_or",
                "-D",
                "unsafe_code",
                "-W",
                "missing_docs",
            ],
        )?,
        Task::Test => run("cargo", &["test", "--workspace", "--all-features"])?,
        Task::Doc => run(
            "cargo",
            &["doc", "--workspace", "--all-features", "--no-deps"],
        )?,
        Task::Security => {
            run_optional("cargo-deny", &["check"])?;
            run_optional("cargo-audit", &["audit"])?;
            run_optional("cargo-machete", &["--with-metadata"])?;
        }
    }

    Ok(())
}

fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to run {cmd} {}", args.join(" ")))?;

    if !status.success() {
        bail!("command failed: {cmd} {}", args.join(" "));
    }

    Ok(())
}

fn run_optional(cmd: &str, args: &[&str]) -> Result<()> {
    if which::which(cmd).is_err() {
        eprintln!("skip {cmd}: command not installed");
        return Ok(());
    }

    run(cmd, args)
}
