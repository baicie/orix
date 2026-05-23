use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use orix_config::Config;
use orix_domain::PackageId;
use orix_manifest::Manifest;
use orix_workspace::Workspace;
use tracing::debug;

use super::types::{success_status, LifecycleEvent, ScriptError, ScriptOutput, PATH_SEP};
use super::util::{
    dependency_scripts_allowed, normalize_script_args, path_env_key, sanitize_path_env,
    shell_args_join,
};

/// Script runner for a single package context.
pub struct ScriptRunner {
    config: Config,
    manifest: Manifest,
    project_root: PathBuf,
    workspace: Option<Workspace>,
}

impl ScriptRunner {
    /// Create a new script runner.
    pub fn new(
        config: Config,
        manifest: Manifest,
        project_root: PathBuf,
        workspace: Option<Workspace>,
    ) -> Self {
        Self {
            config,
            manifest,
            project_root,
            workspace,
        }
    }

    /// Check whether scripts are allowed to run.
    pub fn scripts_enabled(&self) -> bool {
        !self.config.ignore_scripts
    }

    /// Check if a dependency package is allowed to run its lifecycle scripts.
    pub fn dependency_scripts_allowed(&self, pkg_name: &str) -> bool {
        dependency_scripts_allowed(&self.config, pkg_name)
    }

    /// Run the full lifecycle chain (preX, X, postX) for a named script.
    ///
    /// For user-initiated runs, trailing args from the CLI are appended only
    /// to the main script, not to pre/post variants.
    pub async fn run_lifecycle_chain(
        &self,
        name: &str,
        args: Vec<String>,
    ) -> Result<Vec<ScriptOutput>, ScriptError> {
        if !self.scripts_enabled() {
            return Err(ScriptError::Disabled);
        }

        let chain = self.manifest.lifecycle_chain(name);
        if chain.is_empty() {
            return Err(ScriptError::MissingScript(
                name.to_string(),
                self.project_root.clone(),
            ));
        }

        let args = normalize_script_args(args);
        let mut outputs = Vec::with_capacity(chain.len());
        for script_ref in &chain {
            let script_args = if script_ref.name == name {
                args.clone()
            } else {
                Vec::new()
            };

            let output = self
                .run_single(&script_ref.name, script_ref.command, script_args)
                .await?;
            outputs.push(output);
        }

        Ok(outputs)
    }

    /// Run a single lifecycle event for a package.
    ///
    /// Does not execute pre/post variants — caller's responsibility.
    pub async fn run_lifecycle(
        &self,
        event: LifecycleEvent,
        _package: &PackageId,
    ) -> Result<(), ScriptError> {
        if !self.scripts_enabled() {
            return Err(ScriptError::Disabled);
        }

        let cmd = self.manifest.script(event.script_name());
        let Some(command) = cmd else {
            return Ok(());
        };

        let output = self
            .run_single(event.script_name(), command, Vec::new())
            .await?;

        if !output.status.success() {
            let code = output.status.code();
            return Err(ScriptError::Failed {
                name: event.script_name().to_string(),
                code,
            });
        }

        Ok(())
    }

    /// Run a user-initiated script, including pre/post chain.
    ///
    /// Returns `MissingScript` if the script does not exist and `if_present`
    /// is false.
    pub async fn run_script(
        &self,
        name: &str,
        args: Vec<String>,
        if_present: bool,
    ) -> Result<Vec<ScriptOutput>, ScriptError> {
        if !self.scripts_enabled() {
            return Err(ScriptError::Disabled);
        }

        if self.manifest.script(name).is_none() {
            if if_present {
                return Ok(Vec::new());
            }
            return Err(ScriptError::MissingScript(
                name.to_string(),
                self.project_root.clone(),
            ));
        }

        self.run_lifecycle_chain(name, args).await
    }

    /// Run a single script command directly.
    async fn run_single(
        &self,
        name: &str,
        command: &str,
        args: Vec<String>,
    ) -> Result<ScriptOutput, ScriptError> {
        let env = self.build_env(name);
        let cwd = self.project_root.clone();

        let full_command = if args.is_empty() {
            command.to_string()
        } else {
            format!("{} {}", command, shell_args_join(&args))
        };

        debug!(
            script = name,
            command = %full_command,
            cwd = %cwd.display(),
            path = env.get("PATH").map(String::as_str).unwrap_or(""),
            "running script"
        );

        let start = Instant::now();
        let child =
            self.spawn_shell(&full_command, &env, &cwd)
                .map_err(|e| ScriptError::Spawn {
                    name: name.to_string(),
                    source: e,
                })?;

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| ScriptError::Spawn {
                name: name.to_string(),
                source: e,
            })?;

