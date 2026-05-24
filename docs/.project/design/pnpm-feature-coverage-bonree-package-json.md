# Bonree package.json 涉及的 pnpm 特性设计清单

## 背景

本文基于一个真实根项目 `package.json`，整理 orix 为兼容该类 pnpm 项目需要覆盖的特性。该文件不是完整 pnpm 规范，而是面向实现和验收的特性切片，重点回答：

- manifest 中哪些字段会影响安装、运行脚本和 lockfile。
- 这些字段对应 pnpm 的哪些行为。
- orix 当前应支持到什么程度，哪些可以先透传给外部工具。

该项目是一个私有 ESM 根包，包含普通依赖、开发依赖、大量脚本、workspace 过滤脚本、安装后 hooks 初始化，以及第三方工具配置。

## 特性总览

| 区域 | package.json 用法 | pnpm 语义 | orix 设计归属 | 优先级 |
| --- | --- | --- | --- | --- |
| 依赖声明 | `dependencies`、`devDependencies` | 解析 semver 范围，生成完整依赖图 | `manifest`、`resolver`、`lockfile` | P0 |
| 版本范围 | `^9.0.0`、`8.0.10`、`0.2.5` | caret/range/exact 混合解析 | `domain::VersionConstraint`、`resolver` | P0 |
| scripts | `dev`、`build`、`lint` 等 | `pnpm run <script>` 或隐式脚本执行 | `cli`、`core::script` | P0 |
| `.bin` 命令解析 | `rollup`、`vitest`、`eslint`、`simple-git-hooks` | 将 `node_modules/.bin` 注入 PATH | `core::script`、`linker` | P0 |
| lifecycle | `postinstall: simple-git-hooks` | install 后执行根项目 lifecycle | `core::pipeline`、`core::script` | P0 |
| Windows shell | `if: pnpm run ifc & pnpm run ifc-grandchild` | 通过平台 shell 执行脚本文本 | `core::script` | P0 |
| workspace 过滤 | `pnpm --filter ./example dev` | 按 workspace 包路径选择目标包 | `workspace`、`cli`、`core::script` | P1 |
| recursive workspace | `pnpm -r --parallel --filter "./qiankun/*" dev` | 多包并行/递归执行脚本 | `workspace`、`core::script` | P1 |
| 外部 pnpm 命令 | 脚本内直接调用 `pnpm` | pnpm 作为 shell 命令执行 | 短期透传，长期原生兼容 | P1 |
| 工具配置 | `simple-git-hooks`、`lint-staged` | 包管理器保留未知顶层字段 | `manifest` | P0 |
| engines | `node: >=18.12.0` | 安装/运行前校验 Node 版本 | `manifest`、`core` | P2 |
| private/type | `private: true`、`type: module` | publish 防护、Node ESM 语义 | `manifest`；运行时透传给 Node | P2 |

## Manifest 字段处理

orix 读取 `package.json` 时必须保留并理解以下字段：

```json
{
  "name": "root",
  "private": true,
  "type": "module",
  "scripts": {},
  "dependencies": {},
  "devDependencies": {},
  "engines": {},
  "simple-git-hooks": {},
  "lint-staged": {}
}
```

### 必须结构化解析

- `name`
- `version`，若根项目省略，允许为空。
- `private`
- `type`
- `scripts`
- `dependencies`
- `devDependencies`
- `optionalDependencies`
- `peerDependencies`
- `engines`

### 必须保留但不解释

- `simple-git-hooks`
- `lint-staged`
- 其他未知顶层字段

这些字段会被生命周期脚本和下游工具读取。orix 不需要理解其语义，但不能在 `add/remove`、lockfile 导入导出或 manifest 写回时丢失。

## 依赖与版本范围

该文件混合使用三类版本声明：

```json
"rollup": "^4.60.4",
"vite": "8.0.10",
"@baicie/web-worker-inline": "0.2.5"
```

设计要求：

1. caret range 选择满足范围的最高稳定版本。
2. exact version 必须选择指定版本。
3. devDependencies 在根安装时默认参与解析和链接。
4. dependencies 与 devDependencies 在 lockfile importer 中保持分组，便于 frozen 校验和 diff。
5. registry 镜像版本不同步时，应提供更具体诊断，例如：

