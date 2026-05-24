可以，建议目标不要叫“Rust 化 pnpm”，而是：

> **做一个 pnpm 兼容思路的 Rust 包管理器：高性能 install + CAS Store + node_modules linker。**

pnpm 的关键设计是：包文件放进全局 **content-addressable store**，项目里的 `node_modules/.pnpm` 通过 hardlink 指向 store，再用 symlink 拼出 Node 可解析的依赖结构。官方也把安装分成“依赖解析、目录结构计算、链接依赖”三阶段。([pnpm.io][1])

## 1. 项目定位

名字可以叫：

```txt
orix
oxide-pm
fepm
rust-pnpm
```

第一阶段不要追求完整兼容 pnpm，而是做：

```txt
package.json 解析
lockfile 生成
npm registry 拉包
tarball 解压
CAS 全局缓存
node_modules/.pnpm 结构生成
依赖 symlink
workspace 最小支持
```

暂时不做：

```txt
peerDependencies 完整算法
hoist 全模式
publish
patch
catalogs
deploy
复杂 lifecycle scripts 沙箱
```

## 2. 总体架构

```txt
orix
├─ crates/
│  ├─ cli              # 命令入口
│  ├─ config           # .npmrc / registry / proxy / store 配置
│  ├─ manifest         # package.json 解析
│  ├─ resolver         # semver 解析、依赖图求解
│  ├─ registry         # npm registry API
│  ├─ fetcher          # tarball 下载、缓存
│  ├─ store            # content-addressable store
│  ├─ lockfile         # orix-lock.yaml
│  ├─ linker           # node_modules/.pnpm + symlink/hardlink
│  ├─ workspace        # workspace 包扫描
│  ├─ lifecycle        # postinstall 等脚本，后置做
│  └─ core             # install pipeline 编排
└─ Cargo.toml
```

核心流程：

```txt
orix install
↓
读取 package.json / workspace
↓
解析依赖版本
↓
生成依赖图
↓
下载 tarball
↓
解压到 CAS store
↓
生成 lockfile
↓
计算 node_modules 布局
↓
hardlink 到 node_modules/.pnpm
↓
symlink 顶层依赖
```

## 3. Store 设计

pnpm 的 store 省空间是因为 hardlink 指向同一份磁盘内容。([pnpm.io][2])

Rust 版可以设计成：

```txt
~/.orix/store/v1/
├─ files/
│  ├─ sha512/
│  │  ├─ ab/cd/<hash>
├─ packages/
│  ├─ react@18.2.0/
│  │  ├─ integrity.json
│  │  └─ files-index.json
```

每个文件按 hash 入库：

```txt
file content -> sha512 -> store/files/sha512/xx/yy/hash
```

然后 package index 记录：

```json
{
  "name": "react",
  "version": "18.2.0",
  "files": {
    "index.js": "sha512-xxx",
    "package.json": "sha512-yyy"
  }
}
```

这样同包不同版本如果只有一个文件变化，只新增变化的文件。

## 4. node_modules 布局

MVP 可以先实现无 peerDependencies 的结构：

```txt
node_modules/
├─ react -> .pnpm/react@18.2.0/node_modules/react
├─ vite -> .pnpm/vite@5.0.0/node_modules/vite
└─ .pnpm/
   ├─ react@18.2.0/
   │  └─ node_modules/
   │     └─ react/
   └─ vite@5.0.0/
      └─ node_modules/
         ├─ vite/
         └─ rollup -> ../../rollup@4.0.0/node_modules/rollup
```

规则：

```txt
1. 每个包实体放到 node_modules/.pnpm/<pkg>@<version>/node_modules/<pkg>
2. 包文件从 store hardlink 过去
3. 该包自己的依赖 symlink 到同级 node_modules 下
4. 项目直接依赖 symlink 到根 node_modules
```

这就是 pnpm 最核心的“严格依赖隔离”。

## 5. 依赖解析算法

MVP 版本：

```txt
输入 package.json dependencies
↓
从 registry 获取 packument
↓
根据 semver range 选最高满足版本
↓
递归解析 dependencies
↓
生成 dependency graph
```

数据结构：

```rust
struct PackageId {
    name: String,
    version: Version,
}

struct ResolvedPackage {
    id: PackageId,
    integrity: String,
    tarball: String,
    dependencies: BTreeMap<String, String>,
}

struct DependencyGraph {
    packages: BTreeMap<PackageId, ResolvedPackage>,
}
```

先不处理复杂 peer，后面再加：

```txt
react-dom@18(peer react@18)
=> react-dom@18_react@18.2.0
```

也就是 peer 参与 package key。

## 6. Lockfile 设计

可以先自定义：

