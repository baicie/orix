# 测试、集成与质量设计

## 概述

本文档补齐 TODO Phase 12 和 Phase 13 的设计。orix 的核心风险来自文件系统布局、跨平台链接、lockfile 可重现性和 registry 网络行为，因此测试策略必须覆盖“纯逻辑 → 文件系统 → 端到端安装 → CI 平台差异”四层。

完整质量门禁以 `make check` 为准，等价于 `cargo xtask check`。

## 测试分层

```txt
单元测试
  ├─ manifest JSON 解析
  ├─ resolver semver 选择
  ├─ domain 包名 / integrity / platform 匹配
  └─ lockfile diff 纯逻辑

crate 集成测试
  ├─ store 文件去重与 verify
  ├─ linker .pnpm 布局和 symlink/junction
  ├─ fetcher tarball 缓存与 integrity
  └─ workspace discovery 和 workspace 协议

workspace 端到端测试
  ├─ 最小 npm 包 install
  ├─ frozen-lockfile 成功 / 失败
  ├─ add / remove 后 lockfile 和 node_modules 更新
  └─ workspace 根目录安装

CI / 质量工具
  ├─ fmt + clippy + test
  ├─ cargo deny
  ├─ cargo machete
  ├─ coverage
  └─ benchmark
```

## 测试数据

测试 fixture 放在对应 crate 的 `tests/fixtures/`，跨 crate 端到端 fixture 放在 workspace root 的 `tests/fixtures/`。

建议结构：

```txt
tests/
├── integration.rs
└── fixtures/
    ├── simple-package/
    │   └── package.json
    ├── workspace-basic/
    │   ├── pnpm-workspace.yaml
    │   └── packages/
    └── registry/
        ├── is-odd-packument.json
        └── is-number-7.0.0.tgz
```

网络相关测试默认不访问真实 npm registry。优先使用本地 HTTP mock server 或 fixture packument。真实 registry smoke test 只允许放在明确标记的 ignored 测试中，CI 默认不跑。

## Phase 12 覆盖计划

### 12.1 Manifest 解析测试

覆盖：

- 空 `{}` manifest 可解析。
- 标准 `dependencies`、`devDependencies`、`optionalDependencies`、`peerDependencies` 字段可解析。
- `bin` 的 string shorthand 和 map 两种形式可解析。
- 非法 JSON 返回带路径上下文的错误。
- `version` 非法时，转换到 `Version` 返回错误。

测试位置：`crates/manifest/src/lib.rs` 单元测试和 `crates/manifest/tests/manifest.rs`。

### 12.2 Resolver 单元测试

覆盖：

- `exact`、`^`、`~`、`>=`、`latest`、dist-tag 的版本选择。
- 无满足版本时返回 `NoSatisfyingVersion`。
- 相同包约束在一次解析中命中 memo，不重复请求 packument。
- platform/os/cpu 不匹配时按 MVP 规则 warning + skip。
- peerDependencies MVP 只记录 warning，不强制失败。

测试策略：

- 将 packument provider 抽象成 trait，测试中注入内存 packument。
- 不在单元测试中启动真实网络请求。

### 12.3 Store 文件去重测试

覆盖：

- 两个包包含相同文件内容时，`files/sha256` 只生成一份内容文件。
- `integrity.json` 能完整 roundtrip。
- 包导入中断不会留下半写入 package entry。
- hardlink 失败时回退 copy，并在 report 中体现。
- `verify` 能发现缺失内容文件、hash mismatch、损坏的 integrity metadata。

测试位置：`crates/store/tests/store.rs`。

### 12.4 Linker 布局算法测试

覆盖：

- 根依赖只链接直接依赖，不把传递依赖暴露到根 `node_modules`。
- `.pnpm/<pkg>@<ver>/node_modules/<dep>` 的相对链接目标正确。
- scoped package 目录结构正确，例如 `node_modules/@scope/name`。
- `validate_layout` 能发现 broken symlink。
- Windows CI 覆盖 junction fallback。

测试应使用临时目录，不依赖用户机器的全局 store。

### 12.5 Lockfile 读写 / diff 测试

覆盖：