```txt
failed to resolve tldts-core@^7.0.31
registry appears stale: parent package requires a version not present in the selected registry
hint: retry with --registry https://registry.npmjs.org/ or wait for the mirror to sync
```

## Scripts 执行语义

该文件中大部分工作流都依赖脚本：

```json
"dev": "rollup -c ./scripts/rollup.config.ts --watch",
"test-unit": "vitest run --project unit-jsdom",
"lint": "eslint --cache .",
"postinstall": "simple-git-hooks"
```

orix 需要支持：

1. `orix run dev`
2. 隐式 run：`orix dev`
3. 脚本名之后的参数原样传给脚本：

```bash
orix dev --environment local
orix run build --config scripts/rollup.config.ts
```

4. 执行 `pre<script>`、`<script>`、`post<script>` 生命周期链。
5. 脚本失败时返回非零退出码，并在 install 中阻断后续流程。

### PATH 注入

脚本执行前必须将以下路径前置到 PATH：

```txt
<project>/node_modules/.bin
<workspace-root>/node_modules/.bin
<original PATH>
```

Windows 额外要求：

- 环境变量名大小写不敏感，必须把 `Path`、`PATH`、`path` 视作同一个变量。
- 子进程中只保留一个 PATH 变量，建议写回 `Path`。
- `.cmd` shim 目标不能使用 `\\?\D:\...` verbatim 路径，否则 Node 可能将入口解析为裸盘符。

## Lifecycle：postinstall

该项目依赖根生命周期：

```json
"postinstall": "simple-git-hooks"
```

其语义是安装完成后初始化 Git hooks：

```json
"simple-git-hooks": {
  "pre-commit": "pnpm lint-staged",
  "commit-msg": "node -e \"import('@baicie/scripts').then(m => m.verifyCommit())\""
}
```

设计要求：

1. 根项目 `postinstall` 在 link 和 lockfile 写入后执行。
2. `simple-git-hooks` 必须能从 `node_modules/.bin` 解析到。
3. `postinstall` 失败时 install 失败。
4. `--ignore-scripts` 跳过该脚本，并在输出中明确标记 scripts skipped。
5. 依赖包 lifecycle 继续受 `allow-scripts` 控制，不因根项目脚本默认开启而全量放开。

## Workspace 相关脚本

该文件通过脚本直接使用 pnpm workspace 能力：

```json
"qk": "pnpm -r --parallel --filter \"./qiankun/*\" dev",
"qk-build": "pnpm -r --parallel --filter \"./qiankun/*\" build",
"backend": "pnpm --filter ./playground/backend dev",
"ex": "pnpm --filter ./example dev"
```

### 短期策略：shell 透传

因为脚本文本中显式写了 `pnpm`，短期内 orix 可以只负责正确执行 shell，要求用户环境中存在 pnpm：

```txt
orix run qk
  -> shell executes: pnpm -r --parallel --filter "./qiankun/*" dev
```

这保证真实项目能跑，但不等于 orix 原生支持 `--filter`。

### 中期策略：原生 workspace run

orix 可增加等价命令：

```bash
orix run --recursive --parallel --filter "./qiankun/*" dev
orix run --filter ./example dev
```

需要支持的 filter 形式：

| filter | 含义 |
| --- | --- |
| `./example` | 匹配 workspace 相对路径 |
| `./playground/backend` | 匹配单个 workspace 包路径 |
| `./qiankun/*` | glob 匹配多个 workspace 包 |
| 包名 | 匹配 `package.json#name` |

递归执行规则：

1. 从 `pnpm-workspace.yaml` 发现 workspace 包。
2. 用 filter 选择目标包。
3. `--parallel` 时并发执行目标包脚本。
4. 未指定 `--parallel` 时按 workspace 依赖拓扑顺序执行。
5. 任一脚本失败时整体失败；后续策略可增加 `--continue-if-failed`。

## Shell 组合与平台差异

该脚本包含 shell 组合：

```json
"if": "pnpm run ifc & pnpm run ifc-grandchild"
```

orix 不应解析脚本文本，而应交给平台 shell：

| 平台 | shell |
| --- | --- |
| Windows | `cmd.exe /D /S /C "<script>"` |
| Unix | `sh -c "<script>"` |

