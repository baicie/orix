# orix MVP 实施计划

> 状态说明：
> - ✅ 已完成 — 核心逻辑已实现
> - ⚠️ 部分完成 — 核心逻辑完成，CLI/集成待补
> - 🔴 待实施 — 尚未实现

---

## Phase 1 — 本地 manifest + CLI

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 1.1  | Manifest 解析 package.json（name, version, dependencies, devDependencies, scripts 等） | `manifest` | ✅ |
| 1.2  | CLI 命令行参数解析（install / add / remove / store） | `cli` | ✅ |
| 1.3  | CLI 输出美化（进度条、树状输出、颜色） | `cli` | ⚠️ 部分完成 |
| 1.4  | 错误信息格式化（人类可读错误 + hint） | `cli` | ⚠️ stub |

**1.3/1.4 细节：** 当前 `cli/src/main.rs` 已有基础命令框架，但进度条、树状输出、彩色输出、友好的错误提示（如 "Package not found: ..."）需要完善。

---

## Phase 2 — Registry Resolver

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 2.1  | Registry packument API（GET /<package>） | `registry` | ✅ |
| 2.2  | semver 版本选择（^ / ~ / >= / latest / exact） | `resolver` | ✅ |
| 2.3  | 带记忆化的 DFS 递归解析 | `resolver` | ✅ |
| 2.4  | platform/os/cpu 过滤 | `resolver` | ⚠️ stub |
| 2.5  | peerDependencies MVP 处理（跳过，报 warning） | `resolver` | ⚠️ stub |
| 2.6  | workspace:* 协议解析 | `resolver` | ✅ |
| 2.7  | packument HTTP 缓存（TTL 5min） | `resolver` | ⚠️ stub |
| 2.8  | Registry 认证 token（Bearer token from .npmrc） | `registry` | ⚠️ stub |

**2.4/2.5 细节：** 平台过滤在设计文档中提及 MVP 只记录 warning，不硬性失败。peerDependencies 在设计文档中明确 MVP 不强制解析。

---

## Phase 3 — Fetcher

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 3.1  | tarball 下载（reqwest） | `fetcher` | ✅ |
| 3.2  | integrity 验证（sha512 / sha1，常数时间比较） | `fetcher` | ✅ |
| 3.3  | tarball 解压（tar + flate2） | `fetcher` | ✅ |
| 3.4  | tarball 本地缓存（~/.orix/cache/tarballs/） | `fetcher` | ✅ |
| 3.5  | 并发下载控制（Semaphore，concurrency=10） | `fetcher` | ✅ |
| 3.6  | 下载重试 + 指数退避 | `fetcher` | ⚠️ stub |
| 3.7  | offline 模式（仅使用缓存） | `fetcher` | ⚠️ stub |

---

## Phase 4 — CAS Store

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 4.1  | Store 目录结构（files/sha256/xx/yy/hash, packages/name@ver/） | `store` | ✅ |
| 4.2  | 文件内容 hash + 去重入库 | `store` | ✅ |
| 4.3  | integrity.json 生成与读取 | `store` | ✅ |
| 4.4  | 包硬链接策略（hardlink → copy → warn） | `store` | ✅ |
| 4.5  | Store 原子写入（临时文件 → rename） | `store` | ⚠️ stub |
| 4.6  | 并发安全（读写锁） | `store` | ⚠️ stub |
| 4.7  | `orix store prune` 清理未引用包 | `store` + `cli` | 🔴 待实施 |
| 4.8  | `orix store verify` store 完整性校验 | `store` + `cli` | 🔴 待实施 |
| 4.9  | `orix store path` 打印 store 路径 | `cli` | 🔴 待实施 |

---

## Phase 5 — Linker

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 5.1  | .pnpm/ 目录结构生成 | `linker` | ✅ |
| 5.2  | 根依赖 symlink（node_modules/react -> .pnpm/...） | `linker` | ✅ |
| 5.3  | 子依赖 symlink（.pnpm/pkg/node_modules/dep -> ../../dep@ver/...） | `linker` | ✅ |
| 5.4  | 相对路径计算（platform 内依赖链接公式） | `linker` | ✅ |
| 5.5  | Windows junction 回退（symlink 失败时） | `linker` | ⚠️ 部分完成 |
| 5.6  | 布局验证（validate_layout，检测 broken symlink） | `linker` | ⚠️ stub |
| 5.7  | `orix remove` 清理（unlink + 删除 .pnpm 条目） | `linker` | ⚠️ stub |

