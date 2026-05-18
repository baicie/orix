# Lifecycle Scripts 设计 — Phase 8

## 概述

Phase 8 为 orix 增加两类脚本能力：

- 用户显式执行的 `orix run <script>`
- 安装流程中由 npm 生态约定触发的 lifecycle scripts

脚本执行是包管理器的安全边界。设计目标不是无条件复制 npm/pnpm 行为，而是在兼容常见项目的同时，让执行策略可审计、可关闭、可在 CI 中稳定复现。

## 范围

| TODO | 设计归属 | 说明 |
| --- | --- | --- |
| 8.1 | `cli` | 新增 `orix run <script>` 命令 |
| 8.2 | `cli` + `core` | 执行 `start`、`dev`、`build` 等用户脚本 |
| 8.3 | `manifest` | 解析并保留 `package.json#scripts` |
| 8.4 | `core` 或后续 `lifecycle` crate | 脚本执行器、环境变量、PATH、进程管理 |
| 8.5 | `core` + `config` | `--ignore-scripts` 在安装管道生效 |
| 8.6 | `core` | lifecycle 执行时机 |
| 8.7 | `cli` + `workspace` | workspace 作用域脚本 |

本阶段不覆盖完整容器级沙箱、交互式 approve-builds 数据库、并行递归 workspace 脚本调度和 npm 全量 lifecycle 兼容矩阵。

## 命令设计

```bash
orix run build
orix run dev -- --host 0.0.0.0
orix run --dir packages/ui test
orix run --workspace @scope/ui build
orix run --recursive build
orix run --if-present lint
```

```rust
#[derive(Args)]
pub struct RunArgs {
    pub script: String,

    #[arg(trailing_var_arg = true)]
    pub args: Vec<String>,

    #[arg(long)]
    pub if_present: bool,

    #[arg(long)]
    pub workspace: Option<String>,

    #[arg(long, short = 'r')]
    pub recursive: bool,

    #[arg(long, default_value = "4")]
    pub concurrency: usize,
}
```

行为：

1. 从 `--dir` 或当前目录发现最近的 `package.json`。
2. 如果指定 `--workspace`，通过 workspace 索引定位目标包。
3. 找到 `scripts[script]`。
4. 自动按 npm 约定执行 `pre<script>`、`<script>`、`post<script>`。
5. 将 `--` 后参数追加到主脚本命令，不追加到 pre/post。
6. 子进程退出码原样透传；被信号终止时返回明确错误。

`--if-present` 只影响脚本不存在的情况，不吞掉脚本运行失败。

## Manifest 模型

`manifest` 已经解析 `scripts`，Phase 8 需要补齐验证和公共访问 API：

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
}

impl Manifest {
    pub fn script(&self, name: &str) -> Option<&str> {
        self.scripts.get(name).map(String::as_str)
    }

    pub fn lifecycle_chain(&self, name: &str) -> Vec<ScriptRef<'_>> {
        let mut scripts = Vec::new();
        for candidate in [format!("pre{name}"), name.to_string(), format!("post{name}")] {
            if let Some(command) = self.script(&candidate) {
                scripts.push(ScriptRef { name: candidate, command });
            }
        }
        scripts
    }
}
```

验证规则：

- script key 必须是非空字符串。
- script command 必须是字符串；非字符串视为 manifest 错误。
- 不解析 shell 语法，不做字符串重写。

## 脚本执行器

建议先在 `core` 中实现 `script` 模块，等边界稳定后再拆为 `crates/lifecycle`。这样不会提前引入新的 crate 依赖边。

```rust
pub struct ScriptRunner {
    config: Config,
}

pub struct ScriptContext<'a> {
    pub project_root: &'a Path,
    pub manifest: &'a Manifest,
    pub workspace: Option<&'a Workspace>,
    pub kind: ScriptKind,
}

pub enum ScriptKind {
    UserRun { name: String, args: Vec<String> },
    Lifecycle { event: LifecycleEvent, package: PackageId },
}

