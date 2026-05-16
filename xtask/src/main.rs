//! xtask — development automation for orix.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::process::{Command, Stdio};

/// Topological order for publishing crates to crates.io.
/// Each crate must be published before any crate that depends on it.
const CRATE_PUBLISH_ORDER: &[&str] = &[
    "orix-domain",
    "orix-manifest",
    "orix-utils",
    "orix-registry",
    "orix-store",
    "orix-lockfile",
    "orix-resolver",
    "orix-linker",
    "orix-workspace",
    "orix-fetcher",
    "orix-config",
    "orix-core",
    "orix-cli",
    "orix-macros",
];

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
    /// Perform a full release: check → tag → push tag.
    ///
    /// Use --dry-run to preview without making any changes.
    /// Use --crates-only to skip the git tag step.
    Release {
        /// Skip git tag and push (dry-run for the whole flow).
        #[arg(long)]
        dry_run: bool,
        /// Only publish crates to crates.io, skip git tag.
        #[arg(long)]
        crates_only: bool,
        /// Skip publishing to crates.io, only create and push the git tag.
        #[arg(long)]
        skip_crates: bool,
        /// Custom version tag prefix (default: "v").
        #[arg(long, default_value = "v")]
        tag_prefix: String,
    },
    /// Publish crates to crates.io in topological order.
    ///
    /// Use --dry-run to validate the publish plan without publishing.
    PublishCrates {
        /// Run cargo publish --dry-run for each crate.
        #[arg(long, default_value = "false")]
        dry_run: bool,
    },
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
        Task::Release {
            dry_run,
            crates_only,
            skip_crates,
            tag_prefix,
        } => {
            run_release(dry_run, crates_only, skip_crates, tag_prefix)?;
        }
        Task::PublishCrates { dry_run } => {
            run_publish_crates(dry_run)?;
        }
    }

    Ok(())
}

/// Full release flow: check → (optional) publish crates → git tag → push.
fn run_release(
    dry_run: bool,
    crates_only: bool,
    skip_crates: bool,
    tag_prefix: String,
) -> Result<()> {
    let version = read_cargo_toml_version()?;
    let tag = format!("{tag_prefix}{version}");

    eprintln!("=== Release: {tag} ===");

    // Step 1: check (skip in dry-run for speed)
    if dry_run {
        eprintln!("\n[1/4] Skipping checks (--dry-run). Run `cargo xtask check` manually.");
    } else {
        eprintln!("\n[1/4] Running checks...");
        run_check()?;
    }

    if !crates_only && !skip_crates {
        // Step 2: publish crates
        eprintln!("\n[2/4] Publishing crates to crates.io...");
        run_publish_crates(dry_run)?;
    } else if skip_crates {
        eprintln!("\n[2/4] Skipping crates.io publish (--skip-crates)");
    } else {
        eprintln!("\n[2/4] Skipping crates.io publish (--crates-only)");
    }

    if !crates_only {
        // Step 3: git tag
        eprintln!("\n[3/4] Tagging commit: {tag}");
        if !dry_run {
            run("git", &["tag", &tag])?;
            run("git", &["tag", "-l", &format!("{tag_prefix}*")])?;
        } else {
            eprintln!("  (dry-run) would run: git tag {tag}");
        }

        // Step 4: push tag
        eprintln!("\n[4/4] Pushing tag to origin...");
        if !dry_run {
            run("git", &["push", "origin", &tag])?;
        } else {
            eprintln!("  (dry-run) would run: git push origin {tag}");
        }
    } else {
        eprintln!("\n[3-4/4] Skipping git tag and push (--crates-only)");
    }

    if dry_run {
        eprintln!("\n=== dry-run complete — no changes made ===");
        eprintln!("Next: run `cargo xtask release` to execute for real.");
    } else {
        eprintln!("\n=== Release {tag} is live! ===");
        eprintln!("  CI will trigger on tag push and build release artifacts.");
        eprintln!("  Monitor: https://github.com/baicie/orix/actions");
    }

    Ok(())
}

fn run_check() -> Result<()> {
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
    Ok(())
}

/// Publish all crates to crates.io in topological order.
fn run_publish_crates(dry_run: bool) -> Result<()> {
    let token_var = "CARGO_REGISTRY_TOKEN";

    if !dry_run && std::env::var(token_var).is_err() {
        bail!("{token_var} is not set. Set it with:\n  export {token_var}=<your-token>");
    }

    for (i, name) in CRATE_PUBLISH_ORDER.iter().enumerate() {
        let n = CRATE_PUBLISH_ORDER.len();
        if dry_run {
            eprintln!("\n[{}/{}] Would publish {name} (dry-run)", i + 1, n);
        } else {
            eprintln!("\n[{}/{}] Publishing {name}...", i + 1, n);
            run("cargo", &["publish", "-p", name])?;
        }
    }

    eprintln!(
        "\nAll crates published{}.",
        if dry_run { " (plan shown above)" } else { "" }
    );
    Ok(())
}

/// Read `version` from the root Cargo.toml.
fn read_cargo_toml_version() -> Result<String> {
    let content = std::fs::read_to_string("Cargo.toml").context("failed to read Cargo.toml")?;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("version") {
            if let Some(v) = line.split('=').nth(1) {
                return Ok(v.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }
    bail!("could not find version in Cargo.toml");
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