注意：`&` 在 Windows 和 POSIX shell 中都表示命令组合，但语义细节不同。orix 只保证调用项目所在平台的默认 shell，不做跨平台重写。

## 第三方脚本工具

该文件依赖以下 `.bin` 工具：

- `rollup`
- `vitest`
- `cross-env`
- `http-server`
- `rimraf`
- `eslint`
- `prettier`
- `tsc`
- `simple-git-hooks`
- `run-p`，来自 `npm-run-all2`

linker 必须为直接依赖和可见依赖生成可执行 shim：

```txt
node_modules/.bin/rollup.cmd
node_modules/.bin/simple-git-hooks.cmd
node_modules/.bin/run-p.cmd
```

Windows shim 要求：

1. `.cmd` 和 `.ps1` 同时生成。
2. shim 指向真实包目录中的 bin 文件，而不是 store 根目录中的孤立文件。
3. 保留 bin 文件相对 `require()` 能力，例如 Rollup bin 中引用相邻文件。

## engines

该项目声明：

```json
"engines": {
  "node": ">=18.12.0"
}
```

建议实现阶段：

1. P0：解析并写入 lockfile 或 install report，不阻断。
2. P1：在 install/run 时检测当前 Node 版本，不满足时 warning。
3. P2：支持 `.npmrc`/配置项 `engine-strict=true`，不满足时 error。

## private 与 type

```json
"private": true,
"type": "module"
```

设计要求：

- `private`：publish/deploy 相关命令必须尊重；install 阶段只保留。
- `type: module`：orix 不解释 JS 模块系统，但必须保证脚本用 Node 执行时工作目录和 package.json 可见。

## 验收用例

建议为该类项目沉淀以下 fixture：

### 1. 根脚本可解析 `.bin`

```bash
orix run postinstall
```

断言：

- `simple-git-hooks` 从 `node_modules/.bin` 解析成功。
- Windows 下没有 `不是内部或外部命令`。

### 2. 隐式 run

```bash
orix dev --watch
```

断言：

- 等价于 `orix run dev --watch`。
- `--watch` 传给 Rollup，不被 orix CLI 消费。

### 3. workspace 透传

```bash
orix qk
```

短期断言：

- 脚本文本由 shell 执行。
- 若 pnpm 不存在，错误提示指出脚本依赖外部 `pnpm` 命令。

中期断言：

- `orix run --recursive --parallel --filter "./qiankun/*" dev` 可在不调用 pnpm 的情况下执行。

### 4. Windows PATH 大小写

构造父进程同时存在 `Path` 和 `PATH` 的场景。

断言：

- 子进程仅保留一个 PATH 变量。
- `node_modules/.bin` 在 PATH 首位。
- `.cmd` shim 可被 `cmd.exe` 找到。

### 5. registry 镜像不同步

构造 `pkg-a@1.1.0` 依赖 `pkg-b@^1.1.0`，但 registry 只含 `pkg-b@1.0.0`。

断言：

- 报错包含父依赖链和 registry hint。

## 实现优先级

### P0：该项目必须可安装并运行根脚本

- semver 解析与 lockfile 写入。
- `.bin` shim 生成。
- Windows PATH 大小写修正。
- 根 `postinstall`。
- 隐式 `run` 和脚本参数透传。
- 未知顶层字段保留。

### P1：workspace 脚本体验

- `orix run --filter <selector> <script>`。
- `orix run --recursive --parallel <script>`。
- filter 支持路径、glob、包名。
- 缺少外部 pnpm 时给出更明确提示。

### P2：生态增强

- `engine-strict`。
- pnpm lockfile 优先安装。
- registry mirror stale 诊断。
- 更完整 peer dependency 上下文。

## 非目标

本文不要求 orix 解析或重写任意 shell 脚本，例如：

```json
"n": "run-p ex sdk",
"if": "pnpm run ifc & pnpm run ifc-grandchild"
```

这些脚本先按平台 shell 原样执行。orix 只负责：

- 正确的工作目录。
- 正确的环境变量。
- 正确的 PATH。
- 正确传播退出码。

## 详细设计与代码草图

本节给出可直接拆 issue 的实现设计。代码是面向当前 crate 边界的草图，具体字段名可按现有代码微调。

### 1. Manifest：保留未知字段并结构化常用字段

