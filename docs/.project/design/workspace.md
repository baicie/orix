# Workspace 设计 — Monorepo 包管理

## 概述

`crates/workspace` 处理 monorepo 布局。它在工作区内发现包，解析 `workspace:*` 协议依赖，并协调多个包的安装管道，使本地包相互链接而非从 registry 下载。

## 工作区发现

工作区根目录包含一个 `pnpm-workspace.yaml`：

```yaml
packages:
  - 'packages/*'
  - 'apps/*'
  - '!packages/**/node_modules'
```

```rust
/// 已发现的工作区包
#[derive(Clone, Debug)]
pub struct WorkspacePackage {
    /// 相对于工作区根目录的路径
    pub relative_path: PathBuf,
    /// 包的绝对路径
    pub abs_path: PathBuf,
    /// 解析后的 package.json
    pub manifest: Manifest,
}

/// 完整的工作区
#[derive(Clone, Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub packages: Vec<WorkspacePackage>,
    pub lockfile_path: PathBuf,
}
```

### 发现算法

```
1. 从项目根目录读取 pnpm-workspace.yaml
2. 解析每个 glob 模式
3. 遍历匹配 glob 的文件系统
4. 对每个匹配项，检查是否存在 package.json
5. 验证没有包出现在两个 glob 匹配中
6. 返回排序后的 WorkspacePackage 列表
```

```rust
impl Workspace {
    pub fn discover(root: PathBuf) -> Result<Self> {
        let manifest_path = root.join("pnpm-workspace.yaml");
        let source = std::fs::read_to_string(&manifest_path)
            .with_context(|| "failed to read pnpm-workspace.yaml")?;

        let workspace_file: WorkspaceFile = serde_yaml::from_str(&source)
            .with_context(|| "failed to parse pnpm-workspace.yaml")?;

        let packages = Self::find_packages(&root, &workspace_file.packages)?;
        Ok(Self { root, packages, lockfile_path: root.join("orix-lock.yaml") })
    }

    fn find_packages(root: &Path, patterns: &[String]) -> Result<Vec<WorkspacePackage>> {
        let mut packages = Vec::new();
        let mut seen_names = HashSet::new();

        for pattern in patterns {
            for entry in glob::glob(&root.join(pattern).display().to_string())? {
                let pkg_path = entry?;
                let manifest_path = pkg_path.join("package.json");

                if !manifest_path.exists() {
                    continue;
                }

                let manifest = Manifest::read(&manifest_path)?;
                let name = manifest.name.clone().unwrap_or_default();

                if !seen_names.insert((name.clone(), pkg_path.clone())) {
                    anyhow::bail!(
                        "package '{}' at '{}' matches multiple workspace globs",
                        name, pkg_path.display()
                    );
                }

                packages.push(WorkspacePackage {
                    relative_path: pkg_path.strip_prefix(root).unwrap().to_path_buf(),
                    abs_path: pkg_path,
                    manifest,
                });
            }
        }

        packages.sort_by_key(|p| p.relative_path.clone());
        Ok(packages)
    }
}
```

## Workspace 协议解析

### `workspace:*` 协议

当工作区中的包声明对另一个本地包的依赖时：

```json
{
  "name": "@mycompany/ui",
  "dependencies": {
    "@mycompany/utils": "workspace:*"
  }
}
```

`workspace:*` 协议的意思是"使用此包的本地版本"。resolver 必须：

1. 在工作区中找到匹配的包
2. 跳过该依赖的 registry 查询
3. 将本地包的路径作为解析目标返回

### 协议变体

| 协议 | 行为 |
|------|------|
| `workspace:*` | 使用本地 package.json 中的版本 |
| `workspace:^` | 使用匹配本地包 `^` 的版本 |
| `workspace:~` | 使用匹配本地包 `~` 的版本 |
| `workspace:>=1.0.0` | 必须满足相对于本地版本的 semver 约束 |
| `workspace:file:../utils` | 从本地文件路径链接（不在 workspace globs 中） |

### Resolver 集成

