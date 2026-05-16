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

/// crates.io API endpoint for checking / yanking a specific version.
fn crates_api_url(crate_name: &str, version: &str) -> String {
    format!("https://crates.io/api/v1/crates/{crate_name}/{version}")
}

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
    /// Perform a full release: check → publish crates → git tag → push.
    ///
    /// Version is read from Cargo.toml unless overridden with --version.
    /// Use --dry-run to preview without making any changes.
    Release {
        /// Override the version from Cargo.toml (e.g. "0.2.0").
        /// Also sets the git tag. Requires semver format.
        #[arg(long, value_name = "X.Y.Z")]
        version: Option<String>,
        /// Skip git tag and push (dry-run for the whole flow).
        #[arg(long)]
        dry_run: bool,
        /// Only publish crates to crates.io, skip git tag.
        #[arg(long)]
        crates_only: bool,
        /// Skip publishing to crates.io, only create and push the git tag.
        #[arg(long)]
        skip_crates: bool,
        /// Force re-publish: yank existing crates at this version first.
        /// Use when re-publishing the same version (e.g. after a security fix).
        #[arg(long)]
        force: bool,
        /// Custom version tag prefix (default: "v").
        #[arg(long, default_value = "v")]
        tag_prefix: String,
    },
    /// Publish crates to crates.io in topological order.
    ///
    /// Use --dry-run to show the publish plan without publishing.
    PublishCrates {
        /// Override the version for all crates (instead of reading from Cargo.toml).
        #[arg(long, value_name = "X.Y.Z")]
        version: Option<String>,
        /// Force re-publish: yank existing crates at this version first.
        #[arg(long)]
        force: bool,
        /// Show plan only (no actual publishing).
        #[arg(long, default_value = "false")]
        dry_run: bool,
    },
    /// Yank specific version of crates from crates.io.
    ///
    /// Run before re-publishing the same version with --force.
    Yank {
        /// Version to yank (defaults to Cargo.toml version).
        #[arg(value_name = "VERSION")]
        version: Option<String>,
        /// Crates to yank (defaults to all).
        #[arg(long, value_name = "CRATE")]
        crates: Vec<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Task::Check => {
            run("cargo", &["fmt", "--all", "--", "--check"])?;
            run_clippy()?;
            run("cargo", &["test", "--workspace", "--all-features"])?;
            run(
                "cargo",
                &["doc", "--workspace", "--all-features", "--no-deps"],
            )?;
        }
        Task::Fmt => run("cargo", &["fmt", "--all"])?,
        Task::Lint => run_clippy()?,
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
            force,
            tag_prefix,
            version,
        } => {
            let version = resolve_version(version)?;
            run_release(
                &version,
                dry_run,
                crates_only,
                skip_crates,
                force,
                &tag_prefix,
            )?;
        }
        Task::PublishCrates {
            version,
            force,
            dry_run,
        } => {
            let version = resolve_version(version)?;
            run_publish_crates(&version, force, dry_run)?;
        }
        Task::Yank { version, crates } => {
            let version = resolve_version(version)?;
            let crates: Vec<&str> = if crates.is_empty() {
                CRATE_PUBLISH_ORDER.to_vec()
            } else {
                crates.iter().map(|s| s.as_str()).collect()
            };
            run_yank(&crates, &version)?;
        }
    }

    Ok(())
}

/// Resolve version: CLI override → Cargo.toml.
fn resolve_version(cli_version: Option<String>) -> Result<String> {
    match cli_version {
        Some(v) => {
            if semver::Version::parse(&v).is_err() {
                bail!("--version must be a valid semver (e.g. 0.2.0), got: {v}");
            }
            Ok(v)
        }
        None => read_cargo_toml_version(),
    }
}

/// Full release flow.
fn run_release(
    version: &str,
    dry_run: bool,
    crates_only: bool,
    skip_crates: bool,
    force: bool,
    tag_prefix: &str,
) -> Result<()> {
    let tag = format!("{tag_prefix}{version}");

    eprintln!("=== Release: {tag} ===");

    if dry_run {
        eprintln!("\n[1/5] Skipping checks (--dry-run). Run `cargo xtask check` manually.");
    } else {
        eprintln!("\n[1/5] Running checks...");
        run("cargo", &["fmt", "--all", "--", "--check"])?;
        run_clippy()?;
        run("cargo", &["test", "--workspace", "--all-features"])?;
    }

    if !crates_only && !skip_crates {
        eprintln!("\n[2/5] Publishing crates to crates.io (version {version})...");
        run_publish_crates(version, force, dry_run)?;
    } else if skip_crates {
        eprintln!("\n[2/5] Skipping crates.io publish (--skip-crates)");
    } else {
        eprintln!("\n[2/5] Skipping crates.io publish (--crates-only)");
    }

    if !crates_only {
        eprintln!("\n[3/5] Tagging commit: {tag}");
        if !dry_run {
            run("git", &["tag", &tag])?;
            run("git", &["tag", "-l", &format!("{tag_prefix}*")])?;
        } else {
            eprintln!("  (dry-run) would run: git tag {tag}");
        }

        eprintln!("\n[4/5] Pushing tag to origin...");
        if !dry_run {
            run("git", &["push", "origin", &tag])?;
        } else {
            eprintln!("  (dry-run) would run: git push origin {tag}");
        }
    } else {
        eprintln!("\n[3-5/5] Skipping git tag and push (--crates-only)");
    }

    if !crates_only {
        eprintln!("\n[5/5] GitHub Release artifacts");
        if !dry_run {
            eprintln!(
                "  CI will build binaries and create a GitHub Release.\n  Monitor: https://github.com/baicie/orix/actions"
            );
        } else {
            eprintln!(
                "  (dry-run) CI would build Linux/macOS/Windows binaries and attach to release."
            );
        }
    }

    if dry_run {
        eprintln!("\n=== dry-run complete — no changes made ===");
        eprintln!("Next: run `cargo xtask release` to execute for real.");
    } else {
        eprintln!("\n=== Release {tag} is live! ===");
    }

    Ok(())
}