- YAML roundtrip 稳定，输出 key 排序确定。
- `importers` 和 `packages` 分离结构正确。
- `diff` 能区分 added、removed、changed、importers_changed。
- `validate_frozen` 检测 package.json specifier 和 lockfile 不一致。
- 原子写入不产生半截 lockfile。

### 12.6 Integration Tests

最小端到端测试：

```txt
fixture/simple-package/package.json
  dependencies:
    is-number: 7.0.0

orix install --registry <mock-registry>

断言：
  orix-lock.yaml 存在
  node_modules/is-number/package.json 可读
  node_modules/.pnpm/is-number@7.0.0/... 布局存在
  第二次 install 命中缓存并成功
```

后续端到端测试：

- `orix install --frozen-lockfile` 在 lockfile 匹配时成功。
- 修改 `package.json` 后 frozen install 失败并给出 hint。
- `orix add` 修改 manifest 并重新安装。
- `orix remove` 移除 manifest 依赖并清理根链接。
- workspace 根安装生成共享 `orix-lock.yaml`。

### 12.7 Windows CI 测试

Windows 单独关注：

- 目录 symlink 不可用时是否回退 junction。
- hardlink 失败时是否回退 copy。
- lockfile 路径是否保持 `/`。
- scoped 包目录在 Windows 路径下是否正确创建。

## Phase 13 质量门禁

### 13.1 `cargo xtask check`

`make check` 必须至少执行：

```txt
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

`xtask` 后续可以增加：

- 文档测试：`cargo test --doc --workspace`
- feature matrix：默认 features 和 all-features 各跑一遍
- examples 编译检查

### 13.2 `cargo deny check`

目标：

- 阻止已知安全漏洞。
- 限制重复依赖版本。
- 约束许可证白名单。
- 检查 banned crate。

建议 CI 运行频率：

- PR 必跑基础 `cargo deny check advisories licenses bans`。
- 每周 security workflow 额外跑完整检查。

### 13.3 `cargo machete`

目标是发现未使用依赖，避免 workspace 依赖膨胀。

策略：

- PR 中可以作为非阻塞检查先引入。
- 一旦 false positive 被配置文件处理完，再提升为阻塞。
- 新增依赖时必须放在根 `Cargo.toml` 的 `[workspace.dependencies]`。

### 13.4 CI/CD Workflows

CI matrix：

```yaml
os: [ubuntu-latest, windows-latest, macos-latest]
rust: [stable]
```

每个平台运行：

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --all-targets`

Ubuntu 额外运行：

- coverage
- cargo deny
- cargo machete
- docs build

Release workflow：

- tag push 触发。
- 构建 Linux、macOS、Windows binary。
- 生成 checksum。
- 发布 GitHub release。

### 13.5 README

README 应覆盖：

- 项目定位和 MVP 范围。
- 安装与构建方式。
- `orix install/add/remove/store` 常用命令。
- 当前 pnpm 兼容范围。
- 已知限制：peerDependencies、lifecycle scripts、pnpm-lock.yaml、hoist。

### 13.6 CONTRIBUTING

贡献指南应覆盖：

- 本地开发命令。
- crate 边界和依赖方向。
- 新增依赖规则。
- 测试要求。
- PR checklist。
- Windows symlink/junction 注意事项。

### 13.7 Benchmark

性能测试分三类：

| Benchmark | 指标 |
| --- | --- |
| resolver | packument 数量、解析耗时、memo 命中率 |
| fetcher/store | 下载并发、hash 耗时、导入吞吐 |
| linker | 文件数、hardlink/copy 数量、布局耗时 |

建议使用 `criterion`，fixture 固定，避免真实网络。输出只比较趋势，不把绝对时间作为硬性 CI 门禁。性能回归门槛可以后续在 nightly workflow 中启用。

## 最小先行顺序

根据 TODO 的 P0/P1，建议先补这组测试：

1. `tests/integration.rs`：最小端到端安装。
2. `lockfile::validate_frozen` 单元测试。
3. `core::install --frozen-lockfile` 集成测试。
4. `manifest` 标准字段解析测试。
5. `resolver` semver 选择测试。

这条路径优先保护 MVP 主链路：manifest → resolver → fetcher → store → linker → lockfile。