目标：`package.json` 被读入、修改依赖、再写回时，`simple-git-hooks`、`lint-staged` 等未知字段不能丢失。

建议在 `crates/manifest` 的 manifest 类型中保留 `extra_fields`：

```rust
use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub name: Option<String>,
    pub version: Option<String>,

    #[serde(default)]
    pub private: bool,

    #[serde(default, rename = "type")]
    pub module_type: Option<String>,

    #[serde(default)]
    pub scripts: BTreeMap<String, String>,

    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,

    #[serde(default)]
    pub dev_dependencies: BTreeMap<String, String>,

    #[serde(default)]
    pub optional_dependencies: BTreeMap<String, String>,

    #[serde(default)]
    pub peer_dependencies: BTreeMap<String, String>,

    #[serde(default)]
    pub engines: Engines,

    #[serde(flatten)]
    pub extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Engines {
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub npm: Option<String>,
    #[serde(default)]
    pub pnpm: Option<String>,
}
```

读写 API：

```rust
impl Manifest {
    pub fn read(path: &Path) -> Result<Self, ManifestError> {
        let source = std::fs::read_to_string(path)?;
        let manifest = serde_json::from_str(&source)?;
        Ok(manifest)
    }

    pub fn write_preserving_unknown_fields(&self, path: &Path) -> Result<(), ManifestError> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, format!("{json}\n"))?;
        Ok(())
    }

    pub fn script(&self, name: &str) -> Option<&str> {
        self.scripts.get(name).map(String::as_str)
    }

    pub fn lifecycle_chain(&self, name: &str) -> Vec<ScriptRef<'_>> {
        [format!("pre{name}"), name.to_string(), format!("post{name}")]
            .into_iter()
            .filter_map(|script_name| {
                self.script(&script_name).map(|command| ScriptRef {
                    name: script_name,
                    command,
                })
            })
            .collect()
    }
}

pub struct ScriptRef<'a> {
    pub name: String,
    pub command: &'a str,
}
```

验收点：

- `simple-git-hooks` 写回后仍存在。
- `lint-staged` 写回后仍存在。
- `scripts` 中以 `#` 开头的分组占位脚本不报错，只作为普通脚本保存。

### 2. CLI：pnpm 风格 run 与隐式 run

该项目依赖：

```bash
oi dev
oi run dev --watch
oi qk
```

目标行为：

- 内置命令优先。
- 非内置命令作为脚本名。
- 脚本名之后的参数全部透传，不再被 clap 当成 orix 参数。

当前可继续使用 clap `external_subcommand`，但长期更稳的做法是两阶段解析。

```rust
pub enum ParsedCommand {
    Builtin(Command),
    Script(RunArgs),
}

pub struct RunArgs {
    pub script: String,
    pub args: Vec<String>,
    pub if_present: bool,
    pub workspace: Option<String>,
    pub recursive: bool,
    pub parallel: bool,
    pub filter: Vec<String>,
    pub concurrency: usize,
}

const BUILTIN_COMMANDS: &[&str] = &[
    "install", "i",
    "add",
    "remove",
    "run",
    "store",
    "cache",
    "import",
    "export",
    "deploy",
    "prune",
];

pub fn parse_implicit_run(rest: Vec<String>) -> Option<RunArgs> {
    let (script, args) = rest.split_first()?;
    if BUILTIN_COMMANDS.contains(&script.as_str()) {
        return None;
    }

    Some(RunArgs {
        script: script.clone(),
        args: args.to_vec(),
        if_present: false,
        workspace: None,
        recursive: false,
        parallel: false,
        filter: Vec::new(),
        concurrency: 4,
    })
}
```

显式 `run` 参数：

```rust
#[derive(clap::Args)]
pub struct RunCliArgs {
    #[arg(long)]
    pub if_present: bool,

    #[arg(long)]
    pub workspace: Option<String>,

    #[arg(long, short = 'r')]
    pub recursive: bool,

    #[arg(long)]
    pub parallel: bool,

    #[arg(long = "filter")]
    pub filter: Vec<String>,

    #[arg(long, default_value_t = 4)]
    pub concurrency: usize,

    pub script: String,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}
```

### 3. ScriptRunner：环境、PATH、shell

脚本执行器负责运行根脚本、workspace 脚本和 lifecycle。

