# Resolver 设计 — 依赖图构建

## 概述

`crates/resolver` 将项目的声明依赖（来自 `package.json`）转换为完全解析的依赖图。它向 npm registry 查询可用版本，应用 semver 范围匹配，递归解析传递依赖，并生成 fetcher 和 linker 可消费的完整图。

## 问题描述

给定：

```
package.json:
  dependencies:
    react: "^18.2.0"
    react-dom: "^18.2.0"

  devDependencies:
    vite: "^5.0.0"
```

解析为精确版本及完整传递闭包：

```
react@18.2.0
├── js-tokens@4.0.0
│   └── ...
└── loose-envify@1.4.0
    └── ...

react-dom@18.2.0
├── react@18.2.0         ← 去重
├── loose-envify@1.4.0    ← 去重
└── scheduler@0.23.0
    └── ...

vite@5.0.0
├── rollup@4.0.0
│   └── ...
└── esbuild@0.19.0
    └── ...
```

## 核心数据结构

### 领域类型

```rust
/// 唯一标识一个包：name + version（MVP 中无 registry，无 peer 上下文）
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PackageId {
    pub name: PackageName,    // 例如 "react"，规范化为小写
    pub version: Version,     // 精确版本，例如 "18.2.0"
}

/// 来自 package.json 的版本约束："^1.0.0"、"">=2.0"、"latest" 等
#[derive(Clone, Debug)]
pub struct VersionConstraint {
    pub raw: String,
    pub kind: ConstraintKind,
}

pub enum ConstraintKind {
    Exact(Version),
    Range(semver::VersionRange),
    Latest,
    Tag(String),
}

/// 从字符串规范化的版本
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Version(semver::Version);

/// 从 registry 解析的包，包含安装所需的所有元数据
#[derive(Clone, Debug)]
pub struct ResolvedPackage {
    pub id: PackageId,
    pub integrity: String,           // sha512 integrity 字符串
    pub tarball_url: Url,
    pub dependencies: HashMap<PackageName, VersionConstraint>,
    pub dev_dependencies: HashMap<PackageName, VersionConstraint>,
    pub optional_dependencies: HashMap<PackageName, VersionConstraint>,
    pub peer_dependencies: HashMap<PackageName, VersionConstraint>,
    pub engines: Option<String>,
    pub os: Vec<String>,
    pub cpu: Vec<String>,
}

/// 一个 importer 的完整解析依赖图
#[derive(Clone, Debug)]
pub struct DependencyGraph {
    /// 所有已解析的包（传递闭包）
    pub packages: BTreeMap<PackageId, ResolvedPackage>,
    /// 此 importer 的 package.json 中声明的直接依赖
    pub direct_deps: HashSet<PackageId>,
}
```

### Registry API 类型

```rust
/// npm registry packument（简化版 —— 只包含我们实际使用的字段）
/// GET https://registry.npmjs.org/<package>
#[derive(Deserialize, Debug)]
pub struct Packument {
    pub name: String,
    pub versions: HashMap<String, PackageMetadata>,
    #[serde(default)]
    pub dist_tags: HashMap<String, String>,
}

/// registry 中单个版本的元数据
#[derive(Deserialize, Debug)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub dependencies: Option<HashMap<String, String>>,
    pub dev_dependencies: Option<HashMap<String, String>>,
    pub optional_dependencies: Option<HashMap<String, String>>,
    pub peer_dependencies: Option<HashMap<String, String>>,
    pub engines: Option<Engines>,
    pub os: Option<Vec<String>>,
    pub cpu: Option<Vec<String>>,
    pub dist: Dist,
    pub optional: Option<bool>,
}

#[derive(Deserialize, Debug)]
pub struct Dist {
    pub tarball: String,
    pub integrity: Option<String>,
    pub shasum: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Engines {
    pub node: Option<String>,
}
```

## 解析算法

### 顶层策略

resolver 使用**带记忆化的深度优先解析**：

```
resolve(pkg_name, constraint):
  1. 从 registry 获取 packument
  2. 选择满足约束的最佳匹配版本
  3. 如果已解析过（记忆化）→ 返回缓存
  4. 对已解析包的每个依赖：
     → 递归 resolve(dep_name, dep_constraint)
  5. 存入记忆化，返回
```

