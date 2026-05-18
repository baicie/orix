//! xtask — development automation for orix.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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
    /// Run the same checks as CI (format, check, clippy, test, docs, deny).
    ///
    /// Use before pushing or opening a PR to catch issues locally.
    CiLocal,
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
        /// Force re-release: yank existing crates when publishing, and recreate the git tag.
        /// Use when re-publishing or re-triggering the same version release.
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
    /// Build and package release binaries for the current platform.
    ///
    /// Creates a zip (Windows) or tar.gz (Unix) archive in dist/.
    Dist {
        /// Override the version (defaults to Cargo.toml version).
        #[arg(long, value_name = "X.Y.Z")]
        version: Option<String>,
        /// Append a minute-precision build timestamp to artifact names for local previews.
        #[arg(long)]
        preview: bool,
        /// Only build, skip packaging.
        #[arg(long)]
        bins_only: bool,
        /// Output directory (defaults to dist/).
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Build a Windows MSI installer using WiX Toolset.
    ///
    /// Downloads WiX automatically if not found. Creates an MSI with
    /// path-selection UI, Start Menu shortcut, and desktop shortcut.
    Msi {
        /// Override the version (defaults to Cargo.toml version).
        #[arg(long, value_name = "X.Y.Z")]
        version: Option<String>,
        /// Append a minute-precision build timestamp to the MSI filename for local previews.
        #[arg(long)]
        preview: bool,
        /// Output directory (defaults to dist/).
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Task::Check => {
            run("cargo", ["fmt", "--all", "--", "--check"])?;
            run_clippy()?;
            run("cargo", ["test", "--workspace", "--all-features"])?;
            run(
                "cargo",
                ["doc", "--workspace", "--all-features", "--no-deps"],
            )?;
        }
        Task::Fmt => run("cargo", ["fmt", "--all"])?,
        Task::Lint => run_clippy()?,
        Task::Test => run("cargo", ["test", "--workspace", "--all-features"])?,
        Task::Doc => run(
            "cargo",
            ["doc", "--workspace", "--all-features", "--no-deps"],
        )?,
        Task::Security => {
            run_optional("cargo-deny", &["check"])?;
            run_optional("cargo-audit", &["audit"])?;
            run_optional("cargo-machete", &["--with-metadata"])?;
        }
        Task::CiLocal => {
            eprintln!("=== Running CI checks locally ===\n");

            eprintln!("[1/5] Format check...");
            run("cargo", ["fmt", "--all", "--", "--check"])?;

            eprintln!("\n[2/5] Check...");
            run(
                "cargo",
                ["check", "--workspace", "--all-targets", "--all-features"],
            )?;

            eprintln!("\n[3/5] Clippy...");
            run_clippy()?;

            eprintln!("\n[4/5] Tests...");
            run("cargo", ["test", "--workspace", "--all-features"])?;

            eprintln!("\n[5/5] Docs...");
            run(
                "cargo",
                ["doc", "--workspace", "--all-features", "--no-deps"],
            )?;

            eprintln!("\n[bonus] Cargo deny...");
            run_optional("cargo-deny", &["check"])?;

            eprintln!("\n[bonus] Cargo machete...");
            run_optional("cargo-machete", &["--with-metadata"])?;

            eprintln!("\n=== All CI checks passed ===");
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
        Task::Dist {
            version,
            preview,
            bins_only,
            output,
        } => {
            let version = resolve_version(version)?;
            run_dist(&version, preview, bins_only, output.as_deref())?;
        }
        Task::Msi {
            version,
            preview,
            output,
        } => {
            let version = resolve_version(version)?;
            run_msi(&version, preview, output.as_deref())?;
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
        run("cargo", ["fmt", "--all", "--", "--check"])?;
        run_clippy()?;
        run("cargo", ["test", "--workspace", "--all-features"])?;
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
        eprintln!("\n[3/5] Preparing release commit");
        if !dry_run {
            commit_and_push_pending_changes(&tag)?;
        } else {
            eprintln!("  (dry-run) would commit pending changes if the worktree is dirty");
            eprintln!("  (dry-run) would push the current branch to origin");
        }

        eprintln!("\n[4/5] Tagging commit: {tag}");
        if !dry_run {
            if force {
                recreate_release_tag(&tag)?;
            }
            run("git", ["tag", &tag])?;
            run("git", ["tag", "-l", &format!("{tag_prefix}*")])?;
        } else {
            if force {
                eprintln!("  (dry-run) would delete local tag if present: git tag -d {tag}");
                eprintln!("  (dry-run) would delete remote tag if present: git push origin :refs/tags/{tag}");
            }
            eprintln!("  (dry-run) would run: git tag {tag}");
        }

        eprintln!("\n[4/5] Pushing tag to origin...");
        if !dry_run {
            run("git", ["push", "origin", &tag])?;
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

/// Commit and push pending changes before tagging so the release tag includes them.
fn commit_and_push_pending_changes(tag: &str) -> Result<()> {
    let branch = current_branch()?;

    if worktree_has_pending_changes()? {
        eprintln!("  Pending changes detected; committing them before tagging {tag}...");
        run("git", ["add", "-A"])?;
        run(
            "git",
            ["commit", "-m", &format!("chore: prepare release {tag}")],
        )?;
    } else {
        eprintln!("  Worktree clean; no release commit needed.");
    }

    eprintln!("  Pushing current branch {branch} to origin...");
    let refspec = format!("HEAD:{branch}");
    run("git", ["push", "origin", &refspec])
}

fn current_branch() -> Result<String> {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .stdin(Stdio::null())
        .output()
        .context("failed to determine current git branch")?;

    if !output.status.success() {
        bail!("failed to determine current git branch");
    }

    let branch = String::from_utf8(output.stdout)
        .context("git branch output was not valid UTF-8")?
        .trim()
        .to_string();

    if branch.is_empty() {
        bail!("release cannot auto-push from a detached HEAD; check out a branch first");
    }

    Ok(branch)
}

fn worktree_has_pending_changes() -> Result<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .stdin(Stdio::null())
        .output()
        .context("failed to inspect git worktree status")?;

    if !output.status.success() {
        bail!("failed to inspect git worktree status");
    }

    Ok(!output.stdout.is_empty())
}

/// Delete local and remote tags so a forced release can recreate the same version tag.
fn recreate_release_tag(tag: &str) -> Result<()> {
    if local_tag_exists(tag)? {
        eprintln!("  --force: deleting existing local tag {tag}...");
        run("git", ["tag", "-d", tag])?;
    } else {
        eprintln!("  --force: local tag {tag} does not exist; skipping delete.");
    }

    if remote_tag_exists(tag)? {
        eprintln!("  --force: deleting existing remote tag origin/{tag}...");
        let remote_ref = format!(":refs/tags/{tag}");
        run("git", ["push", "origin", &remote_ref])?;
    } else {
        eprintln!("  --force: remote tag origin/{tag} does not exist; skipping delete.");
    }

    Ok(())
}

fn local_tag_exists(tag: &str) -> Result<bool> {
    let tag_ref = format!("refs/tags/{tag}");
    let status = Command::new("git")
        .args(["rev-parse", "-q", "--verify", &tag_ref])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to check local tag {tag}"))?;
    Ok(status.success())
}

fn remote_tag_exists(tag: &str) -> Result<bool> {
    let tag_ref = format!("refs/tags/{tag}");
    let status = Command::new("git")
        .args(["ls-remote", "--exit-code", "--tags", "origin", &tag_ref])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to check remote tag origin/{tag}"))?;

    match status.code() {
        Some(0) => Ok(true),
        Some(2) => Ok(false),
        _ => bail!("failed to check remote tag origin/{tag}"),
    }
}

/// Publish all crates to crates.io in topological order.
fn run_publish_crates(version: &str, force: bool, dry_run: bool) -> Result<()> {
    let token_var = "CARGO_REGISTRY_TOKEN";

    if !dry_run && std::env::var(token_var).is_err() {
        bail!("{token_var} is not set. Set it with:\n  export {token_var}=<your-token>");
    }

    // Step 1: yank existing versions if --force
    if force && !dry_run {
        eprintln!("\n  --force: yanking existing crates at version {version} first...");
        run_yank(CRATE_PUBLISH_ORDER, version)?;
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
            run("cargo", ["publish", "-p", name])?;
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

/// Build and package release binaries for the current platform.
fn run_dist(
    version: &str,
    preview: bool,
    bins_only: bool,
    output_dir: Option<&Path>,
) -> Result<()> {
    let dist_root = output_dir.unwrap_or_else(|| Path::new("dist"));
    let bin_dir = Path::new("target/release");
    let artifact_version = artifact_version(version, preview)?;

    eprintln!("=== Dist: orix v{artifact_version} ===");
    eprintln!("[1/2] Building release binary...");
    run("cargo", ["build", "--release", "--package", "orix-cli"])?;

    let bin_name = if cfg!(windows) { "orix.exe" } else { "orix" };
    let bin_path = bin_dir.join(bin_name);
    if !bin_path.exists() {
        bail!("binary not found at {}", bin_path.display());
    }

    fs::create_dir_all(dist_root).context("failed to create dist directory")?;

    if bins_only {
        eprintln!("[2/2] Skipping packaging (--bins-only).");
        let dest = dist_root.join(format!("orix-{artifact_version}-{bin_name}"));
        fs::copy(&bin_path, &dest).context("failed to copy binary")?;
        eprintln!("Done: {}", dest.display());
        return Ok(());
    }

    eprintln!("[2/2] Packaging...");
    match std::env::consts::OS {
        "windows" => package_zip(&artifact_version, &bin_path, dist_root)?,
        _ => package_tarball(&artifact_version, &bin_path, dist_root)?,
    }

    eprintln!("\n=== Done! Output in {} ===", dist_root.display());
    Ok(())
}

/// Package binary as a .zip archive (Windows).
fn package_zip(version: &str, bin_path: &Path, dist_dir: &Path) -> Result<()> {
    let name = format!("orix-{version}-x86_64-pc-windows-msvc",);
    let archive_path = dist_dir.join(format!("{name}.zip"));

    let file = File::create(&archive_path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut bin_file = File::open(bin_path).context("failed to open binary")?;
    let mut contents = Vec::new();
    bin_file.read_to_end(&mut contents)?;

    zip.start_file(format!("{name}/orix.exe"), options)
        .context("failed to add file to zip")?;
    zip.write_all(&contents)
        .context("failed to write binary to zip")?;
    zip.finish().context("failed to finalize zip")?;

    eprintln!("  Created: {}", archive_path.display());
    Ok(())
}

/// Package binary as a .tar.gz archive (Unix).
fn package_tarball(version: &str, bin_path: &Path, dist_dir: &Path) -> Result<()> {
    let target = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else {
        "unknown"
    };

    let name = format!("orix-{version}-{target}");
    let archive_path = dist_dir.join(format!("{name}.tar.gz"));

    let file = File::create(&archive_path)?;
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);

    tar.append_path_with_name(bin_path, format!("{name}/orix"))
        .context("failed to add binary to tarball")?;
    tar.finish().context("failed to finalize tarball")?;

    eprintln!("  Created: {}", archive_path.display());
    Ok(())
}

/// Build a Windows MSI installer using WiX Toolset.
fn run_msi(version: &str, preview: bool, output_dir: Option<&Path>) -> Result<()> {
    if !cfg!(windows) {
        bail!("MSI packaging is only supported on Windows");
    }

    let dist_root = output_dir.unwrap_or_else(|| Path::new("dist"));
    let wix_path = find_or_install_wix()?;
    let artifact_version = artifact_version(version, preview)?;

    eprintln!("=== MSI: orix v{version} ===");
    eprintln!("[1/3] Building release binary...");
    run("cargo", ["build", "--release", "--package", "orix-cli"])?;

    let bin_path = Path::new("target/release/orix.exe");
    if !bin_path.exists() {
        bail!("binary not found at {}", bin_path.display());
    }

    let bin_dir = Path::new("bin");
    fs::create_dir_all(bin_dir).context("failed to create bin directory")?;
    let bin_exe = bin_dir.join("orix.exe");
    fs::copy(bin_path, &bin_exe).with_context(|| {
        format!(
            "failed to copy {} to {}",
            bin_path.display(),
            bin_exe.display()
        )
    })?;
    eprintln!("  Copied {} -> {}", bin_path.display(), bin_exe.display());

    let wix_src = PathBuf::from("packaging/wix/Product.wxs");
    let wix_compiled = PathBuf::from("packaging/wix/Product_compiled.wxs");
    let wix_obj = PathBuf::from("packaging/wix/Product_compiled.wixobj");
    let msi_path = dist_root.join(format!(
        "orix-{artifact_version}-x86_64-pc-windows-msvc.msi"
    ));

    fs::create_dir_all(dist_root).context("failed to create dist directory")?;

    eprintln!("[2/3] Preparing WXS...");
    let wxs_content = fs::read_to_string(&wix_src)
        .with_context(|| format!("failed to read {}", wix_src.display()))?;
    let source_dir = std::env::current_dir()
        .context("failed to get current directory")?
        .join("bin");
    let wxs_content = wxs_content
        .replace("$(var.SourceDir)", &source_dir.to_string_lossy())
        .replace("$(var.Version)", version);
    fs::write(&wix_compiled, wxs_content)
        .with_context(|| format!("failed to write {}", wix_compiled.display()))?;

    eprintln!("[3/3] Compiling MSI...");
    let candle = wix_path.join("candle.exe");
    let light = wix_path.join("light.exe");

    run_with_output(
        &candle,
        [
            "-nologo",
            "-out",
            &wix_obj.to_string_lossy(),
            &wix_compiled.to_string_lossy(),
        ],
    )?;
    run_with_output(
        &light,
        [
            "-nologo",
            "-ext",
            "WixUIExtension",
            "-out",
            &msi_path.to_string_lossy(),
            &wix_obj.to_string_lossy(),
        ],
    )?;

    eprintln!("\n=== Done! ===");
    eprintln!("  MSI: {}", msi_path.display());
    eprintln!("  Run the MSI to install with path-selection UI.");

    Ok(())
}

fn artifact_version(version: &str, preview: bool) -> Result<String> {
    if !preview {
        return Ok(version.to_string());
    }

    Ok(format!("{version}-{}", preview_timestamp()?))
}

fn preview_timestamp() -> Result<String> {
    let now = time::OffsetDateTime::now_local()
        .or_else(|_| Ok::<_, time::error::IndeterminateOffset>(time::OffsetDateTime::now_utc()))
        .context("failed to determine preview timestamp")?;
    let format = time::macros::format_description!(
        "[year][month padding:zero][day padding:zero][hour padding:zero][minute padding:zero]"
    );

    now.format(&format)
        .context("failed to format preview timestamp")
}

/// Find WiX Toolset in PATH or install it to a temp directory.
fn find_or_install_wix() -> Result<PathBuf> {
    // Check PATH first
    if let Ok(path) = which::which("candle.exe") {
        if which::which("light.exe").is_ok() {
            #[allow(clippy::expect_used)]
            let dir = path
                .parent()
                .expect("candle.exe has a parent dir")
                .to_path_buf();
            eprintln!("  Using WiX from PATH: {}", dir.display());
            return Ok(dir);
        }
    }

    // Check common install location
    let program_files = std::env::var("WIX").ok();
    if let Some(wix) = program_files {
        let path = PathBuf::from(&wix);
        let bin_path =
            if path.join("bin/candle.exe").exists() || path.join("bin\\candle.exe").exists() {
                path.join("bin")
            } else if path.join("candle.exe").exists() {
                path.clone()
            } else {
                eprintln!(
                    "  WIX is set to {} but candle.exe not found, skipping...",
                    path.display()
                );
                return find_or_download_wix();
            };
        eprintln!("  Using WiX from WIX env: {}", bin_path.display());
        return Ok(bin_path);
    }

    // Download and extract WiX
    eprintln!("  WiX not found, downloading...");
    find_or_download_wix()
}

fn find_or_download_wix() -> Result<PathBuf> {
    let wix_version = "wix3141rtm";
    let temp_dir = std::env::temp_dir().join(format!("wix-{wix_version}"));
    let zip_path = temp_dir.join("wix.zip");

    if !temp_dir.join("candle.exe").exists() {
        fs::create_dir_all(&temp_dir).context("failed to create WiX temp directory")?;

        eprintln!("  Downloading WiX {wix_version}...");
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("failed to build HTTP client")?;
        let mut resp = client
            .get(format!(
                "https://github.com/wixtoolset/wix3/releases/download/{wix_version}/wix314-binaries.zip"
            ))
            .send()
            .context("failed to download WiX")?;

        let mut file = File::create(&zip_path).context("failed to create WiX zip file")?;
        resp.copy_to(&mut file).context("failed to write WiX zip")?;

        eprintln!("  Extracting WiX...");
        let zip_file = File::open(&zip_path)?;
        let mut archive =
            zip::ZipArchive::new(zip_file).context("failed to read WiX zip archive")?;
        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .with_context(|| format!("failed to read zip entry {i}"))?;
            let outpath = match file.enclosed_name() {
                Some(path) => temp_dir.join(path),
                None => continue,
            };
            if file.name().ends_with('/') {
                fs::create_dir_all(&outpath)?;
            } else {
                if let Some(parent) = outpath.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut outfile = File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;
            }
        }
    }

    let wix_bin = temp_dir;
    eprintln!("  WiX ready: {}", wix_bin.display());
    Ok(wix_bin)
}

/// Run a command and capture its stdout/stderr to print on error.
fn run_with_output<I, A>(cmd: impl AsRef<Path>, args: I) -> Result<()>
where
    I: IntoIterator<Item = A>,
    A: AsRef<std::ffi::OsStr>,
{
    let cmd = cmd.as_ref();
    let args: Vec<String> = args
        .into_iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect();
    let args_str = args.join(" ");
    let output = Command::new(cmd)
        .args(&args)
        .output()
        .with_context(|| format!("failed to run {} {}", cmd.display(), args_str))?;

    if !output.status.success() {
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        bail!("command failed: {} {}", cmd.display(), args_str);
    }

    Ok(())
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
        [
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
