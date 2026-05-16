# Store 设计 — 内容寻址包缓存

## 概述

`crates/store` 实现 CAS（Content-Addressable Store，内容寻址存储）层。所有下载的包文件都存储在全局 store 目录下，按内容哈希去重。这是节省磁盘空间的主要机制——多个项目共享同一包版本时，磁盘上只有一份物理副本。

## Store 目录布局

```
~/.rpnpm/store/v1/
├── files/
│   └── sha256/
│       └── <前缀>/<哈希>           # 去重的文件内容
├── packages/
│   └── <包名>@<版本>/
│       ├── integrity.json             # 包元数据 & 文件索引
│       └── files/                     # 解压后的包文件
│           ├── index.js
│           ├── package.json
│           └── ...
```

### 内容可寻址文件（`files/`）

tarball 内的每个文件都按其 SHA-256 内容哈希单独存储：

```
文件内容 → sha256(内容) → store/files/sha256/<前2位>/<其余>/<哈希>
```

示例：

```
store/files/sha256/ab/cd/abc123...def456
```

顶级目录只使用前 2 个十六进制字符作为分片机制，避免在单个目录下放置数百万个文件（某些文件系统上的真实问题）。

### 包条目（`packages/`）

每个已安装的包都有一个以 `name@version` 为键的目录：

```
store/packages/lodash@4.17.21/
```

## 核心数据结构

### `integrity.json`

作为 `integrity.json` 存储在每个包条目内：

```json
{
  "name": "react",
  "version": "18.2.0",
  "integrity": "sha512-...",
  "files": {
    "index.js":       "sha256:abc123...",
    "package.json":   "sha256:def456...",
    "README.md":      "sha256:ghi789..."
  },
  "depnodes": ["lodash@4.17.21", "scheduler@0.23.0"]
}
```

- **`files`**：将相对文件路径映射到内容哈希。用于增量更新和去重。
- **`depnodes`**：此包传递依赖的包键列表。linker 用它来知道要创建哪些依赖的符号链接。

## 核心 API

```rust
// 打开或创建全局 store
pub fn open(store_root: PathBuf) -> Result<Store>;

// 将解压后的包目录导入 store
// 返回新添加的文件集合（尚不存在的文件）
pub async fn import_package(
    &self,
    pkg_id: &PackageId,
    source_dir: PathBuf,
) -> Result<HashSet<PathBuf>>;

// 检查包是否已完全存在于 store 中
pub fn contains(&self, pkg_id: &PackageId) -> bool;

// 获取包在 store 中的文件路径
pub fn package_path(&self, pkg_id: &PackageId) -> PathBuf;

// 获取文件的内容可寻址路径
pub fn file_path(&self, hash: &str) -> PathBuf;

// 读取包的完整性元数据
pub fn get_integrity(&self, pkg_id: &PackageId) -> Result<IntegrityMeta>;

// 列出 store 中当前所有包
pub fn list_packages(&self) -> Result<Vec<PackageId>>;

// 清理未引用的包（支持 --dry-run）
pub async fn prune(&self, referenced: &HashSet<PackageId>, dry_run: bool) -> Result<PruneReport>;
```

## 导入流程

导入包 tarball 时：

```
1. 将 tarball 解压到临时目录
2. 遍历临时目录中的所有文件
3. 对每个文件：
   a. 计算 sha256(内容)
   b. 检查 store/files/sha256/xx/yy/<哈希> 是否存在
   c. 不存在则 → 复制到 store（去重）
4. 写入 store/packages/<name>@<version>/integrity.json
5. 将所有文件硬链接（或复制失败时）到包条目目录
6. 删除临时目录
```

## 文件去重算法

```rust
fn import_file(&self, relative_path: &Path, source: &Path) -> Result<FileImportResult> {
    let content = std::fs::read(source)?;
    let hash = sha256(&content);

    let content_path = self.file_path(&hash);
    let is_new_file = !content_path.exists();

    if is_new_file {
        // 确保父目录存在（分片）
        std::fs::create_dir_all(content_path.parent().unwrap())?;
        std::fs::copy(source, &content_path)?;
    }

    Ok(FileImportResult { hash, is_new_file, content_path })
}
```

**去重键**是文件内容哈希，而非文件路径。这意味着：

- 如果两个包的 `package.json` 内容相同，它们共享一份物理副本
- 即使一个文件略有变化，也会得到新的哈希条目
- store 总大小 = 所有已安装包中唯一文件内容的总和

## 硬链接策略

导入时，包条目目录（`store/packages/<pkg>@<ver>/files/`）中的文件从内容可寻址文件硬链接过来：

```
store/files/sha256/ab/.../abc123 ──hardlink──► store/packages/react@18.2.0/files/index.js
```

**回退链：**

1. `link()`（硬链接）——最快，零复制
2. `copy()`——如果硬链接失败（跨文件系统、权限）
3. 记录警告并继续——绝不因复制而失败安装

**Windows junction 优先级：**

在 Windows 上，目录符号链接需要开发者模式/管理员权限。linker crate（不是 store）优先使用 **junctions** 处理目录，硬链接处理文件。

## 并发安全

store 支持多项目并发安装：

- 文件导入使用幂等语义的 `create_dir_all`
- `integrity.json` 写入是原子的（写入临时文件 → 重命名）
- store 根目录上的读写锁防止同时写入同一个包

## 清理（Pruning）

可以用 `rpnpm store prune` 删除未使用的包：

```
1. 构建所有 rpnpm-lock.yaml 中引用的 PackageId 集合
2. 遍历 store/packages/
3. 删除不在引用集合中的条目
4. 遍历 store/files/
5. 删除不被任何剩余 integrity.json 引用的文件条目
6. 报告回收的字节数
```

dry-run 模式（`--dry-run`）报告将要删除的内容而不实际删除。

## 错误处理

| 错误 | 原因 | 恢复方式 |
|------|------|----------|
| `StoreReadOnly` | store 目录不可写 | 警告，回退到复制 |
| `IntegrityNotFound` | 包条目缺失 | 重新获取包 |
| `HashMismatch` | 完整性检查失败 | 删除并重新导入 |
| `StoreCorrupted` | integrity.json 不可读 | 删除并重新导入 |

所有错误都是 `thiserror` 枚举，向上传播到安装管道，由安装管道决定是否重试或中止。
