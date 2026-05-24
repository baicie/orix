# Lockfile Snapshots 设计与代码草案

## 背景

pnpm lockfile 将依赖信息分成三层：

```txt
importers  -> 每个 workspace/root 的直接依赖声明
packages   -> registry package 的物理包元信息
snapshots  -> 某个 package 在特定依赖上下文中的实际依赖快照
```

Orix 目前只有 `importers` 和 `packages`。这在 MVP 下可用，但存在两个问题：

1. `packages` 既保存包元信息，又保存依赖展开信息，后续 peerDependencies 完整算法会变得困难。
2. 同一个 `name@version` 如果处在不同 peer 上下文中，实际 node_modules 依赖边可能不同，只用 `/name@version` 无法表达。

因此需要引入 `snapshots`。

## 目标

- 让 `packages` 只描述物理包：resolution、integrity、name、version、平台限制。
- 让 `snapshots` 描述逻辑实例：dependencies、optionalDependencies、peerDependencies、peer context。
- 不兼容旧 `orix-lock.yaml`。当前仍处于开发阶段，旧 lockfile 由用户删除后重新生成。
- 为后续 pnpm peer 变体 key 做准备，例如：

```txt
/eslint-plugin-x@1.0.0(eslint@10.4.0)
```

## 非目标

- 本阶段不实现完整 peer 解析算法。
- 本阶段不要求完全匹配 pnpm v9 lockfile 的所有字段。
- 本阶段不改变 installer/linker 的 node_modules 布局算法，只改变 lockfile 表达。

## 新格式

```yaml
lockfileVersion: 4
saveRemoteCacheURLs: true

importers:
  .:
    dependencies:
      vite:
        specifier: ^8.0.0
        version: 8.0.10

packages:
  /vite@8.0.10:
    resolution:
      tarball: https://registry.npmjs.org/vite/-/vite-8.0.10.tgz
      integrity: sha512-...
    integrity: sha512-...
    name: vite
    version: 8.0.10
    engines: ^20.19.0 || >=22.12.0

snapshots:
  /vite@8.0.10:
    dependencies:
      esbuild: ^0.28.0
      fdir: ^6.5.0
      picomatch: ^4.0.3
      postcss: ^8.5.6
      rollup: ^4.60.0
    optionalDependencies:
      fsevents: ~2.3.3
    peerDependencies:
      '@types/node': ^20.19.0 || >=22.12.0
```

## 字段归属

| 字段 | importers | packages | snapshots |
| --- | --- | --- | --- |
| package.json specifier | yes | no | no |
| resolved version | yes | yes | key 中也有 |
| tarball URL | no | yes | no |
| integrity | no | yes | no |
| engines/os/cpu | no | yes | no |
| dependencies | no | no | yes |
| optionalDependencies | no | no | yes |
| peerDependencies | no | no | yes |
| peer context | no | no | yes |

## Snapshot Key

先使用无 peer 上下文的 key：

```txt
/<name>@<version>
```

后续 peer 完整算法落地后扩展为：

```txt
/<name>@<version>(<peerName>@<peerVersion>)(<peerName>@<peerVersion>)
```

要求：

- peer 名按字典序排序。
- key 必须稳定，不能依赖 HashMap 遍历顺序。
- 没有 peer context 时与 package key 相同。

## 数据类型代码

目标文件：`crates/lockfile/src/types.rs`

```rust
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const LOCKFILE_VERSION: i32 = 4;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(rename = "lockfileVersion")]
    pub version: i32,

    #[serde(rename = "saveRemoteCacheURLs", default)]
    pub save_remote_cache_urls: bool,

    pub importers: BTreeMap<String, ImporterLock>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub packages: BTreeMap<String, PackageLock>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub snapshots: BTreeMap<String, SnapshotLock>,

    #[serde(
        rename = "orixGraphHash",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub graph_hash: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotLock {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,

    #[serde(
        rename = "optionalDependencies",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub optional_dependencies: BTreeMap<String, String>,

    #[serde(
        rename = "peerDependencies",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub peer_dependencies: BTreeMap<String, String>,

    #[serde(
        rename = "peerContext",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub peer_context: BTreeMap<String, String>,
}
```

`PackageLock` 不再保存依赖边。依赖边只写入 `snapshots`：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLock {
    #[serde(rename = "id", default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    #[serde(rename = "local", default, skip_serializing_if = "Option::is_none")]
    pub local: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<PackageResolution>,

    #[serde(rename = "engines", default, skip_serializing_if = "Option::is_none")]
    pub engines: Option<String>,

    #[serde(rename = "os", default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Vec<String>>,

    #[serde(rename = "cpu", default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<Vec<String>>,
}
```

## 写入逻辑代码

目标文件：`crates/lockfile/src/ops.rs`

```rust
fn package_key(pkg: &orix_domain::ResolvedPackage) -> String {
    format!("/{}@{}", pkg.id.name, pkg.id.version)
}

