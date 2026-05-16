# orix

高性能 Rust 包管理器，采用 pnpm 兼容设计。

## 特性

- **pnpm 兼容**：生成 `node_modules/.pnpm` 结构，支持 workspace 协议
- **全局 CAS 缓存**：内容可寻址存储，跨项目复用 tarball 文件
- **快速安装**：并发下载 + 文件级去重 + 硬链接
- **Lockfile**：可重现安装，支持 `--frozen-lockfile` 在 CI 中验证
- **Workspace 支持**：monorepo 多包管理，支持 workspace 协议变体（`workspace:*`、`workspace:^`、`workspace:~`、`workspace:>=`、`workspace:file:`）
- **跨平台**：Linux、macOS、Windows（带 junction 回退）

## 安装

```bash
cargo install --path crates/cli
```

或从源码构建：

```bash
cargo build -p orix-cli
./target/debug/orix --help
```

## 快速开始

```bash
# 安装依赖
orix install

# 从 lockfile 验证安装（CI 用）
orix install --frozen-lockfile

# 只用本地缓存
orix install --offline

# 指定 registry
orix install --registry https://registry.npmmirror.com
```

## 项目结构

```
crates/
├── cli          # 命令行入口
├── core         # 安装管道编排
├── config       # .npmrc 配置加载
├── domain       # 共享领域类型
├── manifest     # package.json 解析
├── resolver     # 依赖图构建 + semver 解析
├── registry     # npm registry API 客户端
├── fetcher      # tarball 下载与解压
├── store        # 内容可寻址全局缓存
├── lockfile     # orix-lock.yaml 管理
├── linker       # node_modules/.pnpm 结构生成
├── workspace    # workspace 发现与循环依赖检测
├── utils        # 共享工具函数
└── macros       # 过程宏（预留）
```

## 开发

```bash
# 格式化 + linter + 测试
cargo xtask check

# 构建
cargo build --workspace

# 测试
cargo test --workspace

# 文档
cargo doc --no-deps
```

可选工具：

```bash
cargo install cargo-deny cargo-machete cargo-llvm-cov
cargo machete
cargo deny check
```

## 架构

详见 [docs/.project/design/](docs/.project/design/) 下的设计文档。

### 安装流程

```
orix install
  → Config.resolve()    加载 .npmrc / env / CLI 参数
  → Manifest.read()     解析 package.json
  → Workspace.discover()  查找 pnpm-workspace.yaml
  → Lockfile.read()     加载现有 lockfile
  → Resolver.resolve()  构建依赖图（含 workspace 协议）
  → Registry.fetch_packument()  获取 packument
  → semver 匹配 + platform 过滤
  → Fetcher.fetch_all()  并发下载 tarball
  → Store.import_package()  CAS 去重 + 硬链接
  → Lockfile.update()   写入 orix-lock.yaml
  → Linker.link_graph() 生成 .pnpm 结构
```

## 状态

orix 正在积极开发中。MVP 聚焦以下能力：

- package.json 解析
- lockfile 生成与 frozen-lockfile 校验
- npm registry 拉包
- tarball 下载、完整性校验、解压
- CAS 全局缓存
- node_modules/.pnpm 结构生成
- 根依赖和子依赖链接
- workspace 最小支持

MVP 暂不覆盖：peerDependencies 完整算法、全模式 hoist、publish/patch/catalogs、复杂 lifecycle scripts 沙箱。

## License

MIT
