# Issues 记录

本目录记录 orix 已知问题、根因结论与修复方案，供排障与排期使用。

| ID | 标题 | 状态 |
| --- | --- | --- |
| [001](./001-windows-link-system-lag.md) | Windows 在 link 完成后系统明显卡顿 | P0/P1 已修复，P2 待做 |
| [002](./002-lockfile-unchanged-after-version-bump.md) | package.json 升级依赖版本后 lockfile 无变化 | 已修复 |
| [003](./003-windows-junction-bare-drive-node-eisdir.md) | Windows junction 指向 `D:` 导致 `oi dev` / Node `EISDIR` | 已修复 |

相关设计：

- [Linker](../design/linker.md)
- [Lockfile](../design/lockfile.md)
- [Install 性能优化](../design/install-performance-optimization.md)