```rust
pub struct ScriptRunner {
    config: Config,
    manifest: Manifest,
    project_root: PathBuf,
    workspace: Option<Workspace>,
}

impl ScriptRunner {
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
}
```

PATH key 处理：

```rust
pub(crate) fn path_env_key(key: &str) -> Option<&'static str> {
    #[cfg(windows)]
    {
        key.eq_ignore_ascii_case("PATH").then_some("Path")
    }

    #[cfg(not(windows))]
    {
        (key == "PATH").then_some("PATH")
    }
}
```

环境构造：

```rust
fn build_env(&self, script_name: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();

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

    env.insert("ORIX".to_string(), "1".to_string());
    env.insert("npm_lifecycle_event".to_string(), script_name.to_string());
    env.insert("npm_config_user_agent".to_string(), format!("orix/{}", env!("CARGO_PKG_VERSION")));

    if let Some(name) = &self.manifest.name {
        env.insert("npm_package_name".to_string(), name.clone());
    }
    if let Some(version) = &self.manifest.version {
        env.insert("npm_package_version".to_string(), version.clone());
    }

    let path = self.script_path();
    env.insert(path_env_key("PATH").unwrap_or("PATH").to_string(), path);
    env
}
```

PATH 拼接：

```rust
fn script_path(&self) -> String {
    let mut parts = Vec::new();
    parts.push(self.project_root.join("node_modules").join(".bin"));

    if let Some(ws) = &self.workspace {
        if ws.root != self.project_root {
            parts.push(ws.root.join("node_modules").join(".bin"));
        }
    }

    if let Some(existing) = std::env::var_os("PATH") {
        parts.push(PathBuf::from(existing));
    }

    let raw = std::env::join_paths(parts)
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    sanitize_path_env(&raw)
}
```

shell 执行：

```rust
fn spawn_shell(
    command: &str,
    env: &HashMap<String, String>,
    cwd: &Path,
) -> std::io::Result<tokio::process::Child> {
    #[cfg(windows)]
    {
        tokio::process::Command::new("cmd.exe")
            .args(["/D", "/S", "/C", command])
            .env_clear()
            .envs(env)
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
            .env_clear()
            .envs(env)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn()
    }
}
```

说明：`env_clear()` 可以避免 Windows 下父进程同时携带 `Path` 和 `PATH` 变体。若后续发现某些系统变量必须继承，应在 `build_env` 中显式白名单复制。

### 4. Linker：bin shim 生成

直接依赖中的 CLI 工具必须在根 `.bin` 生成 shim。

```rust
fn link_package_bins(
    &self,
    pkg_key: &str,
    store_files: &Path,
    link_global_bins: bool,
    report: &mut LinkReport,
) -> Result<()> {
    let manifest = read_package_manifest(store_files)?;
    let Some(bin_entries) = manifest.bin_entries() else {
        return Ok(());
    };

    let package_dir = self.package_dir_in_virtual_store(pkg_key, &manifest.name);
    let global_bin_dir = self.node_modules.join(".bin");

    for (bin_name, bin_path) in bin_entries {
        let package_bin = package_dir.join(&bin_path);
        if !package_bin.exists() {
            continue;
        }

        #[cfg(windows)]
        {
            let target = normalize_windows_node_entry_path(&package_bin.canonicalize()?);
            create_windows_bin_shims(&global_bin_dir, &bin_name, &target)?;
        }

        #[cfg(not(windows))]
        {
            create_unix_bin_symlink(&global_bin_dir, &bin_name, &package_bin)?;
        }
    }

    Ok(())
}
```

Windows 路径标准化：

```rust
fn normalize_windows_node_entry_path(path: &Path) -> PathBuf {
    let raw = path.display().to_string();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}
```

`.cmd` 模板：

```rust
fn windows_cmd_shim(target: &Path) -> String {
    let target = target.display().to_string().replace('/', "\\");
    format!(
        "@ECHO off\r\n\
SETLOCAL\r\n\
SET \"basedir=%~dp0\"\r\n\
IF EXIST \"%basedir%\\node.exe\" (\r\n\
  SET \"_prog=%basedir%\\node.exe\"\r\n\
) ELSE (\r\n\
  SET \"_prog=node\"\r\n\
)\r\n\
\"%_prog%\" \"{target}\" %*\r\n"
    )
}
```

