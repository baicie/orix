# Manifest、Domain 与 Utils 设计

## 概述

本文档补齐 `manifest`、`domain`、`utils` 与 `macros` 的边界设计。这几个 crate 不直接执行安装管道，但它们定义了其他 crate 共享的语言、输入模型和小型通用能力。

设计目标：

- `manifest` 只负责 `package.json` 的解析、序列化和轻量验证。
- `domain` 只放跨 crate 共享的领域类型，不能依赖业务 crate。
- `utils` 只放无领域状态的通用函数，不能成为“杂物箱”。
- `macros` 保持预留，除非能明显降低重复样板，否则不新增过程宏。

## Crate 边界

```txt
manifest -> domain
utils    -> none
domain   -> none
macros   -> none
```

禁止从这些底层 crate 反向依赖 `core`、`resolver`、`lockfile`、`store` 或 `linker`。如果某个类型只被单个业务 crate 使用，应留在该业务 crate 内部。

## Manifest

`crates/manifest` 是 npm manifest 的输入适配层。它读写用户的 `package.json`，保留 npm 常见字段，并提供面向 resolver/linker 的访问方法。

### 支持字段

MVP 需要稳定支持：

| 字段 | 用途 |
| --- | --- |
| `name` | workspace 包识别、bin shorthand 名称、lockfile importer 展示 |
| `version` | workspace 本地包解析、发布元数据 |
| `dependencies` | 生产依赖解析 |
| `devDependencies` | 开发依赖解析 |
| `optionalDependencies` | 可选依赖解析和平台不匹配降级 |
| `peerDependencies` | MVP 记录并警告，不参与完整 peer 上下文求解 |
| `scripts` | 生命周期脚本预留，MVP 默认跳过 |
| `engines` | 记录 Node 约束，MVP 只警告 |
| `os` / `cpu` | 平台过滤输入 |
| `bin` | 未来生成 `.bin` 链接 |
| `files` / `private` / `type` | 保留生态兼容信息 |

### API 设计

```rust
pub struct Manifest {
    pub name: Option<String>,
    pub version: Option<String>,
    pub dependencies: BTreeMap<String, String>,
    pub dev_dependencies: BTreeMap<String, String>,
    pub peer_dependencies: BTreeMap<String, String>,
    pub optional_dependencies: BTreeMap<String, String>,
    pub scripts: BTreeMap<String, String>,
    pub engines: Option<Engines>,
    pub os: Vec<String>,
    pub cpu: Vec<String>,
    pub bin: BinField,
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Self>;
    pub fn write(&self, path: &Path) -> Result<()>;
    pub fn has_dependencies(&self) -> bool;
    pub fn name_as_pkg_name(&self) -> Option<PackageName>;
    pub fn version_as_version(&self) -> Option<Version>;
    pub fn resolve_bin(&self, cmd_name: &str, package_root: &Path) -> Option<PathBuf>;
}
```

### 验证策略

`manifest` 做轻量、局部、确定性的验证：

1. JSON 必须可解析。
2. 依赖字段必须是对象，键和值都按字符串处理。
3. `version` 如果存在，必须是合法 semver。
4. `bin` 支持 npm 的 string shorthand 和 object map 两种形式。
5. 未识别字段默认忽略，不阻塞安装。

跨 registry 的判断、版本是否存在、平台是否匹配、workspace 是否可解析，不放在 `manifest`。这些分别属于 `resolver`、`domain` 和 `workspace`。

## Domain

`crates/domain` 是共享领域语言。它必须稳定、可序列化，并避免携带具体 I/O 行为。

### 核心类型

```rust
pub struct PackageName(Cow<'static, str>);
pub struct Version(semver::Version);
pub struct VersionConstraint {
    pub raw: String,
    pub kind: ConstraintKind,
}

pub enum ConstraintKind {
    Exact(Version),
    Range(semver::VersionReq),
    Latest,
    Tag(String),
}

pub struct PackageId {
    pub name: PackageName,
    pub version: Version,
}

pub struct ResolvedPackage {
    pub id: PackageId,
    pub integrity: String,
    pub tarball: String,
    pub dependencies: Vec<(PackageName, String)>,
    pub optional_dependencies: Vec<(PackageName, String)>,
    pub peer_dependencies: Vec<(PackageName, String)>,
    pub engines: Option<String>,
    pub os: Vec<String>,
    pub cpu: Vec<String>,
    pub depnodes: Vec<String>,
}
```

### 包名规范化

包名规范化属于领域层语义，`utils::normalize_name` 不能继续承担 npm 包名规则。最终策略：

