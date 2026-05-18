# 生态兼容设计 — Phase 9

## 概述

Phase 9 的目标是让 orix 从“能安装 MVP 项目”推进到“能处理主流 npm/pnpm 生态项目”。核心工作包括：

- 完整 peerDependencies 解析与冲突诊断。
- pnpm-lock.yaml 读取和导出。
- `patch:` 协议。
- workspace catalogs。
- `orix deploy`。

这些能力跨越 `resolver`、`lockfile`、`workspace`、`fetcher`、`linker`、`core` 和 `cli`。设计上保持现有 crate 依赖方向不变：复杂编排放在 `core`，格式转换放在 `lockfile`，协议解析放在 `resolver`/`workspace`，文件操作放在 `fetcher`/`store`/`linker`。

## 范围与优先级

| TODO | 优先级 | Crate | 说明 |
| --- | --- | --- | --- |
| 9.1 | P0 | `resolver` + `domain` | peer 上下文参与 package key |
| 9.2 | P0 | `resolver` + `cli` | peer 缺失、冲突、可选 peer 诊断 |
| 9.3 | P1 | `lockfile` | 读取 pnpm-lock.yaml 并转为 orix lockfile |
| 9.4 | P2 | `lockfile` | 从 orix lockfile 导出 pnpm-lock.yaml |
| 9.5 | P1 | `resolver` + `fetcher` | `patch:` 协议和补丁应用 |
| 9.6 | P1 | `workspace` + `resolver` | catalogs 和 `catalog:` 协议 |
| 9.7 | P2 | `cli` + `core` | deploy 模式 |

## Peer Dependencies

### 问题

peerDependencies 描述“由使用者环境提供”的依赖。相同版本的包在不同 peer 环境中可能必须形成不同实例。

```json
{
  "dependencies": {
    "react": "^18.2.0",
    "react-dom": "^18.2.0"
  }
}
```

`react-dom` 声明 `peerDependencies.react = ^18`，它不能只按 `react-dom@18.2.0` 去重。

### 数据模型

Phase 9 将源码身份和安装身份分开：

```rust
pub struct PackageId {
    pub name: PackageName,
    pub version: Version,
}

pub struct PackageInstanceId {
    pub package: PackageId,
    pub peer_context: PeerContext,
}

pub struct PeerContext {
    pub resolved: BTreeMap<PackageName, PackageId>,
}

pub struct PeerRequirement {
    pub requester: PackageId,
    pub name: PackageName,
    pub range: VersionConstraint,
    pub optional: bool,
}
```

`DependencyGraph` 改为以 `PackageInstanceId` 为节点：

```rust
pub struct DependencyGraph {
    pub packages: BTreeMap<PackageInstanceId, ResolvedPackage>,
    pub edges: BTreeMap<PackageInstanceId, BTreeMap<PackageName, PackageInstanceId>>,
    pub direct_deps: BTreeMap<ImporterId, BTreeMap<PackageName, PackageInstanceId>>,
    pub diagnostics: Vec<ResolverDiagnostic>,
}
```

### 解析算法

resolver 使用两阶段解析：

```txt
1. Source resolution
   package name + semver range -> PackageId
   只决定 tarball 来源和普通 dependencies。

2. Instance resolution
   PackageId + ancestor peer environment -> PackageInstanceId
   计算 peer_context，生成实际 node_modules 实例。
```

伪代码：

```txt
resolve_instance(pkg, parent_env):
  metadata = source_resolve(pkg)
  peer_context = {}

  for peer in metadata.peer_dependencies:
    candidate = parent_env.find(peer.name)
    if candidate exists and satisfies(candidate.version, peer.range):
      peer_context[peer.name] = candidate
    else if peer is optional:
      warn optional peer missing
    else:
      diagnostic missing/conflict

  instance = PackageInstanceId(pkg, peer_context)
  if memo contains instance:
    return instance

  child_env = parent_env + metadata.dependencies + peer_context
  for dep in metadata.dependencies:
    dep_pkg = source_resolve(dep)
    dep_instance = resolve_instance(dep_pkg, child_env)
    add edge instance -> dep_instance

  return instance
```

`parent_env` 是从 importer 到当前包的可见依赖环境，优先级：