```rust
impl Workspace {
    /// 将 workspace 协议依赖解析为本地 PackageId
    pub fn resolve_workspace_dep(
        &self,
        spec: &WorkspaceSpec,
    ) -> Option<ResolvedWorkspaceDep> {
        match spec {
            WorkspaceSpec::Star(name) => {
                self.packages.iter().find(|p| p.manifest.name.as_ref() == name)
            }
            WorkspaceSpec::File(path) => {
                let abs = self.root.join(path);
                let manifest = Manifest::read(&abs.join("package.json")).ok()?;
                Some(WorkspacePackage { relative_path: path.clone(), abs_path: abs, manifest })
            }
            _ => None,
        }.map(|pkg| ResolvedWorkspaceDep {
            id: PackageId { name: pkg.manifest.name.clone(), version: pkg.manifest.version.clone() },
            local_path: pkg.abs_path.clone(),
        })
    }
}
```

### Lockfile 中的 Workspace 依赖表示

lockfile 中的 workspace 依赖：

```yaml
packages:
  /@mycompany/utils@1.0.0:
    local: ../packages/utils
    resolution:
      type: local
      path: packages/utils
```

这告诉 linker 将 `node_modules/@mycompany/utils` 符号链接到本地包目录，而非创建 `.pnpm/` 条目。

## 工作区安装行为

### 按包安装

在 workspace 子目录内运行 `orix install` 时：

```
cd packages/my-lib
orix install
```

1. 从最近的 `pnpm-workspace.yaml` 祖先加载工作区
2. 首先解析所有 workspace 依赖（它们总是本地满足）
3. 其余依赖按常规从 registry 解析
4. 每个包获得自己的 `node_modules/`，只包含自己的直接依赖
5. Workspace 依赖被符号链接，而非安装到每个包的 node_modules

### 根目录安装

从工作区根目录运行 `orix install` 时：

1. 扫描所有 workspace 包
2. 收集整个工作区的所有依赖
3. 去重——如果多个包使用 `react@18.2.0`，只安装一次
4. 在工作区根目录生成一个共享的 `orix-lock.yaml`
5. 在工作区根目录构建一个共享的 `.pnpm/` store
6. 每个包的 `node_modules/` 符号链接到共享的 `.pnpm/`

### 带工作区的 Lockfile 结构

```yaml
# 工作区根目录的 orix-lock.yaml
lockfileVersion: 1

importers:
  .:
    dependencies:
      '@mycompany/utils': 'workspace:*'
  packages/my-lib:
    dependencies:
      react: '^18.2.0'
      '@mycompany/utils': 'workspace:*'
  packages/my-app:
    dependencies:
      '@mycompany/ui': 'workspace:*'
      react: '^18.2.0'

packages:
  /@mycompany/utils@1.0.0:
    local: packages/utils
    resolution:
      type: local
      path: packages/utils
  /react@18.2.0:
    ... (registry 条目)
```

## 工作区本地链接

linker 以不同方式处理 workspace 包：

```rust
fn link_workspace_dep(pkg: &WorkspacePackage, project_node_modules: &Path) -> Result<()> {
    let target = project_node_modules.join(pkg.manifest.name.as_ref().unwrap());
    let source = pkg.abs_path.clone();

    // 对于 workspace 依赖：直接符号链接到包源码
    // 不需要 .pnpm/ 条目 —— 包本身就是源码
    if cfg!(windows) {
        create_junction(&source, &target)?;
    } else {
        std::os::unix::fs::symlink(&source, &target)?;
    }
    Ok(())
}
```

## 约束和边缘情况

- **重复包名**：两个 workspace 包具有相同的 `name` 字段会导致错误
- **循环 workspace 依赖**：在解析图中检测为循环，报告错误
- **globs 外的 workspace 依赖**：`workspace:file:../sibling` 允许链接不在 globs 中的包
- **Workspace 依赖版本不匹配**：如果 `workspace:*` 解析到的版本不满足父包的 semver 范围，给出警告但不阻止
