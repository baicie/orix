# 安装管道设计 — 核心编排

## 概述

`crates/core` 是编排层，将所有其他 crate 连接在一起。它暴露供 `crates/cli` 消费的高级 API：`install`、`add`、`remove` 和 store 管理命令。此 crate 了解管道流程，但不涉及任何单个步骤的实现细节。

## 管道架构

```
orix install
│
├─ Config.resolve()          # 加载 .npmrc、env、CLI 参数
│
├─ Manifest.read()           # 解析 package.json
│
├─ Workspace.discover()      # 查找 pnpm-workspace.yaml（如果存在）
│
├─ Lockfile.read()           # 加载现有 lockfile（如果存在）
│
├─ Resolver.resolve()        # 构建依赖图
│   │
│   ├─ Registry.fetch_packument()
│   └─ semver 匹配
│
├─ Fetcher.fetch_all()       # 下载 + 解压所有 tarball
│   │
│   ├─ TarballCache.get_or_fetch()
│   ├─ 验证完整性
│   └─ 解压到临时目录
│
├─ Store.import_package()    # 去重，硬链接到 CAS store
│
├─ Lockfile.update()         # 写入更新后的 lockfile
│
├─ Linker.link_graph()      # 构建 node_modules 结构
│   │
│   ├─ 创建 .pnpm/ 树
│   ├─ 从 store 硬链接
│   └─ 符号链接虚拟依赖
│
└─ done
```

## Manifest（package.json）

```rust
/// 解析后的 package.json
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub name: Option<PackageName>,
    pub version: Version,
    pub description: Option<String>,
    pub dependencies: HashMap<PackageName, VersionConstraint>,
    pub dev_dependencies: HashMap<PackageName, VersionConstraint>,
    pub peer_dependencies: HashMap<PackageName, VersionConstraint>,
    pub optional_dependencies: HashMap<PackageName, VersionConstraint>,
    pub scripts: HashMap<String, String>,
    pub engines: Option<EnginesConstraint>,
    pub os: Vec<String>,
    pub cpu: Vec<String>,
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&source)
            .with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// 返回此 manifest 的 node_modules 有效目录
    pub fn node_modules_dir(&self) -> PathBuf;
}
```

## Install 命令

```rust
/// 顶层安装编排
pub async fn install(project_root: &Path, opts: &InstallOpts) -> Result<InstallReport> {
    let _span = tracing::info_span!("install", root = %project_root.display());
    let start = std::time::Instant::now();

    // 步骤 1：配置
    let config = Config::load(project_root)?;
    if opts.frozen_lockfile && !config.lockfile_path().exists() {
        anyhow::bail!(
            "No lockfile found at {}. Run orix install without --frozen-lockfile first.",
            config.lockfile_path().display()
        );
    }

    // 步骤 2：Manifest
    let manifest = Manifest::read(&project_root.join("package.json"))
        .with_context(|| "failed to read package.json")?;

    // 步骤 3：Workspace（可选）
    let workspace = match Workspace::discover(project_root.clone()) {
        Ok(ws) => Some(ws),
        Err(_) => None,  // 不是 workspace
    };

    // 步骤 4：解析
    let lockfile = match Lockfile::read(&config.lockfile_path()) {
        Ok(lf) => lf,
        Err(_) => Lockfile::empty(),
    };

    let mut resolver = Resolver::new(config.registry.clone());
    let graph = if opts.frozen_lockfile {
        // 使用 lockfile 作为事实来源
        resolver.resolve_from_lockfile(&lockfile, &manifest)?
    } else {
        // 重新解析所有内容
        if let Some(ref ws) = workspace {
            resolver.resolve_with_workspace(&manifest, ws).await?
        } else {
            resolver.resolve_manifest(&manifest).await?
        }
    };

    // 步骤 5：验证
    if opts.frozen_lockfile {
        validate_frozen_lockfile(&lockfile, &manifest, &graph)?;
    }

    // 步骤 6：获取
    let store = Store::open(config.store_dir.clone())?;
    let tarball_cache = TarballCache::new(config.cache_dir.clone());
    let fetcher = Fetcher::new(config.registry.clone());

    let fetch_report = fetcher.fetch_all(&graph, &tarball_cache, &store, config.concurrency).await?;

    // 步骤 7：更新 lockfile
    let updated_lockfile = Lockfile::update(&lockfile, &manifest, &graph);
    updated_lockfile.write(&config.lockfile_path())?;

    // 步骤 8：链接
    let linker = Linker::new(store, project_root.join("node_modules"));
    let link_report = linker.link_graph(&graph, &manifest.dependencies).await?;

    let duration = start.elapsed();
    Ok(InstallReport {
        packages_added: graph.packages.len(),
        fetch_report,
        link_report,
        duration,
    })
}
```