`.ps1` 模板：

```rust
fn windows_ps1_shim(target: &Path) -> String {
    let target = target.display().to_string().replace('\\', "/");
    format!(
        "$basedir = Split-Path $MyInvocation.MyCommand.Definition -Parent\n\
$exe = Join-Path $basedir 'node.exe'\n\
if (Test-Path $exe) {{\n\
  & $exe '{target}' @args\n\
}} else {{\n\
  & node '{target}' @args\n\
}}\n"
    )
}
```

### 5. install pipeline：lifecycle 顺序

面向该项目，安装流程必须保证 `postinstall` 在 link 之后执行：

```txt
read config
read manifest
run root preinstall
resolve graph
fetch packages
link node_modules
write lockfile
run dependency lifecycle allowed by allow-scripts
run root install
run root postinstall
run root prepare
validate layout
finish
```

核心编排草图：

```rust
pub async fn finish_install(ctx: FinishInstallCtx<'_>) -> Result<InstallReport> {
    if !ctx.opts.ignore_scripts {
        run_dependency_lifecycles(
            ctx.graph,
            ctx.config,
            ctx.project_root,
            ctx.progress_tx,
        )
        .await?;
    }

    write_lockfile_if_needed(ctx.lockfile, ctx.project_root)?;

    if !ctx.opts.ignore_scripts {
        run_project_lifecycle(
            ctx.project_root,
            ctx.manifest,
            ctx.config,
            ctx.workspace,
            LifecycleEvent::Install,
            ctx.progress_tx,
        )
        .await?;

        run_project_lifecycle(
            ctx.project_root,
            ctx.manifest,
            ctx.config,
            ctx.workspace,
            LifecycleEvent::Postinstall,
            ctx.progress_tx,
        )
        .await?;

        if ctx.manifest.script("prepare").is_some() {
            run_project_lifecycle(
                ctx.project_root,
                ctx.manifest,
                ctx.config,
                ctx.workspace,
                LifecycleEvent::Prepare,
                ctx.progress_tx,
            )
            .await?;
        }
    }

    validate_layout(ctx.linker, ctx.direct_deps)?;
    Ok(ctx.report)
}
```

建议把过长参数收敛为上下文结构，避免 `finish_install` 参数继续膨胀：

```rust
pub(crate) struct FinishInstallCtx<'a> {
    pub opts: &'a InstallOpts,
    pub config: &'a Config,
    pub manifest: &'a Manifest,
    pub workspace: Option<&'a Workspace>,
    pub graph: &'a DependencyGraph,
    pub lockfile: &'a Lockfile,
    pub linker: &'a Linker,
    pub direct_deps: &'a BTreeSet<String>,
    pub project_root: &'a Path,
    pub progress_tx: &'a Option<mpsc::Sender<InstallEvent>>,
    pub report: InstallReport,
}
```

### 6. Workspace filter 选择器

该项目使用路径 filter：

```bash
pnpm --filter ./example dev
pnpm --filter ./playground/backend dev
pnpm -r --parallel --filter "./qiankun/*" dev
```

orix 原生 selector：

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSelector {
    PackageName(String),
    RelativePath(PathBuf),
    Glob(String),
}