1. 当前 package 自己的 dependencies。
2. 父链提供的 dependencies。
3. importer 直接依赖。
4. workspace root 公共依赖。

### peer optional 与冲突诊断

支持 `peerDependenciesMeta` 中的 `optional`：

```json
{
  "peerDependencies": {
    "typescript": ">=5"
  },
  "peerDependenciesMeta": {
    "typescript": {
      "optional": true
    }
  }
}
```

| 类型 | 默认级别 | 行为 |
| --- | --- | --- |
| missing peer | warning | 继续安装 |
| optional peer missing | info | 继续安装 |
| peer version conflict | warning | 继续安装，但 lockfile 记录 |
| strict peer conflict | error | `strict-peer-dependencies=true` 时失败 |

错误示例：

```txt
warning: unmet peer dependency
  react-dom@18.2.0 requires react@^18
  found react@17.0.2 from importer .
hint: update react to ^18 or install a compatible react-dom version
```

### Package key

lockfile package key 增加 peer suffix：

```yaml
packages:
  /react-dom@18.2.0(react@18.2.0):
    peerDependencies:
      react: ^18.0.0
    peerDependenciesMeta: {}
```

peer suffix 规则：

- peer 名称按字典序排序。
- scope 包名中的 `/` 保持原样。
- 版本使用解析后的精确版本。
- 缺失 optional peer 不进入 suffix。

## pnpm-lock.yaml 兼容

### 读取

`lockfile` 增加 pnpm schema 类型，不直接复用 orix 内部类型：

```rust
pub struct PnpmLockfile {
    pub lockfile_version: PnpmLockfileVersion,
    pub importers: BTreeMap<String, PnpmImporter>,
    pub packages: BTreeMap<String, PnpmPackage>,
    pub snapshots: BTreeMap<String, PnpmSnapshot>,
    pub patched_dependencies: BTreeMap<String, String>,
}
```

转换流程：

```txt
pnpm-lock.yaml
  -> PnpmLockfile
  -> validate supported schema version
  -> normalize package keys
  -> map importers
  -> map packages + snapshots
  -> Orix Lockfile
```

Phase 9 读取支持：

- lockfileVersion `6.x`、`9.x` 的常见字段。
- importers dependencies/devDependencies/optionalDependencies。
- packages resolution.integrity/tarball。
- peer suffix package key。
- patchedDependencies。

遇到未知字段时保留 warning，不因额外字段失败。遇到无法映射的协议时给出明确 unsupported error。

CLI：

```bash
orix import --from pnpm-lock.yaml
orix install --prefer-pnpm-lock
```

### 导出

导出目标是让 pnpm 能理解依赖图，不保证逐字节复刻 pnpm 输出。

```bash
orix export --to pnpm-lock.yaml
```

导出规则：

- `orix-lock.yaml` 是权威来源。
- pnpm lockfile package key 从 `PackageInstanceId` 生成。
- `importers` 保留原始 specifier。
- `patchedDependencies` 从 orix patch 元数据生成。
- 不支持的 orix 扩展写入 `x-orix` 字段，pnpm 可忽略。

## Patch 协议

### 用户模型

支持两种输入：

```json
{
  "dependencies": {
    "vite": "patch:vite@5.0.0#./patches/vite.patch"
  }
}
```

以及 pnpm 风格配置：

```json
{
  "pnpm": {
    "patchedDependencies": {
      "vite@5.0.0": "patches/vite.patch"
    }
  }
}
```

Phase 9 先支持应用已有 patch 文件；`orix patch` / `orix patch-commit` 可作为后续 DX 命令。

### 应用时机

```txt
fetch tarball
extract to temp dir
apply patch
verify patched tree
import patched files into CAS store
record patch metadata in lockfile
link
```

补丁应用由 `fetcher` 完成，因为它已经拥有解压后的临时目录。`store` 只接收最终文件树，不理解 patch 语义。

lockfile 记录：

```yaml
packages:
  /vite@5.0.0:
    resolution:
      tarball: https://registry.npmjs.org/vite/-/vite-5.0.0.tgz
      integrity: sha512-...
    patch:
      path: patches/vite.patch
      integrity: sha256-...
```