**5.6 细节：** `linker/src/linker.rs` 中 `validate_layout` 方法需要完整实现，遍历 node_modules/ 检测 broken symlink 并验证直接依赖可解析。

---

## Phase 6 — Lockfile

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 6.1  | orix-lock.yaml 读写（serde_yaml） | `lockfile` | ✅ |
| 6.2  | importers / packages 分离结构 | `lockfile` | ✅ |
| 6.3  | Lockfile.update()（合并新解析结果） | `lockfile` | ✅ |
| 6.4  | Lockfile.diff()（计算 added/removed/changed） | `lockfile` | ⚠️ stub |
| 6.5  | frozen-lockfile 验证（--frozen-lockfile） | `lockfile` | ✅ |
| 6.6  | Lockfile 原子写入（临时文件 → rename） | `lockfile` | ⚠️ stub |
| 6.7  | 解析结果写入 importer specifiers | `lockfile` | ⚠️ stub |

---

## Phase 7 — Workspace

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 7.1  | pnpm-workspace.yaml 解析 | `workspace` | ✅ |
| 7.2  | Workspace 发现算法（glob 模式匹配） | `workspace` | ✅ |
| 7.3  | workspace:* 协议解析 | `workspace` | ✅ |
| 7.4  | workspace:^ / workspace:~ / workspace:>=1.0.0 协议变体 | `workspace` | 🔴 待实施 |
| 7.5  | workspace:file:../utils 协议 | `workspace` | 🔴 待实施 |
| 7.6  | 本地包 symlink（link_workspace_dep） | `workspace` | ⚠️ stub |
| 7.7  | 根目录 workspace 安装（共享 orix-lock.yaml） | `workspace` | ⚠️ stub |
| 7.8  | 循环 workspace 依赖检测 | `workspace` | 🔴 待实施 |

---

## Phase 8 — 安装管道（Pipeline）

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 8.1  | core::install() 完整管道编排 | `core` | ✅ |
| 8.2  | core::add()（修改 package.json + install） | `core` | ✅ |
| 8.3  | core::remove()（修改 package.json + install） | `core` | ✅ |
| 8.4  | frozen-lockfile 流程（resolve from lockfile） | `core` | ✅ |
| 8.5  | force 流程（重新获取所有包） | `core` | ⚠️ stub |
| 8.6  | CoreError 枚举（聚合所有子 crate 错误） | `core` | ✅ |
| 8.7  | InstallReport / FetchReport / LinkReport 结构 | `core` | ⚠️ 部分完成 |

---

## Phase 9 — Config

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 9.1  | Config 结构体（registry, store_dir, cache_dir, etc.） | `config` | ✅ |
| 9.2  | .npmrc 文件解析 | `config` | ⚠️ 部分完成 |
| 9.3  | 环境变量覆盖（RPNPM_REGISTRY, RPNPM_STORE, etc.） | `config` | ⚠️ stub |
| 9.4  | 用户 ~/.npmrc 加载 | `config` | ⚠️ stub |
| 9.5  | CLI 参数覆盖（最高优先级） | `config` | ⚠️ stub |
| 9.6  | hoist-patterns / side-effects-cache 配置 | `config` | 🔴 待实施 |

---

## Phase 10 — Utils & Macros

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 10.1 | 包名规范化（normalize_name → 小写 + 规范化斜杠） | `utils` | ⚠️ stub |
| 10.2 | 路径工具函数 | `utils` | 🔴 待实施 |
| 10.3 | 过程宏（预留） | `macros` | ⚠️ stub |

---