impl WorkspaceSelector {
    pub fn parse(raw: &str) -> Self {
        if raw.starts_with("./") || raw.starts_with("../") {
            if raw.contains('*') || raw.contains('?') || raw.contains('[') {
                Self::Glob(raw.to_string())
            } else {
                Self::RelativePath(PathBuf::from(raw))
            }
        } else {
            Self::PackageName(raw.to_string())
        }
    }
}
```

匹配逻辑：

```rust
pub fn filter_workspace_packages(
    workspace: &Workspace,
    selectors: &[WorkspaceSelector],
) -> Result<Vec<WorkspacePackage>> {
    if selectors.is_empty() {
        return Ok(workspace.packages.clone());
    }

    let mut selected = BTreeMap::new();

    for selector in selectors {
        match selector {
            WorkspaceSelector::PackageName(name) => {
                for pkg in &workspace.packages {
                    if pkg.manifest.name.as_deref() == Some(name.as_str()) {
                        selected.insert(pkg.relative_path.clone(), pkg.clone());
                    }
                }
            }
            WorkspaceSelector::RelativePath(path) => {
                let normalized = normalize_workspace_path(path);
                for pkg in &workspace.packages {
                    if pkg.relative_path == normalized {
                        selected.insert(pkg.relative_path.clone(), pkg.clone());
                    }
                }
            }
            WorkspaceSelector::Glob(pattern) => {
                let matcher = glob::Pattern::new(pattern.trim_start_matches("./"))?;
                for pkg in &workspace.packages {
                    let path = pkg.relative_path.to_string_lossy().replace('\\', "/");
                    if matcher.matches(&path) {
                        selected.insert(pkg.relative_path.clone(), pkg.clone());
                    }
                }
            }
        }
    }

    Ok(selected.into_values().collect())
}
```

### 7. Workspace run 调度

串行拓扑执行：

```rust
pub async fn run_workspace_serial(
    runner_factory: impl Fn(&WorkspacePackage) -> ScriptRunner,
    packages: Vec<WorkspacePackage>,
    script: &str,
    args: Vec<String>,
) -> Result<Vec<WorkspaceScriptResult>, ScriptError> {
    let ordered = topo_sort_workspace_packages(packages);
    let mut results = Vec::new();

    for pkg in ordered {
        if pkg.manifest.script(script).is_none() {
            results.push(WorkspaceScriptResult::skipped(pkg, "missing script"));
            continue;
        }

        let runner = runner_factory(&pkg);
        let output = runner.run_script(script, args.clone(), true).await?;
        results.push(WorkspaceScriptResult::ran(pkg, output));
    }

    Ok(results)
}
```

并行执行：

```rust
pub async fn run_workspace_parallel(
    runner_factory: impl Fn(&WorkspacePackage) -> ScriptRunner + Send + Sync + 'static,
    packages: Vec<WorkspacePackage>,
    script: String,
    args: Vec<String>,
    concurrency: usize,
) -> Result<Vec<WorkspaceScriptResult>, ScriptError> {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency.max(1)));
    let factory = Arc::new(runner_factory);
    let mut set = tokio::task::JoinSet::new();

    for pkg in packages {
        if pkg.manifest.script(&script).is_none() {
            continue;
        }

        let permit = semaphore.clone().acquire_owned().await?;
        let runner = factory(&pkg);
        let script = script.clone();
        let args = args.clone();

        set.spawn(async move {
            let _permit = permit;
            let output = runner.run_script(&script, args, true).await?;
            Ok::<_, ScriptError>(WorkspaceScriptResult::ran(pkg, output))
        });
    }

    let mut results = Vec::new();
    while let Some(result) = set.join_next().await {
        results.push(result.map_err(|_| ScriptError::Terminated {
            name: script.clone(),
        })??);
    }

    Ok(results)
}
```

### 8. Registry mirror stale 诊断

真实场景：`tldts@7.0.31` 已同步，但 `tldts-core@7.0.31` 尚未同步。

解析错误应携带父依赖链：

```rust
#[derive(Debug, Clone)]
pub struct ResolveRequest {
    pub name: PackageName,
    pub constraint: VersionConstraint,
    pub requested_by: Option<PackageId>,
}

#[derive(thiserror::Error, Debug)]
pub enum ResolveError {
    #[error("no version satisfies {name}@{range}")]
    NoMatchingVersion {
        name: PackageName,
        range: String,
        requested_by: Option<PackageId>,
        registry: String,
        latest: Option<String>,
        max_available: Option<String>,
    },
}
```

错误渲染：

```rust
pub fn render_no_matching_version(error: &ResolveError) -> String {
    match error {
        ResolveError::NoMatchingVersion {
            name,
            range,
            requested_by,
            registry,
            latest,
            max_available,
        } => {
            let mut out = format!("failed to resolve {name}@{range}\n");
            if let Some(parent) = requested_by {
                out.push_str(&format!("required by {parent}\n"));
            }
            out.push_str(&format!("registry: {registry}\n"));
            if let Some(max) = max_available {
                out.push_str(&format!("highest available version in registry: {max}\n"));
            }
            if let Some(latest) = latest {
                out.push_str(&format!("dist-tags.latest: {latest}\n"));
            }
            out.push_str("hint: the selected registry mirror may be stale; retry with the official npm registry or a frozen lockfile\n");
            out
        }
    }
}
```

### 9. 外部 pnpm 命令提示

短期内脚本里的 `pnpm` 仍作为外部命令执行。若缺失，应给出更明确提示。

```rust
fn enrich_script_failure(name: &str, command: &str, stderr: &str) -> Option<String> {
    let first = command.split_whitespace().next()?;
    if first == "pnpm" && is_command_not_found(stderr) {
        return Some(format!(
            "script `{name}` calls external command `pnpm`, but it was not found in PATH\n\
hint: install pnpm or replace this script with an orix native workspace command"
        ));
    }
    None
}

