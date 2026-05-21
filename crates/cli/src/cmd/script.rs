//! Script execution helpers.

use crate::args::RunArgs;
use anyhow::Context;

use orix_core::{normalize_script_args, Manifest, ScriptRunner, Workspace};

use super::{CHECKMARK, CROSS};

pub(crate) async fn run_script(
    project_root: &std::path::Path,
    args: &RunArgs,
) -> anyhow::Result<()> {
    let manifest = Manifest::read(&project_root.join("package.json"))
        .with_context(|| "failed to read package.json")?;
    let config = orix_core::Config::load(project_root)?;
    let workspace = Workspace::discover(project_root.to_path_buf()).ok();
    let script_args = normalize_script_args(args.args.clone());

    if args.recursive {
        let runner = ScriptRunner::new(config, manifest, project_root.to_path_buf(), workspace);
        let results = runner
            .run_recursive(&args.script, script_args, args.concurrency)
            .await?;

        let mut failed = false;
        for (pkg_name, result) in results {
            match result {
                Ok(output) => {
                    println!(
                        " {} {} (exit {})",
                        CHECKMARK,
                        pkg_name,
                        output.status.code().unwrap_or(-1)
                    );
                }
                Err(orix_core::ScriptError::MissingScript(..)) => {
                    println!(" - {} (no script)", pkg_name);
                }
                Err(orix_core::ScriptError::Disabled) => {
                    println!(" - {} (scripts disabled)", pkg_name);
                }
                Err(e) => {
                    eprintln!(" {} {}: {}", CROSS, pkg_name, e);
                    failed = true;
                }
            }
        }

        if failed {
            anyhow::bail!("one or more scripts failed");
        }
    } else if let Some(ref ws_pkg) = args.workspace {
        let runner = ScriptRunner::new(config, manifest, project_root.to_path_buf(), workspace);
        let output = runner
            .run_in_workspace(ws_pkg, &args.script, script_args, args.if_present)
            .await?;
        if !output.status.success() {
            std::process::exit(output.status.code().unwrap_or(-1));
        }
    } else {
        let runner = ScriptRunner::new(config, manifest, project_root.to_path_buf(), workspace);
        let outputs = runner
            .run_script(&args.script, script_args, args.if_present)
            .await?;

        let all_success = outputs.iter().all(|o| o.status.success());
        if let Some(last) = outputs.last() {
            if !last.status.success() {
                std::process::exit(last.status.code().unwrap_or(-1));
            }
        }

        if !all_success {
            anyhow::bail!("one or more scripts in the lifecycle chain failed");
        }
    }

    Ok(())
}