- 普通包名小写：`React` → `react`。
- scoped 包保留 `/` 结构并分别规范化：`@Scope/Package` → `@scope/package`。
- 拒绝空字符串、包含反斜杠、包含路径穿越片段的包名。
- 错误类型应可区分 `EmptyName`、`InvalidScope`、`InvalidCharacter`。

```rust
impl PackageName {
    pub fn parse(input: &str) -> Result<Self, PackageNameError>;
    pub fn scope(&self) -> Option<&str>;
    pub fn unscoped(&self) -> &str;
}
```

### Integrity 字符串

TODO 11.4 的 integrity parser 应放在 `domain`，因为 registry、fetcher、lockfile 都需要同一种解释。

支持格式：

```txt
sha512-<base64>
sha1-<base64>
sha512-<base64> sha1-<base64>
```

设计：

```rust
pub struct Integrity {
    pub algorithms: Vec<IntegrityDigest>,
}

pub struct IntegrityDigest {
    pub algorithm: IntegrityAlgorithm,
    pub digest_base64: String,
}

pub enum IntegrityAlgorithm {
    Sha512,
    Sha1,
}

impl Integrity {
    pub fn parse(input: &str) -> Result<Self, IntegrityError>;
    pub fn strongest(&self) -> Option<&IntegrityDigest>;
}
```

校验策略：

- 优先使用 `sha512`。
- 只在 registry 缺少 `sha512` 时回退到 `sha1`。
- 多个 digest 存在时，至少最强 digest 必须匹配；如实现成本可接受，也可以验证全部 digest。
- 比较 digest 必须使用常数时间比较。

### Tarball URL Builder

TODO 11.5 的 tarball URL builder 需要谨慎放置。`domain` 可以提供纯字符串/URL 规则，但不能做 HTTP。

```rust
pub fn package_metadata_url(registry: &Url, name: &PackageName) -> Result<Url>;
pub fn default_tarball_url(registry: &Url, id: &PackageId) -> Result<Url>;
```

规则：

- scoped 包请求 packument 时必须 URL encode `/` 为 `%2f` 或按 registry 兼容规则构造。
- registry base URL 必须以 `/` 结尾后再 join，避免吞掉路径前缀。
- 如果 packument 已提供 `dist.tarball`，以 registry 元数据为准，不使用默认 builder 猜测。

### 平台匹配

`domain` 提供平台匹配函数，但不决定安装是否失败：

```rust
pub fn check_platform_compatibility(os: &[String], cpu: &[String]) -> Option<PlatformMismatch>;
pub fn current_os() -> String;
pub fn current_cpu() -> String;
```

resolver 根据依赖类型决定处理方式：

- 普通依赖平台不匹配：MVP 警告并跳过，后续可改为严格失败。
- optional 依赖平台不匹配：静默跳过或低级别日志。
- lockfile 应保留 `os` / `cpu`，保证跨平台安装可解释。

## Utils

`crates/utils` 只放不引入领域概念的通用能力。凡是包含 npm 包名、版本、lockfile、registry 等语义的函数，都不属于 `utils`。

### 路径工具函数

TODO 10.2 建议提供：

```rust
pub fn normalize_path_for_lockfile(path: &Path) -> String;
pub fn relative_path(from_dir: &Path, to_path: &Path) -> Result<PathBuf>;
pub fn ensure_parent_dir(path: &Path) -> io::Result<()>;
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()>;
```

约束：

- lockfile 中路径统一使用 `/`，即使在 Windows 上生成。
- `relative_path` 不访问文件系统，只做路径组件计算。
- `atomic_write` 写入同目录临时文件后 rename，保证目标文件不会半写入。
- 不在 `utils` 中吞掉错误；底层返回 `io::Error`，由调用方映射成 crate 自己的错误类型。

### 当前 `normalize_name`

当前 `utils::normalize_name` 是占位函数，不符合 npm 包名规范化需求。后续实现 10.1 时应迁移到 `domain::PackageName::parse`，或将 `utils::normalize_name` 改名为仅用于展示字符串的函数，避免误用。

## Macros

`crates/macros` 目前只保留 no-op attribute。MVP 不依赖过程宏。

保留原则：

- 只有当多个 crate 出现重复、机械、易错的结构化样板时才新增宏。
- 过程宏不得隐藏 I/O、网络、文件系统副作用。
- 宏展开结果必须能通过文档或测试清晰说明。

可能的未来用途：

- 为错误类型生成统一 hint 映射。
- 为 CLI report 类型生成表格输出。

在这些需求出现前，`macros` 不应进入关键路径。
