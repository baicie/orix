# Lockfile 设计 — 可重现的安装状态

## 概述

`crates/lockfile` 管理 `orix-lock.yaml` lockfile——一个人类可读的、版本控制的完整依赖图快照。lockfile 确保在任何机器上（或 CI 中）运行 `orix install` 都能产生逐字节完全相同的 `node_modules` 布局。

## 文件格式

lockfile 使用 YAML 以提高人类可读性和版本控制友好性。

```yaml
# orix-lock.yaml
lockfileVersion: 1
saveRemoteCacheURLs: true

importers:
  .:
    dependencies:
      react: &ref001
        specifier: ^18.2.0
        version: 18.2.0
    devDependencies:
      vite:
        specifier: ^5.0.0
        version: 5.0.0
  packages/my-lib:
    dependencies:
      react:
        specifier: ^18.2.0
        version: 18.2.0
    specifier: file:../my-lib

packages:
  /react@18.2.0:
    id: registry.npmjs.org/react/18.2.0/react-18.2.0.tgz
    integrity: sha512-7neZq8Z+5I6eKf2N5I4cTvZd3+Pw4H5I5YjNhEYPp3a6nIwAefqHf4O4a8ai切H8QAhZqUfmDjB4+4nH5H3T3P+5H5Q==
    name: react
    version: 18.2.0
    resolution:
      tarball: https://registry.npmjs.org/react/-/react-18.2.0.tgz
      integrity: sha512-7neZq8Z+5I6eKf2N5I4cTvZd3+Pw4H5I5YjNhEYPp3a6nIwAefqHf4O4a8ai切H8QAhZqUfmDjB4+4nH5H3T3P+5H5Q==
    dependencies:
      loose-envify: ^1.1.0
      scheduler: ^0.23.0
    optionalDependencies: {}

  /loose-envify@1.4.0:
    id: registry.npmjs.org/loose-envify/1.4.0/loose-envify-1.4.0.tgz
    integrity: sha512-lyuxPGr/SfR2aqEKH7McL9
    name: loose-envify
    version: 1.4.0
    resolution:
      tarball: https://registry.npmjs.org/loose-envify/-/loose-envify-1.4.0.tgz
      integrity: sha512-lyuxPGr/SfR2aqEKH7McL9
    dependencies:
      js-tokens: ^4.0.0

  /vite@5.0.0:
    id: registry.npmjs.org/vite/5.0.0/vite-5.0.0.tgz
    integrity: sha512-hJsQJpTG0Rzkv/W5U
    name: vite
    version: 5.0.0
    resolution:
      tarball: https://registry.npmjs.org/vite/-/vite-5.0.0.tgz
      integrity: sha512-hJsQJpTG0Rzkv/W5U
    dependencies:
      rollup: ^4.0.0

snapshots:
  /node_modules/.pnpm-store.yaml:
    content: sha512:abc...
```

## 关键设计决策

### `lockfileVersion: 1`

版本表示 lockfile 格式的修订。本文档涵盖版本 1。当格式变更时（例如添加 peer 解析），递增版本并实现迁移路径。

### 包键：`/<name>@<version>`

使用 `/name@version` 作为 YAML 键（带前导斜杠）可避免包名以数字或特殊字符开头时的 YAML 解析问题。示例：`/123-array@1.0.0`。

### 分离 `importers` 和 `packages`

两个 section 防止大型 monorepo 中的合并冲突：

- **`importers`**：每个项目的直接依赖——每个项目各自变更，合并友好
- **`packages`**：共享的已解析包注册表——变更频率较低，在 importers 间去重

### 保留 `specifier`

原始的 `package.json` 约束（`^18.2.0`）与解析后的版本（`18.2.0`）一起保留。这使得：

- 检测 lockfile 何时过期 vs `package.json` 何时变更
- 为关心的工具保留用户意图（精确 vs 范围）

## 核心 API