`--frozen-lockfile` 检查 patch 文件 hash 是否与 lockfile 匹配。

## Catalogs

`pnpm-workspace.yaml`：

```yaml
packages:
  - packages/*

catalog:
  react: ^18.2.0
  typescript: ^5.4.0

catalogs:
  react19:
    react: ^19.0.0
    react-dom: ^19.0.0
```

package.json：

```json
{
  "dependencies": {
    "react": "catalog:",
    "typescript": "catalog:"
  },
  "devDependencies": {
    "react-dom": "catalog:react19"
  }
}
```

解析规则：

1. `catalog:` 读取默认 `catalog`。
2. `catalog:name` 读取 `catalogs[name]`。
3. catalog 中必须存在同名 package。
4. catalog 展开在 resolver 选择版本前完成。
5. lockfile importer 同时记录原始 specifier 和展开后的 resolved specifier。

```yaml
importers:
  packages/app:
    dependencies:
      react:
        specifier: catalog:
        version: 18.2.0
        resolvedSpecifier: ^18.2.0
```

catalog 只定义版本策略，不改变 package name，也不隐式添加依赖。

## Deploy 模式

`orix deploy` 用于从 workspace 中抽取某个 package 的生产运行目录。

```bash
orix deploy --filter @scope/api --prod dist/api
orix deploy --filter apps/web --prod --frozen-lockfile out/web
```

流程：

```txt
read workspace + lockfile
select target importer
compute production dependency closure
materialize package files
materialize node_modules for closure
copy lockfile subset and package.json
run optional deploy hooks (disabled by default)
```

文件包含策略：

1. 如果 package.json 有 `files` 字段，按 `files` 白名单。
2. 否则复制 package 根目录，排除 `.git`、`node_modules`、`target`、store/cache、测试快照等。
3. 始终保留 `package.json`、README、LICENSE。

deploy 不发布，不运行 registry 交互，不替代 `npm publish`。

## Lockfile 格式升级

Phase 9 需要将 `orix-lock.yaml` 升级到版本 2：

```yaml
lockfileVersion: 2

settings:
  strictPeerDependencies: false

importers:
  .:
    dependencies:
      react:
        specifier: ^18.2.0
        version: 18.2.0

packages:
  /react-dom@18.2.0(react@18.2.0):
    name: react-dom
    version: 18.2.0
    peerDependencies:
      react: ^18.0.0
    peerDependenciesMeta: {}
    dependencies:
      scheduler: 0.23.2
    transitivePeerDependencies: []
```

迁移策略：

- v1 lockfile 可读。
- 写入时默认输出 v2。
- v1 包键按空 peer context 迁移。
- `orix lockfile migrate` 可选提供显式迁移命令。

## 测试计划

| 场景 | 类型 |
| --- | --- |
| 同一包在不同 peer 环境下生成不同 instance | resolver unit |
| 缺失 peer warning | resolver unit |
| strict peer conflict 失败 | resolver unit |
| optional peer 缺失不失败 | resolver unit |
| peer suffix lockfile 稳定排序 | lockfile snapshot |
| 读取 pnpm-lock.yaml v6/v9 fixture | lockfile fixture |
| 导出 pnpm-lock.yaml 后可被 pnpm 解析 | integration |
| patch 文件 hash 变化触发 frozen 失败 | core integration |
| patch 后文件进入 CAS store | fetcher/store integration |
| catalog 展开默认和命名 catalog | workspace/resolver unit |
| deploy 只包含 prod closure | integration |

## 实施顺序

1. `domain` 引入 `PackageInstanceId`、`PeerContext`。
2. `resolver` 改为两阶段解析，保留无 peer 项目的兼容路径。
3. `lockfile` 支持 v2 package key 和 v1 -> v2 读取迁移。
4. `cli` 渲染 peer diagnostics。
5. `workspace` 解析 catalogs，resolver 展开 `catalog:`。
6. `lockfile` 读取 pnpm-lock.yaml。
7. `fetcher` 支持 patch 应用，lockfile 记录 patch metadata。
8. `lockfile` 导出 pnpm-lock.yaml。
9. `core` + `cli` 实现 deploy。