## Add 命令

```rust
pub async fn add(
    project_root: &Path,
    packages: &[String],
    dep_type: DependencyType,
    opts: &InstallOpts,
) -> Result<InstallReport> {
    let manifest_path = project_root.join("package.json");
    let mut manifest = Manifest::read(&manifest_path)?;

    for pkg_spec in packages {
        // 将 "react@^18.2.0" 解析为 (name, constraint)
        let (name, constraint) = parse_package_spec(pkg_spec)?;
        match dep_type {
            DependencyType::Production => { manifest.dependencies.insert(name, constraint); }
            DependencyType::Dev => { manifest.dev_dependencies.insert(name, constraint); }
            DependencyType::Peer => { manifest.peer_dependencies.insert(name, constraint); }
            DependencyType::Optional => { manifest.optional_dependencies.insert(name, constraint); }
        }
    }

    manifest.write(&manifest_path)?;
    install(project_root, opts).await
}
```

## Remove 命令

```rust
pub async fn remove(project_root: &Path, packages: &[String]) -> Result<RemoveReport> {
    let manifest_path = project_root.join("package.json");
    let mut manifest = Manifest::read(&manifest_path)?;

    let mut removed = Vec::new();
    for pkg_name in packages {
        let name = pkg_name.to_string();
        if manifest.dependencies.remove(&name).is_some() {
            removed.push(name.clone());
        }
        if manifest.dev_dependencies.remove(&name).is_some() {
            removed.push(name.clone());
        }
        if manifest.optional_dependencies.remove(&name).is_some() {
            removed.push(name.clone());
        }
    }

    manifest.write(&manifest_path)?;

    // 重新运行安装以清理 node_modules 和更新 lockfile
    let report = install(project_root, &InstallOpts::default()).await?;

    Ok(RemoveReport {
        removed_packages: removed,
        install_report: report,
    })
}
```

## 错误传播

所有来自各个 crate 的错误通过 `crates/core/src/error.rs` 中定义的 `thiserror` 枚举向上冒泡：

```rust
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("manifest error: {0}")]
    Manifest(#[from] ManifestError),

    #[error("resolution error: {0}")]
    Resolution(#[from] ResolutionError),

    #[error("fetch error: {0}")]
    Fetch(#[from] FetchError),

    #[error("store error: {0}")]
    Store(#[from] StoreError),

    #[error("link error: {0}")]
    Link(#[from] LinkError),

    #[error("lockfile error: {0}")]
    Lockfile(#[from] LockfileError),

    #[error("workspace error: {0}")]
    Workspace(#[from] WorkspaceError),
}
```

## 报告

每个命令返回结构化报告：

```rust
pub struct InstallReport {
    pub packages_added: usize,
    pub fetch_report: FetchReport,
    pub link_report: LinkReport,
    pub duration: Duration,
}

pub struct FetchReport {
    pub success: usize,
    pub skipped: usize,     // 已在 store 中
    pub failures: Vec<String>,
}

pub struct RemoveReport {
    pub removed_packages: Vec<String>,
    pub install_report: InstallReport,
}
```

## 生命周期脚本（未来）

linker 成功完成后，`crates/lifecycle` 运行声明的脚本：

```
preinstall → install → postinstall → prepare
```

MVP：脚本被跳过（`--ignore-scripts`）。第八阶段添加：

```rust
pub async fn run_lifecycle_scripts(
    manifest: &Manifest,
    root: &Path,
    config: &Config,
) -> Result<()> {
    if config.ignore_scripts {
        return Ok(());
    }

    let scripts = ["preinstall", "install", "postinstall", "prepare"];
    for script in scripts {
        if let Some(cmd) = manifest.scripts.get(script) {
            tracing::info!("Running {} script...", script);
            run_script(script, cmd, manifest, root, config).await?;
        }
    }
    Ok(())
}
```

脚本在沙箱环境中运行（受限的 PATH，默认无网络访问）。

Phase 8 的完整命令、执行器、安全策略、workspace 作用域和测试计划见 [Lifecycle Scripts](./lifecycle-scripts.md)。