### 版本选择

```rust
fn select_version(packument: &Packument, constraint: &VersionConstraint) -> Result<Version> {
    match &constraint.kind {
        ConstraintKind::Exact(v) => Ok(v.clone()),
        ConstraintKind::Range(range) => {
            let mut candidates: Vec<_> = packument.versions.keys()
                .filter(|v| {
                    let parsed = Version::parse(v).ok();
                    parsed.as_ref().map_or(false, |ver| range.contains(ver))
                })
                .collect();
            candidates.sort_by(|a, b| Version::parse(b).unwrap().cmp(&Version::parse(a).unwrap()));
            candidates.first()
                .and_then(|v| Version::parse(v).ok())
                .with_context(|| format!("no version satisfies {}", constraint.raw))
        }
        ConstraintKind::Latest => {
            Ok(packument.dist_tags.get("latest")
                .and_then(|v| Version::parse(v).ok())
                .with_context(|| "no dist-tags.latest found")?)
        }
        ConstraintKind::Tag(tag) => {
            Ok(packument.dist_tags.get(tag)
                .and_then(|v| Version::parse(v).ok())
                .with_context(|| format!("tag '{}' not found", tag))?)
        }
    }
}
```

### 平台过滤

某些包在其元数据中有 `os` 或 `cpu` 限制。如果当前平台不匹配，这些包会被跳过（视为可选）。MVP 会记录警告，但不会因平台不匹配而硬性失败。

### Peer Dependencies（MVP 范围）

MVP 不实现完整的 pnpm peer 解析算法。取而代之的是：

- Peer dependencies 被视为**可选的**——如果存在于图中则解析，但不强制要求
- lockfile 中的包键**不**包含 peer 上下文（第三阶段增强）
- 这意味着 MVP 中 `peerDependencies` 在安装时不被强制执行

未来（第三阶段+）：具有未满足 peer dependencies 的包会得到 `pnpmfile.cjs` 垫片或警告。

完整 peer 上下文、冲突诊断和 lockfile package key 升级见 [生态兼容设计](./ecosystem-compat.md)。

## 解析缓存

为避免重复的 registry 调用：

```rust
pub struct Resolver {
    registry_url: Url,
    http_client: Client,
    /// 缓存：packument 缓存（TTL：5 分钟）
    packument_cache: Cache<PackumentCacheKey, Packument>,
    /// 缓存：解析记忆化（已解析的 PackageId）
    resolution_memo: HashMap<(PackageName, ConstraintKind), PackageId>,
}

impl Resolver {
    /// 解析单个依赖约束，返回解析后的 PackageId
    pub async fn resolve(
        &mut self,
        name: &PackageName,
        constraint: &VersionConstraint,
    ) -> Result<PackageId>;

    /// 将整个 manifest 的依赖解析为完整图
    pub async fn resolve_manifest(&mut self, manifest: &Manifest) -> Result<DependencyGraph>;

    /// 使用本地工作区覆盖解析（workspace:* 协议）
    pub async fn resolve_with_workspace(
        &mut self,
        manifest: &Manifest,
        workspace: &Workspace,
    ) -> Result<DependencyGraph>;
}
```

## 错误处理

| 错误 | 原因 | 解决方式 |
|------|------|----------|
| `RegistryUnreachable` | 无网络/registry URL 错误 | 指数退避重试，然后报错 |
| `PackageNotFound` | 包在 registry 上不存在 | 失败并给出清晰消息 |
| `NoSatisfyingVersion` | semver 范围无匹配 | 建议可用版本 |
| `IntegrityUnavailable` | registry 返回无 integrity | 回退到 shasum（带警告） |

## 性能考量

- **并行获取**：所有 packument 获取通过信号量限制并发执行（例如 10 个并发）
- **依赖排序**：包在返回前按拓扑排序，使 fetcher 可以按最优顺序下载
- **记忆化**：同一包+约束在一次解析会话中绝不会两次访问 registry
