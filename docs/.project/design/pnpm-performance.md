# pnpm 高性能原理与各阶段优化

pnpm 快不是靠某一项黑科技，而是每个安装阶段都有针对性优化，形成完整的流水线加速。

---

## 一、依赖解析阶段 — 并发 + 去重

### 问题

旧实现用 `while let` 串行等待每个 packument 的 HTTP 响应，N 个包 = N × RTT（网络往返时间）。

### 优化手段

| 手段 | 效果 |
|------|------|
| **有界并发 work queue**（默认 10 并发） | 理论耗时从 `N × RTT` 降到 `ceil(N/10) × RTT` |
| **in-flight 请求去重**（single-flight） | 同一包名同时只发一个 HTTP 请求，多个约束复用同一 packument |
| **内存 TTL 缓存** | 同一次 install 内重复引用只走内存，不走网络 |
| **磁盘 packument 缓存** | 二次安装/重复 install 消除 registry 元数据请求 |
| **版本索引缓存** | packument 只需解析一次版本列表，range 匹配直接遍历已排序数组 |
| **peer 移出热路径** | peer dependency 不在主解析热路径中同步网络查询，只做诊断 |

### 关键数据结构

调度器维护三层状态：

- `pending`：等待解析的 `(PackageName, VersionConstraint)`
- `in_flight_resolution`：已派发但未完成的 resolution key
- `in_flight_packuments`：同一包名共享同一个 fetch future

---

## 二、下载阶段 — tarball 缓存 + 并发

### 问题

每个包的 tarball 都要下载，重复安装时浪费带宽。

### 优化手段

- **tarball 磁盘缓存**：URL → SHA-256 哈希 → `~/.orix/cache/tarballs/sha256/<前缀>/<哈希>.tgz`，重复安装直接命中缓存
- **完整性预校验**：缓存命中时先验 integrity，损坏则自动删除重下
- **有界并发下载**（默认 10）：`Semaphore` 控制并发数，防止压垮 registry
- **下载后立即验签**：写盘后二次校验，捕获坏数据

### 缓存路径结构

```
~/.orix/cache/tarballs/
├── sha256/<url-哈希>/
│   └── <package>-<version>.tgz
```

---

## 三、CAS Store — 文件级去重

### 问题

npm/yarn 全量 hoist 时同一个包的不同版本各占一份磁盘，空间浪费严重。

### 核心设计：内容可寻址存储

每个文件按 SHA-256 **内容哈希**存储，而非按包或按路径：

```
store/files/sha256/ab/cd/abc123...def456
```

**去重键是文件内容哈希，而非文件路径**。这意味着：

- 如果两个包的 `package.json` 内容相同，它们共享一份物理副本
- 即使一个文件略有变化，也会得到新的哈希条目
- store 总大小 = 所有已安装包中唯一文件内容的总和

### 其他优化

- **分片目录**：只用前 2 位十六进制字符分片，避免单目录文件过多
- **包条目目录**：`store/packages/<name>@<ver>/` 从 CAS 硬链接文件，零复制
- **回退链**：硬链接 → 复制 → 记录警告，绝不因文件系统限制而失败安装

### store 路径结构

```
~/.orix/store/v1/
├── files/
│   └── sha256/
│       └── <前缀>/<哈希>           # 去重的文件内容
├── packages/
│   └── <包名>@<版本>/
│       ├── integrity.json             # 包元数据 & 文件索引
│       └── files/                     # 解压后的包文件
```

---

## 四、链接阶段 — 硬链接 + 严格隔离

### 问题

npm/yarn 的全 hoist 产生大量目录和符号链接，Windows 上尤慢。

### 优化手段

| 手段 | 效果 |
|------|------|
| **硬链接优先** | `hard_link` 零复制，跨文件系统时自动回退到复制 |
| **严格隔离**（非全 hoist） | 只对直接依赖创建根目录符号链接，传递依赖不走 hoist，目录数量大幅减少 |
| **平台感知链接** | Windows 优先 junction（不需要管理员权限），Unix 用 symlink |
| **.orix 物理目录 + 相对符号链接** | `node_modules/react` → `.orix/react@18.2.0/node_modules/react`，包内依赖用相对路径 |

### 目标目录结构

```
project-root/
├── node_modules/
│   ├── .orix/                           # 物理包文件
│   │   ├── react@18.2.0/
│   │   │   └── node_modules/
│   │   │       └── react/
│   │   │           ├── index.js
│   │   │           └── package.json
│   │   └── react-dom@18.2.0/
│   │       └── node_modules/
│   │           └── react -> ../../react@18.2.0/node_modules/react
│   │
│   ├── react -> .orix/react@18.2.0/node_modules/react
│   └── react-dom -> .orix/react-dom@18.2.0/node_modules/react-dom
└── package.json
```

### 硬链接回退链

```
hard_link()           # 零复制，最快
  ↓ 失败时
copy()                # 跨文件系统或权限问题
  ↓ 失败时
记录警告，继续安装    # 绝不因复制而失败
```

---

## 五、Lockfile — 可重现 + frozen 快路径

### 问题

每次 install 都要重新解析完整依赖图，即使 lockfile 完全有效也不例外。

### 优化手段

- **frozen-lockfile**：lockfile 与 manifest 匹配时，直接从 lockfile 构建依赖图，**完全跳过 resolver 和 registry 请求**
- **增量更新**：`add`/`remove` 只变更受影响的部分，lockfile diff 计算后选择性安装
- **YAML 双结构**：
  - `importers`：每个项目的直接依赖，各自变更，合并友好
  - `packages`：共享的已解析包注册表，变更频率低，在 importers 间去重
- **原子写入**：写临时文件 → rename，防止崩溃时 lockfile 损坏

### frozen-lockfile 验证流程

```
lockfile + manifest
  → 验证 specifier 与 resolved version 匹配
  → 验证所有 manifest 依赖都在 lockfile 中有条目
  → 跳过 resolve + registry + fetch（直接复用 lockfile 中的 tarball URL）
  → 只做 store 存在性验证 + linker
```

---

## 总结：时间都花在哪了

### 传统串行流程耗时

```
Resolve(N包) → Fetch(N包) → Link(N包)
  N×RTT        N×RTT        N×硬链接
```

### pnpm 优化后流程耗时

```
Resolve(N/10×RTT + 缓存命中) → Fetch(N/10×RTT + tarball缓存) → Link(硬链接)
    ↓                               ↓                            ↓
  并发+去重                      tarball缓存                   严格隔离
  packument缓存                  完整性校验                   平台感知链接
```

### 核心是三件事

1. **网络 I/O 并发化**（resolve + fetch）
2. **磁盘 I/O 去重化**（CAS store 文件级共享）
3. **文件系统操作最小化**（硬链接 + 严格隔离，跳过全 hoist）