```yaml
lockfileVersion: 1

importers:
  .:
    dependencies:
      react: 18.2.0

packages:
  react@18.2.0:
    resolution:
      tarball: https://registry.npmjs.org/react/-/react-18.2.0.tgz
      integrity: sha512-xxx
    dependencies:
      loose-envify: ^1.1.0
```

命令支持：

```bash
orix install
orix install --frozen-lockfile
orix add react
orix remove react
```

## 7. Rust 技术选型

```toml
[dependencies]
clap = "4"              # CLI
tokio = "1"             # async runtime
reqwest = "0.12"        # registry/tarball download
serde = "1"
serde_json = "1"
serde_yaml = "0.9"
semver = "1"
tar = "0.4"
flate2 = "1"
sha2 = "0.10"
hex = "0.4"
walkdir = "2"
thiserror = "2"
anyhow = "1"
rayon = "1"             # 并行 hardlink/copy
```

Windows symlink 注意：

```txt
目录 symlink 可能需要开发者模式/管理员权限
```

所以 Windows 下优先策略：

```txt
junction > symlink
```

文件链接策略：

```txt
hardlink 优先
失败 fallback copy
```

## 8. MVP 开发阶段

### Phase 1：本地 manifest + CLI

目标：

```bash
orix install
```

实现：

```txt
读取 package.json
解析 dependencies/devDependencies
打印待安装列表
```

### Phase 2：registry resolver

实现：

```txt
GET https://registry.npmjs.org/react
semver range -> version
拿 tarball/integrity/dependencies
```

### Phase 3：fetch + extract

实现：

```txt
下载 .tgz
校验 integrity
解压 package/
```

### Phase 4：CAS store

实现：

```txt
每个文件 hash 入库
生成 package files-index
重复文件跳过
```

### Phase 5：linker

实现：

```txt
创建 node_modules/.pnpm
hardlink package files
创建根依赖 symlink
创建子依赖 symlink
```

这是第一个真正可用版本。

### Phase 6：lockfile

实现：

```txt
首次 install 生成 lockfile
已有 lockfile 优先安装
frozen-lockfile 校验
```

### Phase 7：workspace

支持：

```yaml
packages:
  - packages/*
  - apps/*
```

规则：

```txt
workspace:* 优先链接本地包
非 workspace 走 registry
```

### Phase 8：scripts

支持：

```txt
preinstall
install
postinstall
prepare
```

但要加白名单或配置，因为脚本执行有安全风险。

### Phase 9：peerDeps + 生态兼容

支持：

```txt
peerDependencies 完整解析（hoisting 策略）
pnpm-lock.yaml 读取兼容
pnpm-lock.yaml 导出兼容
patch 协议
catalogs
deploy
```

## 9. 最难的地方

真正难点不是下载包，而是这些：

```txt
peerDependencies 解析
optionalDependencies 平台过滤
bin 链接
workspace 协议
.npmrc 兼容
registry auth token
lifecycle scripts
Windows symlink/junction 差异
lockfile 与 pnpm 兼容
```

所以建议第一版别做“完全兼容 pnpm-lock.yaml”，先做自己的 lockfile。

## 10. 最小代码骨架

```rust
// crates/core/src/install.rs
pub async fn install(project_root: &Path) -> Result<()> {
    let manifest = Manifest::read(project_root.join("package.json"))?;

    let config = Config::load(project_root)?;
    let resolver = Resolver::new(config.registry.clone());

    let graph = resolver.resolve_manifest(&manifest).await?;

    let store = Store::open(config.store_dir)?;
    let fetcher = Fetcher::new(config.registry.clone());

    for package in graph.packages.values() {
        let tarball = fetcher.fetch(package).await?;
        store.import_tarball(package, tarball).await?;
    }

    Lockfile::write(project_root.join("orix-lock.yaml"), &graph)?;

    let linker = Linker::new(store, project_root.join("node_modules"));
    linker.link_graph(&graph, &manifest)?;

    Ok(())
}
```

## 11. 你的最佳切入点

我建议你不要一上来做全量包管理器，而是先做：

```txt
mini-pnpm-rs
```

第一目标：

```txt
安装 react + vite 这种普通依赖
能生成 node_modules
能 node -e "require('react')" 成功
```

第二目标：

```txt
支持 workspace 本地包软链
```

第三目标：

```txt
支持 peerDependencies
```

这个项目做出来，对你研究：

```txt
pnpm
Node 依赖解析
前端工程化
Rust CLI
包管理器底层
```

都很有价值。

[1]: https://pnpm.io/symlinked-node-modules-structure?utm_source=chatgpt.com "Symlinked `node_modules` structure"
[2]: https://pnpm.io/faq?utm_source=chatgpt.com "Frequently Asked Questions"
