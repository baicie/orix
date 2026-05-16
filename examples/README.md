# Orix 示例

本目录包含用于测试 orix 各功能模块的小型 JavaScript 项目。

从仓库根目录运行示例：

```bash
cargo run -p orix-cli -- --dir examples/basic-install install
```

或者进入示例目录内运行：

```bash
cd examples/basic-install
cargo run -p orix-cli -- install
```

## 示例列表

| 示例 | 重点 |
| --- | --- |
| `basic-install` | 常规生产依赖和 CommonJS 入口。 |
| `dev-dependencies` | `devDependencies`、ESM 及脚本元数据解析。 |
| `optional-dependencies` | 可选依赖处理，包括平台相关包。 |
| `bin-package` | `bin` 入口和可执行包元数据。 |
| `workspace-monorepo` | `pnpm-workspace.yaml` 发现及 `workspace:*` 本地包。 |

这些项目刻意保持最小化，以便在开发 resolver、fetcher、lockfile、linker 和 workspace 功能时作为手动测试用例使用。
