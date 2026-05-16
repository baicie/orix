# Linker 设计 — node_modules 目录结构生成

## 概述

`crates/linker` 负责构建 `node_modules/` 目录结构。它接收已解析的依赖图，查询 CAS store 中包文件的位置，并创建相应的硬链接和符号链接，生成可被 Node.js 模块解析的目录树。

这是安装器中文件系统操作最密集的部分。核心不变式是**严格隔离**：包只能 require 其声明的依赖，不能访问工作区中的其他任何东西。

## 目标目录结构

```
project-root/
├── node_modules/
│   ├── .pnpm/                           # 物理包文件
│   │   ├── react@18.2.0/
│   │   │   └── node_modules/
│   │   │       └── react/               # 包文件（从 store 硬链接）
│   │   │           ├── index.js
│   │   │           └── package.json
│   │   ├── react-dom@18.2.0/
│   │   │   └── node_modules/
│   │   │       ├── react-dom/
│   │   │       └── react -> ../../react@18.2.0/node_modules/react
│   │   └── loose-envify@1.4.0/
│   │       └── node_modules/
│   │           └── loose-envify/
│   │
│   ├── react -> .pnpm/react@18.2.0/node_modules/react
│   ├── react-dom -> .pnpm/react-dom@18.2.0/node_modules/react-dom
│   └── .pnpmfile.cjs                    #（未来：支持 pnpmfile）
│
└── package.json
```

## 符号链接约定

pnpm 布局使用符号链接从物理存储构建虚拟模块树：

| 类型 | 源 | 目标 |
|------|-----|------|
| 项目依赖 | `node_modules/<pkg>` | `.pnpm/<pkg>@<ver>/node_modules/<pkg>` |
| 平台内依赖 | `.pnpm/<pkg>@<ver>/node_modules/<dep>` | `../../<dep>@<ver>/node_modules/<dep>` |

这意味着任何包中的 `require('react')` 解析到 `node_modules/react`，它符号链接到 `.pnpm/react@18.2.0/node_modules/react`。

## 布局算法

给定 `DependencyGraph`，linker 通过两遍生成符号链接：

### 第一遍：物理布局（.pnpm/）

对图中的每个 `PackageNode`：

```
1. 创建 .pnpm/<name>@<version>/node_modules/<name>/
2. 将 store/packages/<name>@<version>/files/ 中的所有文件硬链接（或复制）到此目录
3. 对该包的每个声明依赖 dep：
   a. 在图中定位 dep 的 ResolvedPackage
   b. 在 .pnpm/<name>@<version>/node_modules/<dep_name> 创建符号链接
      指向 ../../<dep_name>@<dep_version>/node_modules/<dep_name>
```

**平台内符号链接的相对路径公式：**

```rust
fn relative_symlink_target(dep: &PackageId, parent: &PackageId, root: &Path) -> PathBuf {
    let dep_physical = root.join(".pnpm").join(format!("{}@{}", dep.name, dep.version))
        .join("node_modules").join(&dep.name);
    let parent_physical = root.join(".pnpm").join(format!("{}@{}", parent.name, parent.version))
        .join("node_modules");
    rel_path(&dep_physical, &parent_physical)
}
```

### 第二遍：虚拟布局（项目根目录）

对项目中 `package.json` 的每个**直接**（非传递）依赖：

```
node_modules/<name> -> .pnpm/<name>@<version>/node_modules/<name>
```

传递依赖**不会**符号链接到根目录——它们只能通过显式依赖链访问（严格隔离）。

## 硬链接策略（平台感知）

```rust
fn link_file(source: &Path, target: &Path) -> LinkResult {
    // 先尝试硬链接（最佳性能，零复制）
    match std::fs::hard_link(source, target) {
        Ok(_) => LinkResult::HardLinked,
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices
             || e.kind() == io::ErrorKind::PermissionDenied => {
            // 跨设备或权限问题 —— 回退到复制
            match std::fs::copy(source, target) {
                Ok(_) => LinkResult::Copied,
                Err(e) => LinkResult::Failed(e),
            }
        }
        Err(e) => LinkResult::Failed(e),
    }
}
```

## Windows 特殊处理

| 平台 | 目录链接 | 文件链接 |
|------|----------|----------|
| Unix | `symlink()` | `link()`（硬链接） |
| Windows | `junction()`（优先）或 `symlink()` | `link()`（硬链接） |

**目录回退到 Junction：**

```rust
#[cfg(windows)]
fn create_dir_symlink(source: &Path, target: &Path) -> io::Result<()> {
    // 先尝试符号链接（需要开发者模式）
    if let Ok(_) = std::fs::symlink_dir(source, target) {
        return Ok(());
    }
    // 回退到通过 `cmd /c mklink /J` 创建 junction
    let output = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J", &target.display().to_string(),
               &source.display().to_string()])
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
```

## 符号链接图验证

创建所有符号链接后，运行验证阶段：

```rust
pub fn validate_layout(project_root: &Path) -> Result<LayoutReport> {
    let mut broken = Vec::new();
    let mut warnings = Vec::new();

    // 遍历 node_modules/（跟随符号链接）
    for entry in walkdir::WalkDir::new(project_root.join("node_modules"))
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_symlink() {
            let target = entry.read_link().unwrap();
            if !target.exists() {
                broken.push(entry.path().to_path_buf());
            }
        }
    }

    // 验证每个直接依赖是否可解析
    let manifest = Manifest::read(project_root.join("package.json"))?;
    for dep in manifest.dependencies.keys() {
        let path = project_root.join("node_modules").join(dep);
        if !path.exists() {
            broken.push(path);
        }
    }

    Ok(LayoutReport { broken, warnings })
}
```

## 核心 API

```rust
pub struct Linker {
    store: Store,
    project_root: PathBuf,
    node_modules: PathBuf,
}

impl Linker {
    pub fn new(store: Store, project_root: PathBuf) -> Self;

    /// 入口函数：从依赖图构建完整的 node_modules 布局
    pub async fn link_graph(
        &self,
        graph: &DependencyGraph,
        direct_deps: &HashMap<String, VersionConstraint>,
    ) -> Result<LinkReport>;

    /// 删除此项目生成的所有链接和 .pnpm/ 内容
    pub fn unlink(&self) -> Result<()>;

    /// 清理不再被引用的 .pnpm/ 条目
    pub fn prune_stale(&self, referenced: &HashSet<PackageId>) -> Result<PruneReport>;
}
```

## 链接报告

linker 每次安装后返回结构化报告：

```rust
pub struct LinkReport {
    pub hardlinked_files: u64,
    pub copied_files: u64,
    pub symlinks_created: u64,
    pub bytes_saved: u64,      // 受益于硬链接而未复制的字节数
    pub duration_ms: u64,
}
```

## 卸载/更新时的清理

运行 `rpnpm remove <pkg>` 时：

1. 删除 `node_modules/<pkg>` 符号链接
2. 删除 `node_modules/.pnpm/<pkg>@<ver>/` 目录树
3. 更新 `rpnpm-lock.yaml`
4. 如果包不再被使用，可选触发 store 清理

## 与 Store 的交互

linker **不**管理 store 内容——它只从 store 读取。store 是包文件位置的事实来源。linker 假设包存在于 store 中（安装管道在调用 linker 之前确保这一点）。
