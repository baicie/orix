# 003 — Windows junction 指向裸盘符导致 Node `EISDIR: lstat 'D:'`

## 现象

- `orix install` 成功，显示 `Lockfile unchanged`、`Linked dependencies`（可能跳过 relink）。
- `orix run dev` / `oi dev` 或依赖 `postinstall` 失败：

```text
Error: EISDIR: illegal operation on a directory, lstat 'D:'
    at Object.realpathSync (node:fs:2729:25)
```

- 多见于项目在 `D:\...`、Node 较新版本（如 v26）。

## 根因

1. **旧版 link** 在 Windows 上对目录 junction 使用 `relative_path` 目标；在跨盘符或特定相对深度下，junction 可能解析为裸盘符 `D:`（盘符根目录仍存在，故校验通过）。
2. **`validate_layout`** 只检查 `resolved.exists()`，未拒绝「指向盘符根」的 junction。
3. **layout 快路径**：`metadata.json` 中 graph hash 未变时跳过 `unlink+link`，损坏 junction 一直保留。
4. **`create_dir_link`** 在链接已存在时直接返回，即使目标错误也不会重建。

Node 通过 `.bin` shim 或 `require` 解析模块时，`realpathSync` 落到 `D:`，触发 `EISDIR`。

## 修复（orix）

| 项 | 说明 |
| --- | --- |
| junction 目标 | Windows 上 `create_dir_link` 始终规范化为绝对路径再建 junction |
| `validate_layout` | 拒绝 `D:` / 仅盘符根的目标 |
| `dir_link_needs_repair` | relink 时删除并重建错误 junction |
| `link_protocol_version` | marker 版本 bump，使旧 layout 失效并触发 relink |
| PATH | `sanitize_path_env` 过滤 PATH 中的裸 `D:` 段（脚本执行） |

## 用户侧恢复步骤

1. 使用包含上述修复的 orix 二进制（本地 `cargo build --release` 或更新 `oi` 别名指向的二进制）。
2. 在项目中重新安装（会因 protocol 版本不匹配而 relink）：

```powershell
cd D:\workspace\bonree-code\bonree
oi i
```

3. 若仍失败，删除 `node_modules` 后重装：

```powershell
Remove-Item -Recurse -Force node_modules
oi i
```

4. 再执行 `oi dev`。

## 相关

- [Linker 设计](../design/linker.md)
- [Issue 001 — Windows link 卡顿](./001-windows-link-system-lag.md)