fn is_command_not_found(stderr: &str) -> bool {
    stderr.contains("not recognized as an internal or external command")
        || stderr.contains("command not found")
        || stderr.contains("No such file or directory")
}
```

### 10. 测试设计

建议新增 fixture：`tests/fixtures/bonree-like-package`。

文件结构：

```txt
bonree-like-package/
├── package.json
├── pnpm-workspace.yaml
├── example/package.json
├── playground/backend/package.json
└── qiankun/app-a/package.json
```

根 `package.json` 最小化：

```json
{
  "name": "root",
  "private": true,
  "scripts": {
    "dev": "fixture-bin --watch",
    "postinstall": "simple-git-hooks",
    "qk": "pnpm -r --parallel --filter \"./qiankun/*\" dev"
  },
  "devDependencies": {
    "simple-git-hooks": "1.0.0",
    "fixture-bin": "1.0.0"
  },
  "simple-git-hooks": {
    "pre-commit": "pnpm lint-staged"
  }
}
```

Windows PATH 回归测试：

```rust
#[test]
fn script_runner_uses_single_windows_path_key() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = make_fixture_project(temp.path())?;

    let mut cmd = assert_cmd::Command::cargo_bin("orix")?;
    cmd.arg("-C")
        .arg(project)
        .arg("run")
        .arg("postinstall")
        .env("Path", "C:\\Windows\\System32")
        .env("PATH", "D:\\stale-path")
        .assert()
        .success();

    Ok(())
}
```

Shim 路径回归测试：

```rust
#[cfg(windows)]
#[test]
fn windows_bin_shim_does_not_use_verbatim_path() -> anyhow::Result<()> {
    let shim = project.join("node_modules/.bin/rollup.cmd");
    let content = std::fs::read_to_string(shim)?;

    assert!(!content.contains(r"\\?\"));
    Ok(())
}
```

Workspace filter 测试：

```rust
#[test]
fn workspace_filter_matches_relative_glob() -> anyhow::Result<()> {
    let ws = workspace_fixture(&[
        "example",
        "playground/backend",
        "qiankun/app-a",
        "qiankun/app-b",
    ])?;

    let selected = filter_workspace_packages(
        &ws,
        &[WorkspaceSelector::parse("./qiankun/*")],
    )?;

    let paths: Vec<_> = selected
        .iter()
        .map(|pkg| pkg.relative_path.to_string_lossy().replace('\\', "/"))
        .collect();

    assert_eq!(paths, vec!["qiankun/app-a", "qiankun/app-b"]);
    Ok(())
}
```

### 11. Issue 拆分建议

| Issue | 标题 | 验收 |
| --- | --- | --- |
| P0-1 | Manifest 保留未知顶层字段 | `simple-git-hooks`、`lint-staged` 写回不丢 |
| P0-2 | Windows ScriptRunner PATH 规范化 | `simple-git-hooks` 可从 `.bin` 执行 |
| P0-3 | Windows shim 去除 verbatim path | `rollup.cmd --version` 成功 |
| P0-4 | 根 `postinstall` 失败阻断 install | 非零脚本使 install 失败 |
| P0-5 | 隐式 run 与参数透传 | `orix dev --watch` 传参到脚本 |
| P1-1 | workspace `--filter` 选择器 | 路径、glob、包名匹配 |
| P1-2 | workspace recursive/parallel run | 多包脚本并发执行并汇总失败 |
| P2-1 | engine-strict | Node 版本不满足时按配置 warning/error |
| P2-2 | registry mirror stale 诊断 | 报错包含父依赖、registry、hint |
