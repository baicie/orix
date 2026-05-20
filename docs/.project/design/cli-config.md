# CLI & Config 设计 — 用户界面

## 概述

`crates/cli` 是二进制入口点。它解析命令行参数，从 `.npmrc` 加载配置，并将任务委托给核心库。`crates/config` 处理所有配置源：默认值、环境变量、`.npmrc` 和 CLI 参数。

**相关文档**：[CLI 透传、隐式 run 与 postinstall](./cli-run-passthrough-lifecycle.md)（参数无 `--` 透传、`oi dev` 隐式 run、安装脚本缺口与修复顺序）。

## CLI 命令

### Install（安装）

```bash
# 从 lockfile 安装所有依赖（快速，冻结）
orix install

# 安装并更新 lockfile 以匹配 package.json
orix install

# 冻结 lockfile（CI/CD）
orix install --frozen-lockfile

# 优先使用离线缓存
orix install --offline

# 强制重新获取所有包
orix install --force

# 安装到特定目录（用于 workspace 中的子包）
orix install --dir packages/my-lib

# 使用自定义全局 store 和 tarball cache，避免默认落到用户目录 / C 盘
orix --store-dir D:/orix/store --cache-dir D:/orix/cache install
```

### Add（添加）

```bash
# 添加生产依赖
orix add react
orix add react@18
orix add "react@^18.2.0"
orix add react react-dom

# 作为 dev 依赖添加
orix add -D vite

# 作为可选依赖添加
orix add -O @emotion/css
```

### Remove（移除）

```bash
orix remove react
orix remove react react-dom
orix remove --dir packages/my-lib react
```

### Store（Store 管理）

```bash
# 清理未使用的包
orix store prune

# 显示 store 状态
orix store status

# store 目录路径
orix store path
```

### Run（脚本执行）

```bash
# 执行当前 package 的 scripts.build
orix run build

# 参数透传给主脚本（脚本名之后无需 `--`，与 pnpm 一致）
orix run dev --host 0.0.0.0
orix run build -w --config rollup.config.mjs

# 隐式 run：未匹配内置子命令时等同 orix run（见专项设计文档）
orix dev
orix dev --host 0.0.0.0

# 在 workspace 子包中执行
orix run --workspace @scope/ui build

# 递归执行所有 workspace package 中存在的脚本
orix run --recursive --if-present test
```

- orix 专属选项（`--recursive`、`--workspace`、`--if-present`）须写在**脚本名之前**；脚本名之后的 `-` 开头参数一律交给脚本。
- 仍支持 npm 习惯：`orix run dev -- --host`（会剥掉一层 `--`）。

详细执行模型、PATH、环境变量、安全策略和 workspace 拓扑顺序见 [Lifecycle Scripts](./lifecycle-scripts.md)。透传解析、隐式 `run` 与 `postinstall` 修复见 [CLI 透传、隐式 run 与 postinstall](./cli-run-passthrough-lifecycle.md)。

### 其他

```bash
orix --version
orix --help
orix import              # 从 package-lock.json 或 yarn.lock 导入
```

## CLI 参数

```rust
use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "orix")]
#[command(version, about = "Fast, disk-space efficient package manager")]
struct Cli {
    /// 全局：覆盖 registry URL
    #[arg(long, global = true, env = "RPNPM_REGISTRY")]
    registry: Option<String>,

    /// 全局：日志级别
    #[arg(long, global = true, default_value = "info", env = "RPNPM_LOG")]
    log: String,

    /// 工作目录（默认：当前目录）
    #[arg(long, short = 'C', default_value = ".", env = "RPNPM_DIR")]
    dir: PathBuf,

    /// 全局 store 目录（默认：~/.orix/store/v1）
    #[arg(long, global = true, env = "ORIX_STORE")]
    store_dir: Option<PathBuf>,

    /// tarball cache 目录（默认：系统 cache 目录）
    #[arg(long, global = true, env = "ORIX_CACHE")]
    cache_dir: Option<PathBuf>,

    /// 全局：输出使用颜色
    #[arg(long, global = true, default_value = "auto")]
    color: ColorChoice,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 从 lockfile 或 package.json 安装依赖
    Install(InstallArgs),

    /// 添加依赖
    Add(AddArgs),

    /// 移除依赖
    Remove(RemoveArgs),

    /// 管理包 store
    Store(StoreArgs),

    /// 执行 package.json scripts
    Run(RunArgs),
}

#[derive(Args)]
struct InstallArgs {
    /// 如果 lockfile 过期则失败
    #[arg(long)]
    frozen_lockfile: bool,

    /// 遇到网络错误时继续，使用缓存的包
    #[arg(long)]
    offline: bool,

    /// 强制重新获取所有包并重新生成 lockfile
    #[arg(long)]
    force: bool,

    /// 不运行安装脚本
    #[arg(long)]
    ignore_scripts: bool,

    /// 并发下载数
    #[arg(long, default_value = "10")]
    concurrency: usize,
}

#[derive(Args)]
struct AddArgs {
    /// 保存为 dev 依赖
    #[arg(short = 'D')]
    dev: bool,

    /// 保存为可选依赖
    #[arg(short = 'O')]
    optional: bool,

    /// 要添加的包
    #[arg(trailing_var_arg = true)]
    packages: Vec<String>,
}

#[derive(Args)]
struct RemoveArgs {
    /// 包的工作目录
    #[arg(long, short = 'C')]
    dir: Option<PathBuf>,

    /// 要移除的包
    #[arg(trailing_var_arg = true)]
    packages: Vec<String>,
}

#[derive(Args)]
struct RunArgs {
    /// 要执行的 script 名称
    script: String,

    /// 脚本不存在时成功退出
    #[arg(long)]
    if_present: bool,

    /// 在指定 workspace package 中执行
    #[arg(long)]
    workspace: Option<String>,

    /// 在所有 workspace package 中按拓扑顺序执行
    #[arg(long, short = 'r')]
    recursive: bool,

    /// 传给脚本的剩余参数（脚本名之后；支持 `-` 开头，见 cli-run-passthrough-lifecycle.md）
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Subcommand)]
enum StoreArgs {
    /// 从 store 中移除未引用的包
    Prune {
        #[arg(long)]
        dry_run: bool,
    },

    /// 打印 store 根路径
    Path,

    /// 检查 store 完整性
    Verify,
}
```