/// Publish all crates to crates.io in topological order.
fn run_publish_crates(version: &str, force: bool, dry_run: bool) -> Result<()> {
    let token_var = "CARGO_REGISTRY_TOKEN";

    if !dry_run {
        if std::env::var(token_var).is_err() {
            bail!("{token_var} is not set. Set it with:\n  export {token_var}=<your-token>");
        }
    }

    // Step 1: yank existing versions if --force
    if force && !dry_run {
        eprintln!("\n  --force: yanking existing crates at version {version} first...");
        run_yank(&CRATE_PUBLISH_ORDER.to_vec(), version)?;
    }

    // Step 2: update version in Cargo.toml files if --version override was given
    if !dry_run {
        let current = read_cargo_toml_version().unwrap_or_default();
        if current != version {
            set_workspace_version(version)?;
        }
    }

    let total = CRATE_PUBLISH_ORDER.len();
    for (i, name) in CRATE_PUBLISH_ORDER.iter().enumerate() {
        let idx = i + 1;
        if dry_run {
            let extra = if force {
                " (--force, would yank first)"
            } else {
                ""
            };
            eprintln!("\n[{idx}/{total}] Would publish {name} at {version}{extra}");
        } else {
            eprintln!("\n[{idx}/{total}] Publishing {name} at {version}...");
            run("cargo", &["publish", "-p", name])?;
        }
    }

    eprintln!(
        "\nAll crates published{}.",
        if dry_run { " (plan shown above)" } else { "" }
    );
    Ok(())
}

/// Yank crates at a specific version via crates.io API.
fn run_yank(crate_names: &[&str], version: &str) -> Result<()> {
    let token = std::env::var("CARGO_REGISTRY_TOKEN").with_context(|| {
        "CARGO_REGISTRY_TOKEN is not set. Set it with: export CARGO_REGISTRY_TOKEN=<your-token>"
    })?;

    for name in crate_names {
        let url = crates_api_url(name, version);
        eprintln!("  Yanking {name}@{version}...");

        let client = reqwest::blocking::Client::new();
        let resp = client
            .delete(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "orix-release-xtask/0.1.0")
            .send()
            .with_context(|| format!("failed to request {url}"))?;

        let status_code = resp.status().as_u16();
        if resp.status().is_success() {
            eprintln!("    yank OK: {name}@{version}");
        } else if status_code == 404 {
            eprintln!("    skip: {name}@{version} not found on crates.io");
        } else if status_code == 403 {
            bail!("403 Forbidden — ensure CARGO_REGISTRY_TOKEN has publish permission for {name}");
        } else {
            let body = resp.text().unwrap_or_default();
            bail!("failed to yank {name}@{version}: HTTP {status_code} — {body}");
        }
    }

    Ok(())
}

/// Set `version` in all crate Cargo.toml files (used when --version overrides the file).
fn set_workspace_version(version: &str) -> Result<()> {
    eprintln!("  Setting workspace version to {version}...");
    // Update root Cargo.toml
    let root = std::path::Path::new("Cargo.toml");
    let content = std::fs::read_to_string(root).context("failed to read Cargo.toml")?;
    let new_content = content
        .lines()
        .map(|line| {
            if line.trim().starts_with("version") && line.contains('=') {
                format!("version = \"{version}\"")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(root, new_content).context("failed to write Cargo.toml")?;

    // Update each crate's Cargo.toml
    for &name in CRATE_PUBLISH_ORDER {
        let crate_dir = name.strip_prefix("orix-").unwrap_or(name);
        let path = std::path::Path::new("crates")
            .join(crate_dir)
            .join("Cargo.toml");
        if path.exists() {
            let content =
                std::fs::read_to_string(&path).context(format!("read {}", path.display()))?;
            let new_content = content
                .lines()
                .map(|line| {
                    if line.trim() == "version.workspace = true" {
                        format!("version = \"{version}\"")
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(&path, new_content).context(format!("write {}", path.display()))?;
        }
    }

    eprintln!("  Version updated. Remember to commit this change before tagging!");
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

fn run<I, A>(cmd: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = A>,
    A: AsRef<std::ffi::OsStr>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect();
    let args_str = args.join(" ");
    let status = Command::new(cmd)
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to run {cmd} {args_str}"))?;

    if !status.success() {
        bail!("command failed: {cmd} {args_str}");
    }

    Ok(())
}

fn run_clippy() -> Result<()> {
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
    )
}

fn run_optional(cmd: &str, args: &[&str]) -> Result<()> {
    if which::which(cmd).is_err() {
        eprintln!("skip {cmd}: command not installed");
        return Ok(());
    }

    run(cmd, args)
}