## Phase 11 — Domain

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 11.1 | PackageId / Version / PackageName / VersionConstraint | `domain` | ✅ |
| 11.2 | ResolvedPackage / DependencyGraph | `domain` | ✅ |
| 11.3 | PackageKey / ImporterId 类型别名 | `domain` | ✅ |
| 11.4 | Integritty string 解析器 | `domain` | 🔴 待实施 |
| 11.5 | tarball URL builder | `domain` | 🔴 待实施 |

---

## Phase 12 — 测试

| ID   | 任务 | Crate | 状态 |
|------|------|-------|------|
| 12.1 | manifest 解析测试（fixture: valid/invalid package.json） | `manifest` | ✅ |
| 12.2 | resolver 单元测试（semver 选择逻辑） | `resolver` | ✅ |
| 12.3 | store 文件去重测试 | `store` | 🔴 待实施 |
| 12.4 | linker 布局算法测试 | `linker` | 🔴 待实施 |
| 12.5 | lockfile 读写/diff 测试 | `lockfile` | 🔴 待实施 |
| 12.6 | integration tests（真实 npm 包安装测试） | `tests/` | ✅ |
| 12.7 | Windows CI 测试（symlink / junction 行为） | CI | 🔴 待实施 |

---

## Phase 13 — 集成 & 质量

| ID   | 任务 | 状态 |
|------|------|------|
| 13.1 | `cargo xtask check` 完整（fmt + clippy + test） | ✅ |
| 13.2 | `cargo deny check` CI 集成 | 🔴 待实施 |
| 13.3 | `cargo machete` 依赖检查 | 🔴 待实施 |
| 13.4 | CI/CD workflows（Ubuntu + Windows + macOS） | ⚠️ 模板待替换 |
| 13.5 | 文档：README.md | 🔴 待实施 |
| 13.6 | 文档：CONTRIBUTING.md | 🔴 待实施 |
| 13.7 | 性能测试 / benchmark | 🔴 待实施 |

---

## 优先级排序（推荐实施顺序）

### P0 — 让 MVP 可运行（核心链路打通）
```
12.6 integration tests (最简单端到端测试)✅
  ↓
3.5  并发下载（影响安装速度）✅
  ↓
6.5  frozen-lockfile 验证（CI 必需）✅
  ↓
8.4  frozen-lockfile 流程✅
  ↓
12.1 manifest 解析测试✅
  ↓
12.2 resolver 单元测试✅
```

### P1 — CLI 体验完善
```
1.3  CLI 进度条输出
1.4  人类可读错误信息
13.5 README.md
```

### P2 — Store 管理命令
```
4.7  store prune
4.8  store verify
4.9  store path
```

### P3 — Workspace 完整支持
```
7.4  workspace 协议变体（^/~/>=）
7.6  本地包 symlink
7.7  根目录 workspace 安装
7.8  循环依赖检测
```

### P4 — 细节打磨
```
2.6  packument HTTP 缓存
2.8  Registry 认证 token
3.6  下载重试 + 指数退避
3.7  offline 模式
5.6  布局验证
5.7  remove 清理
6.4  lockfile diff
6.6  lockfile 原子写入
9.3  环境变量覆盖
```

### P5 — 可选增强
```
10.1 包名规范化
10.2 路径工具函数
11.4 integrity string 解析
11.5 tarball URL builder
12.7 Windows CI 测试
13.2 cargo deny
13.3 cargo machete
13.7 benchmark
```

---

## 当前进度总览

```
Phase 1  CLI + manifest      ████████░░  75%
Phase 2  Resolver             ████████░░  70%
Phase 3  Fetcher             ████████░░  75%
Phase 4  CAS Store           ████████░░  65%
Phase 5  Linker              ████████░░  80%
Phase 6  Lockfile           ████████░░  65%
Phase 7  Workspace          ████████░░  65%
Phase 8  Pipeline           █████████░  85%
Phase 9  Config              ██████░░░░  50%
Phase 10 Utils & Macros     ██░░░░░░░░  15%
Phase 11 Domain             ████████░░  70%
Phase 12 测试                ░░░░░░░░░░   0%
Phase 13 集成 & 质量         █░░░░░░░░░   5%
```

**总体完成度：~55%**