```rust
pub struct Lockfile {
    pub version: LockfileVersion,
    pub importers: BTreeMap<ImporterId, ImporterLock>,
    pub packages: BTreeMap<PackageKey, PackageLock>,
    pub snapshots: HashMap<PathBuf, String>,
}

pub struct ImporterLock {
    pub specifiers: HashMap<PackageName, String>,
    pub dependencies: HashMap<PackageName, ResolvedDep>,
    pub dev_dependencies: HashMap<PackageName, ResolvedDep>,
    pub optional_dependencies: HashMap<PackageName, ResolvedDep>,
}

pub struct PackageLock {
    pub id: String,
    pub integrity: String,
    pub name: PackageName,
    pub version: Version,
    pub resolution: PackageResolution,
    pub dependencies: HashMap<PackageName, Version>,
    pub optional_dependencies: HashMap<PackageName, Version>,
    pub engines: Option<String>,
    pub os: Option<Vec<String>>,
    pub cpu: Option<Vec<String>>,
}

pub struct PackageResolution {
    pub tarball: Url,
    pub integrity: String,
}
```

## 操作

### `Lockfile::read(path: &Path) -> Result<Lockfile>`

使用 `serde_yaml` 解析 `orix-lock.yaml`。如果文件缺失（首次安装）或格式错误则返回错误。

### `Lockfile::write(path: &Path, lockfile: &Lockfile) -> Result<()>`

序列化为 YAML 并原子写入（写入临时文件 → 重命名）以防止崩溃时损坏。

### `Lockfile::update(manifest: &Manifest, graph: &DependencyGraph) -> Lockfile`

将新解析合并到 lockfile：

1. 更新 importer 的 `specifiers` 和 `dependencies` section
2. 为任何新增/变更的包在 `packages` 中添加或更新条目
3. 保留不再使用的包条目（便于 diff 清晰）

### `Lockfile::diff(old: &Lockfile, new: &Lockfile) -> LockfileDiff`

计算两个 lockfile 版本之间的变更。用于：

- 向用户显示安装前的变更内容
- 选择性安装（只获取变更的包）

```rust
pub struct LockfileDiff {
    pub added: Vec<PackageId>,
    pub removed: Vec<PackageId>,
    pub changed: Vec<(PackageId, PackageId)>,  // (旧, 新)
    pub importers_changed: Vec<ImporterId>,
}
```

## `--frozen-lockfile` 验证

当用户传入 `--frozen-lockfile`（或 `RPNPM_FROZEN_LOCKFILE=true`）时：

```rust
pub fn validate_frozen(lockfile: &Lockfile, manifest: &Manifest) -> Result<()> {
    let current = Lockfile::read(lockfile_path)?;
    let expected = current.clone();

    // 检查 package.json specifiers 与 lockfile 匹配
    for (name, constraint) in manifest.dependencies.iter() {
        let resolved = &current.importers["."].dependencies[name];
        if !satisfies_exact(&resolved.version, constraint) {
            anyhow::bail!(
                "package.json dependency '{}' ({}) does not match lockfile ({}). \
                 Run orix install without --frozen-lockfile.",
                name, constraint, resolved.version
            );
        }
    }

    // 检查 lockfile 对所有声明的依赖都有条目
    for name in manifest.dependencies.keys() {
        if !current.importers["."].dependencies.contains_key(name) {
            anyhow::bail!(
                "package '{}' is in package.json but not in lockfile. \
                 Run orix install without --frozen-lockfile.",
                name
            );
        }
    }

    Ok(())
}
```

## 与 pnpm-lock.yaml 的兼容性（未来）

第三阶段+ 可能添加转换器：

```
orix import-pnpm-lock    # 读取 pnpm-lock.yaml → orix-lock.yaml
orix export-pnpm-lock   # 写入 pnpm-lock.yaml 以兼容 pnpm
```

这需要实现完整的 pnpm lockfile schema，包括 peer 解析键。

## 文件位置

- 单工作区根目录：`./orix-lock.yaml`
- 项目内：与声明依赖的 `package.json` 同级的 `./orix-lock.yaml`