        let status = output.status;
        let duration = start.elapsed();

        if !status.success() {
            if status.code().is_none() {
                return Err(ScriptError::Terminated {
                    name: name.to_string(),
                });
            }
            return Err(ScriptError::Failed {
                name: name.to_string(),
                code: status.code(),
            });
        }

        Ok(ScriptOutput {
            name: name.to_string(),
            status,
            duration,
        })
    }

    /// Build the environment for script execution.
    fn build_env(&self, script_name: &str) -> HashMap<String, String> {
        let mut env = HashMap::new();

        // Copy existing environment, excluding variables we'll override.
        for (k, v) in std::env::vars() {
            let key = k.as_str();
            if path_env_key(key).is_none()
                && !matches!(
                    key,
                    "npm_lifecycle_event"
                        | "npm_package_name"
                        | "npm_package_version"
                        | "npm_config_user_agent"
                        | "INIT_CWD"
                        | "ORIX"
                )
            {
                env.insert(k, v);
            }
        }

        // Add orix-specific environment.
        env.insert("ORIX".to_string(), "1".to_string());
        env.insert(
            "INIT_CWD".to_string(),
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        );
        env.insert("npm_lifecycle_event".to_string(), script_name.to_string());

        if let Some(ref name) = self.manifest.name {
            env.insert("npm_package_name".to_string(), name.clone());
        }
        if let Some(ref version) = self.manifest.version {
            env.insert("npm_package_version".to_string(), version.clone());
        }

        env.insert(
            "npm_config_user_agent".to_string(),
            format!("orix/{}", env!("CARGO_PKG_VERSION")),
        );

        // Prepend node_modules/.bin directories to PATH.
        let extra_path = self.build_path();
        if let Some(existing) = std::env::var_os("PATH") {
            let mut combined = extra_path;
            combined.push(PATH_SEP);
            combined.push(&existing);
            let path_str = combined.into_string().unwrap_or_default();
            env.insert(
                path_env_key("PATH").unwrap_or("PATH").to_string(),
                sanitize_path_env(&path_str),
            );
        } else {
            env.insert(
                path_env_key("PATH").unwrap_or("PATH").to_string(),
                extra_path.into_string().unwrap_or_default(),
            );
        }

        env
    }

    /// Build the PATH prefix: project .bin, workspace root .bin, then original PATH.
    /// Returns the extra PATH entries as an OsString (to be prepended to existing PATH).
    /// Directories are included even if they don't yet exist, to aid debugging.
    fn build_path(&self) -> std::ffi::OsString {
        let mut parts = Vec::new();

        // Current project's node_modules/.bin
        let project_bin = self.project_root.join("node_modules").join(".bin");
        parts.push(project_bin.clone());
        debug!(path = %project_bin.display(), "adding project .bin to PATH");

        // Workspace root's node_modules/.bin (if we're in a workspace package)
        if let Some(ref ws) = self.workspace {
            if ws.root != self.project_root {
                let root_bin = ws.root.join("node_modules").join(".bin");
                parts.push(root_bin.clone());
                debug!(path = %root_bin.display(), "adding workspace root .bin to PATH");
            }
        }

        let mut result = std::ffi::OsString::new();
        for (i, part) in parts.into_iter().enumerate() {
            if i > 0 {
                result.push(PATH_SEP);
            }
            result.push(&part);
        }

        result
    }

    /// Spawn a shell process running the given command.
    fn spawn_shell(
        &self,
        command: &str,
        env: &HashMap<String, String>,
        cwd: &Path,
    ) -> std::io::Result<tokio::process::Child> {
        #[cfg(windows)]
        {
            tokio::process::Command::new("cmd.exe")
                .args(["/D", "/S", "/C", command])
                .envs(env.iter())
                .current_dir(cwd)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .spawn()
        }

        #[cfg(not(windows))]
        {
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .envs(env.iter())
                .current_dir(cwd)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .spawn()
        }
    }

    /// Run a script in a specific workspace package.
    pub async fn run_in_workspace(
        &self,
        pkg_name: &str,
        script: &str,
        args: Vec<String>,
        if_present: bool,
    ) -> Result<ScriptOutput, ScriptError> {
        let ws = self.workspace.as_ref().ok_or_else(|| ScriptError::Spawn {
            name: script.to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no workspace available"),
        })?;

        let pkg = ws.find_package_by_name(pkg_name).ok_or_else(|| {
            ScriptError::MissingScript(pkg_name.to_string(), self.project_root.clone())
        })?;

        let runner = ScriptRunner::new(
            self.config.clone(),
            pkg.manifest.clone(),
            pkg.abs_path.clone(),
            self.workspace.clone(),
        );

        let outputs = runner.run_script(script, args, if_present).await?;

        // Return the last output (main script), or a synthetic success if none ran.
        Ok(outputs.into_iter().last().unwrap_or_else(|| ScriptOutput {
            name: script.to_string(),
            status: success_status(),
            duration: Duration::ZERO,
        }))
    }

    /// Run a script recursively across all workspace packages in topological order.
    ///
    /// Only packages that declare the script are executed. Packages without the
    /// script are skipped. Execution stops on the first failure.
    pub async fn run_recursive(
        &self,
        script: &str,
        args: Vec<String>,
        _concurrency: usize,
    ) -> Result<Vec<(String, Result<ScriptOutput, ScriptError>)>, ScriptError> {
        let ws = self.workspace.as_ref().ok_or_else(|| ScriptError::Spawn {
            name: script.to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no workspace available"),
        })?;

        // Topological sort of workspace packages based on their dependencies.
        let sorted = self.topological_sort(ws)?;

        let mut results = Vec::new();

        for pkg in sorted {
            let pkg_name = pkg.manifest.name.clone().unwrap_or_default();

            if pkg.manifest.script(script).is_none() {
                continue;
            }

            let runner = ScriptRunner::new(
                self.config.clone(),
                pkg.manifest.clone(),
                pkg.abs_path.clone(),
                self.workspace.clone(),
            );

            let output = runner.run_script(script, args.clone(), true).await;

            // Extract the last script output (main script in the chain).
            let main_output = match output {
                Ok(outputs) => outputs.into_iter().last(),
                Err(e) => {
                    results.push((pkg_name.clone(), Err(e)));
                    break;
                }
            };

            if let Some(main) = main_output {
                results.push((pkg_name.clone(), Ok(main)));
            }
        }

        Ok(results)
    }

    /// Topological sort of workspace packages based on their inter-dependencies.
    fn topological_sort(
        &self,
        ws: &Workspace,
    ) -> Result<Vec<orix_workspace::WorkspacePackage>, ScriptError> {
        let pkg_names: HashSet<_> = ws
            .packages
            .iter()
            .filter_map(|p| p.manifest.name.clone())
            .collect();

        // Build adjacency list: package -> packages that depend on it.
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        for pkg in &ws.packages {
            if let Some(ref name) = pkg.manifest.name {
                dependents.entry(name.clone()).or_default();
                for dep_name in pkg.manifest.dependencies.keys() {
                    if pkg_names.contains(dep_name) {
                        dependents
                            .entry(dep_name.clone())
                            .or_default()
                            .push(name.clone());
                    }
                }
            }
        }

        // Compute in-degrees.
        let mut in_degree: HashMap<String, usize> =
            HashMap::from_iter(dependents.keys().cloned().map(|k| (k, 0)));
        for deps in dependents.values() {
            for dep in deps {
                *in_degree.entry(dep.clone()).or_insert(0) += 1;
            }
        }

        // Kahn's algorithm.
        let mut queue: Vec<_> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(k, _)| k.clone())
            .collect();
        queue.sort();

        let mut sorted = Vec::new();
        while let Some(name) = queue.pop() {
            if let Some(pkg) = ws
                .packages
                .iter()
                .find(|p| p.manifest.name.as_ref() == Some(&name))
            {
                sorted.push(pkg.clone());
            }

            if let Some(deps) = dependents.get(&name) {
                for dep in deps {
                    if let Some(d) = in_degree.get_mut(dep) {
                        *d -= 1;
                        if *d == 0 {
                            queue.push(dep.clone());
                            queue.sort();
                        }
                    }
                }
            }
        }

        // Compute names of already-sorted packages.
        let sorted_names: HashSet<String> = sorted
            .iter()
            .filter_map(|p| p.manifest.name.clone())
            .collect();

        // Add packages not in the dependency graph.
        for pkg in &ws.packages {
            if let Some(ref name) = pkg.manifest.name {
                if !sorted_names.contains(name) {
                    sorted.push(pkg.clone());
                }
            }
        }

        Ok(sorted)
    }
}