pub struct ScriptOutput {
    pub name: String,
    pub status: ExitStatus,
    pub duration: Duration,
}
```

### Shell 选择

为了兼容 npm scripts，脚本通过平台 shell 执行：

| 平台 | 命令 |
| --- | --- |
| Unix | `sh -c "<script>"` |
| Windows | `cmd.exe /D /S /C "<script>"` |

后续可支持 `.npmrc` 的 `script-shell` 配置。

### 环境变量

执行器设置最小 npm 兼容环境：

```txt
INIT_CWD=<用户启动 orix 的目录>
npm_lifecycle_event=<script name>
npm_package_name=<manifest.name>
npm_package_version=<manifest.version>
npm_config_user_agent=orix/<version>
ORIX=1
```

PATH 规则：

1. 当前 package 的 `node_modules/.bin`
2. workspace root 的 `node_modules/.bin`
3. 原始系统 PATH

PATH 前缀必须使用平台分隔符，且只加入存在的目录。

## 安装 lifecycle

Phase 8 先支持 project lifecycle，dependency lifecycle 默认受安全策略限制。

推荐顺序：

```txt
read config
read manifest/workspace
run project preinstall
resolve
fetch
store
write lockfile
link
run dependency lifecycle (受策略控制)
run project install
run project postinstall
run project prepare
validate layout
done
```

如果 `--ignore-scripts` 为 true：

- 跳过全部 project lifecycle。
- 跳过全部 dependency lifecycle。
- reporter 将 Scripts 阶段标为 skipped。

### Dependency lifecycle 策略

默认策略建议：

```txt
project scripts: enabled
dependency scripts: disabled unless allow-scripts includes package
CI: disabled unless ORIX_ENABLE_SCRIPTS=true
```

对应配置：

```ini
ignore-scripts=false
allow-scripts[]=esbuild
allow-scripts[]=@swc/core
```

`allow-scripts` 匹配 package name，不匹配任意 shell 片段。

## Workspace 作用域

workspace 发现由 `crates/workspace` 提供索引：

```rust
pub struct WorkspaceIndex {
    pub root: PathBuf,
    pub packages_by_name: BTreeMap<PackageName, WorkspacePackage>,
    pub packages_by_path: BTreeMap<PathBuf, WorkspacePackage>,
}
```

`orix run --recursive <script>`：

1. 读取所有 workspace package。
2. 根据 workspace package 之间的依赖关系拓扑排序。
3. 只执行声明了该脚本的 package；未声明的 package 视为 skipped。
4. 任一 package 失败则停止后续依赖该 package 的任务。

Phase 8 可以先串行执行，保留 `--concurrency` 参数作为后续增强。

## 错误模型

```rust
#[derive(thiserror::Error, Debug)]
pub enum ScriptError {
    #[error("script `{0}` not found in {1}")]
    MissingScript(String, PathBuf),

    #[error("script `{name}` failed with exit code {code:?}")]
    Failed { name: String, code: Option<i32> },

    #[error("script `{name}` was terminated by signal")]
    Terminated { name: String },

    #[error("script execution is disabled by --ignore-scripts")]
    Disabled,

    #[error("failed to spawn script `{name}`: {source}")]
    Spawn { name: String, source: std::io::Error },
}
```

`cli` 将 `MissingScript` 渲染为：

```txt
error: script "build" not found
hint: available scripts: dev, test
```

## 安全策略

- 不在日志中输出 auth token、完整 `.npmrc` 内容或带 token 的 registry URL。
- dependency lifecycle 默认不自动执行未知包。
- `--ignore-scripts` 必须在所有路径上生效，包括 `install`、`add`、`remove` 后触发的 install。
- 子进程继承 stdio，避免把大量输出缓存进内存。
- Ctrl-C 时转发终止信号并等待子进程退出。

Phase 8 的“沙箱”定义为受控环境和 allow-list，不承诺阻止恶意脚本访问本机文件系统。

## 测试计划

| 场景 | 类型 |
| --- | --- |
| `orix run build` 正常执行 | CLI integration |
| `prebuild/build/postbuild` 顺序 | unit + integration |
| `-- --flag` 参数只追加到主脚本 | integration |
| 缺失脚本报错，`--if-present` 成功 | integration |
| `node_modules/.bin` 出现在 PATH 前缀 | integration |
| `--ignore-scripts` 跳过 install lifecycle | core integration |
| workspace `--workspace` 定位子包 | workspace integration |
| workspace recursive 拓扑顺序 | unit |
| Windows `cmd.exe` 路径和 `.cmd` bin | Windows CI |

## 实施顺序

1. `manifest` 补 scripts API 和验证测试。
2. `cli` 增加 `run` 命令和参数透传。
3. `core` 实现 `ScriptRunner`，先支持用户脚本。
4. `core::install/add/remove` 串接 `--ignore-scripts` 和 project lifecycle。
5. `workspace` 提供按名称定位 package 的 API。
6. 增加 workspace run 和递归执行。
7. 引入 dependency lifecycle allow-list。