fn snapshot_key(pkg: &orix_domain::ResolvedPackage) -> String {
    // P0: no peer context yet.
    package_key(pkg)
}

fn snapshot_from_package(pkg: &orix_domain::ResolvedPackage) -> SnapshotLock {
    SnapshotLock {
        dependencies: pkg
            .dependencies
            .iter()
            .map(|(name, raw)| (name.to_string(), raw.clone()))
            .collect(),
        optional_dependencies: pkg
            .optional_dependencies
            .iter()
            .map(|(name, raw)| (name.to_string(), raw.clone()))
            .collect(),
        peer_dependencies: pkg
            .peer_dependencies
            .iter()
            .map(|(name, raw)| (name.to_string(), raw.clone()))
            .collect(),
        peer_context: BTreeMap::new(),
    }
}

fn package_lock_from_package(pkg: &orix_domain::ResolvedPackage) -> PackageLock {
    PackageLock {
        id: Some(format!("registry.npmjs.org/{}/{}", pkg.id.name, pkg.id.version)),
        local: None,
        integrity: Some(pkg.integrity.clone()),
        name: Some(pkg.id.name.to_string()),
        version: Some(pkg.id.version.to_string()),
        resolution: Some(PackageResolution {
            tarball: Some(pkg.tarball.clone()),
            integrity: Some(pkg.integrity.clone()),
            resolution_type: None,
            path: None,
        }),
        dependencies: BTreeMap::new(),
        optional_dependencies: BTreeMap::new(),
        peer_dependencies: BTreeMap::new(),
        engines: pkg.engines.clone(),
        os: non_empty_vec(pkg.os.clone()),
        cpu: non_empty_vec(pkg.cpu.clone()),
    }
}
```

在 `Lockfile::update` 中替换 package 写入：

```rust
for pkg in graph.packages() {
    lockfile
        .packages
        .insert(package_key(pkg), package_lock_from_package(pkg));

    lockfile
        .snapshots
        .insert(snapshot_key(pkg), snapshot_from_package(pkg));
}
```

## 读取代码

目标文件：`crates/lockfile/src/ops.rs`

```rust
impl Lockfile {
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let lockfile: Self = serde_yaml::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        if lockfile.version != LOCKFILE_VERSION {
            anyhow::bail!(
                "Lockfile version {} is not supported by this orix version (expected {}). Delete orix-lock.yaml and run orix install again.",
                lockfile.version,
                LOCKFILE_VERSION
            );
        }