## 配置加载

配置从多个源加载，优先级如下（最高优先）：

1. CLI 参数
2. 环境变量（`RPNPM_*`）
3. 项目 `.npmrc`
4. 用户 `.npmrc`（`~/.npmrc`）
5. 全局 `.npmrc`（`/etc/npmrc`）
6. 默认值

### 配置源

#### `.npmrc` 格式

```ini
# ~/.npmrc 或 ./.npmrc
registry=https://registry.npmjs.org/
strict-ssl=true
fetch-retries=3
fetch-timeout=30000

# 认证（每个 registry）
#registry.npmjs.org/:_authToken=${NPM_TOKEN}

# 缓存设置
cache-dir=~/.npm
store-dir=~/.orix/store

# 链接设置
public-hoist-pattern[]=*_eslint-plugin_*
public-hoist-pattern[]=*_babel_*
side-effects-cache=true
```

### 配置结构体

```rust
/// 应用级配置
#[derive(Clone, Debug)]
pub struct Config {
    /// Registry 基础 URL
    pub registry: Url,

    /// 全局 store 目录
    pub store_dir: PathBuf,

    /// 本地 tarball 缓存目录
    pub cache_dir: PathBuf,

    /// HTTP 认证 token（可选）
    pub auth_token: Option<String>,

    /// 并发下载数
    pub concurrency: usize,

    /// HTTP 超时（毫秒）
    pub fetch_timeout: Duration,

    /// 获取重试次数
    pub fetch_retries: u32,

    /// 运行生命周期脚本
    pub ignore_scripts: bool,

    /// Hoist 模式（未来：public-hoist-pattern）
    pub hoist_patterns: Vec<String>,

    /// 彩色输出
    pub color: ColorChoice,
}

impl Config {
    pub fn load(project_root: &Path) -> Result<Self> {
        // 1. 从默认值开始
        let mut config = Config::default();

        // 2. 加载全局 .npmrc
        if let Some(home) = dirs::home_dir() {
            let global_rc = home.join(".npmrc");
            if global_rc.exists() {
                config.merge_file(&global_rc)?;
            }
        }

        // 3. 加载项目 .npmrc（覆盖全局）
        let project_rc = project_root.join(".npmrc");
        if project_rc.exists() {
            config.merge_file(&project_rc)?;
        }

        // 4. 加载环境变量（覆盖 .npmrc）
        config.merge_env()?;

        Ok(config)
    }

    fn merge_file(&mut self, path: &Path) -> Result<()> {
        let source = std::fs::read_to_string(path)?;
        for line in source.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                self.set(key.trim(), value.trim());
            }
        }
        Ok(())
    }

    fn merge_env(&mut self) {
        if let Ok(v) = env::var("RPNPM_REGISTRY") {
            self.registry = Url::parse(&v).unwrap_or(self.registry.clone());
        }
        if let Ok(v) = env::var("RPNPM_STORE") {
            self.store_dir = PathBuf::from(v);
        }
        // ...
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            registry: Url::parse("https://registry.npmjs.org/").unwrap(),
            store_dir: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".orix/store/v1"),
            cache_dir: dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("orix/tarballs"),
            auth_token: None,
            concurrency: 10,
            fetch_timeout: Duration::from_secs(30),
            fetch_retries: 3,
            ignore_scripts: false,
            hoist_patterns: Vec::new(),
            color: ColorChoice::Auto,
        }
    }
}
```

## 输出与用户体验

### 进度输出

CLI 在安装期间显示结构化进度：

```
orix install
│
├─resolve       2/2    ████████████████████ done
├─fetch         3/3    ████████████████████ done
├─link          8/8    ████████████████████ done
└─done          12 packages installed in 1.23s
```

### 错误输出

错误是人类可读的，带有上下文：

```
error: Package not found: "nonexistent-pkg-xyz"
  at registry: https://registry.npmjs.org/

Hint: Check the package name for typos.
      If the package is private, add it to your .npmrc with authentication.
```

而不是原始的 Rust 错误链。

## 架构：CLI ↔ Core 边界

```
cli (binary)
  └─► config::Config        # 组装后的配置
  └─► core::install()       # 主入口
  └─► core::add()           # 添加命令
  └─► core::remove()        # 移除命令
  └─► core::store_*()       # store 管理
```

CLI crate 零业务逻辑。它是一个纯适配层。