        Ok(lockfile)
    }
}
```

## Graph 恢复代码

目标文件：`crates/lockfile/src/resolve.rs`

```rust
pub fn resolve_from_lockfile(lockfile: &Lockfile) -> DependencyGraph {
    let mut graph = DependencyGraph::new();

    for (key, pkg) in &lockfile.packages {
        let tarball = match pkg.resolution.as_ref().and_then(|r| r.tarball.clone()) {
            Some(tarball) => tarball,
            None => continue,
        };

        let key_str = key.trim_start_matches('/');
        let (name_str, ver_str) = key_str.rsplit_once('@').unwrap_or((key_str, ""));
        let Ok(version) = Version::parse(ver_str) else {
            continue;
        };

        let Some(snapshot) = lockfile.snapshots.get(key) else {
            continue;
        };

        let deps = snapshot
            .dependencies
            .iter()
            .map(|(name, raw)| (PackageName::from(name.as_str()), raw.clone()))
            .collect();
        let opt_deps = snapshot
            .optional_dependencies
            .iter()
            .map(|(name, raw)| (PackageName::from(name.as_str()), raw.clone()))
            .collect();
        let peer_deps = snapshot
            .peer_dependencies
            .iter()
            .map(|(name, raw)| (PackageName::from(name.as_str()), raw.clone()))
            .collect();

        let depnodes = snapshot
            .dependencies
            .keys()
            .chain(snapshot.optional_dependencies.keys())
            .chain(snapshot.peer_dependencies.keys())
            .cloned()
            .collect();

        graph.insert(ResolvedPackage {
            id: PackageId::new(PackageName::from(name_str), version),
            integrity: pkg.integrity.clone().unwrap_or_default(),
            tarball,
            dependencies: deps,
            dev_dependencies: Vec::new(),
            optional_dependencies: opt_deps,
            peer_dependencies: peer_deps,
            engines: pkg.engines.clone(),
            os: pkg.os.clone().unwrap_or_default(),
            cpu: pkg.cpu.clone().unwrap_or_default(),
            depnodes,
            patch: None,
        });
    }

    graph
}
```

旧函数直接删除。core fast path 统一改成：

```rust
let graph = resolve_from_lockfile(old_lockfile);
```

## Pnpm 导入映射

目标文件：`crates/lockfile/src/pnpm.rs`

导入 pnpm v9 时：

```txt
pnpm.packages  -> orix.packages
pnpm.snapshots -> orix.snapshots
```

如果 pnpm snapshot key 不存在于 packages：

- 仍保留 snapshot。
- graph 恢复时只从 packages 生成物理包。
- peer 完整算法落地后再使用 peer snapshot key 生成多实例 graph。

## 迁移策略

不做旧版本兼容迁移。开发期策略是：

```txt
delete orix-lock.yaml
oi i
```

实现要求：

- `LOCKFILE_VERSION` 从 `3` 升到 `4`。
- 新增 `Lockfile.snapshots`。
- `read()` 遇到非 v4 直接报错。
- `write()` 只写 v4 格式。
- `packages` 不再包含 dependencies / optionalDependencies / peerDependencies。
- `snapshots` 是恢复 graph 的唯一依赖边来源。

验收：

- 旧 `orix-lock.yaml` 读取失败，并提示删除重装。
- `cargo test -p orix-lockfile` 通过。

### Phase 1：fast path 从 lockfile 恢复 graph

- 新增 `resolve_from_lockfile(&Lockfile)`。
- core fast path 改用 `resolve_from_lockfile(old_lockfile)`。
- 删除 `resolve_from_lockfile_packages` 或仅在测试中替换掉。

验收：

- v4 lockfile 能 fast path。
- snapshots 缺失时跳过该 package 或返回结构错误，不能 fallback 到 packages。

### Phase 2：pnpm snapshots 导入

- pnpm import 保留 `snapshots` section。
- 导入后不丢 peer context key。

验收：

- Bonree `pnpm-lock.yaml` 导入后 snapshot 数接近 pnpm。
- `orix-lock.yaml` 文件大小不因 importer 重复字段膨胀。

### Phase 3：peer context 多实例

- resolver 生成 snapshot key 时带 peer context。
- linker 按 snapshot key 创建 virtual store 实例。
- package key 仍指向物理 tarball，snapshot key 指向逻辑实例。

验收：

- 同一 `name@version` 在不同 peer 下可以产生不同 snapshot。
- `.orix` layout 不会把两个 peer context 混成一个实例。

## 测试计划

### 单元测试

```rust
#[test]
fn update_writes_dependency_edges_to_snapshots() {
    // packages entry has empty dependencies
    // snapshots entry has dependencies
}

#[test]
fn read_rejects_old_lockfile_version() {
    // lockfileVersion: 3 -> error with delete/regenerate hint
}

#[test]
fn resolve_from_lockfile_requires_snapshot_edges() {
    // packages alone are not enough to restore dependencies
}
```

### 集成测试

- 使用一个包含 root + workspace 的 fixture。
- 首次安装生成 v4 lockfile。
- 删除 `node_modules` 后 fast path 能从 v4 snapshots 恢复 graph。
- 人工删除 snapshots 后 fast path 失败或跳过不完整 package，不能静默使用旧字段。

## 风险

### 文件大小可能上升

pnpm 有 snapshots 后仍然比简单 lockfile 大，但它避免了把 peer 上下文塞进 package key 的歧义。Orix 需要保证：

- importers 不重复 package 元信息。
- packages 不重复 snapshot 依赖边。
- 空字段不序列化。

### fast path hash 变化

引入 snapshots 后 graph hash 可能变化。需要让 `orixGraphHash` 基于恢复后的 `DependencyGraph`，而不是基于 YAML 文本。

## 推荐落地顺序

1. 新增 `SnapshotLock` 和 `Lockfile.snapshots`，`LOCKFILE_VERSION = 4`。
2. `read()` 拒绝非 v4 lockfile，提示删除重装。
3. `update()` 写 snapshots，packages 不再写依赖边。
4. 新增 `resolve_from_lockfile(&Lockfile)`，core fast path 改用它。
5. 删除 `resolve_from_lockfile_packages` 的生产调用。
6. pnpm import 映射 snapshots。
7. 后续 peer 完整算法使用 snapshot key。
