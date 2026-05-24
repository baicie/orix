# Issue 001：Windows 在 link 完成后系统明显卡顿

**状态**：部分已修复（见下方「已落地」）  
**影响平台**：Windows（10/11 为主；跨盘、杀毒、机械盘更明显）  
**相关代码**：`crates/linker/src/linker.rs`、`crates/core/src/pipeline.rs`

---

## 修复优先级（共识）

```txt
P0：不要每次 unlink 整个 node_modules
P0：layout marker 有效时直接跳过 link
P1：package 已存在且 package.json 完整时跳过文件导入
P1：用 integrity 文件列表替代 WalkDir 扫 store
P1：同盘 hardlink，不同盘 fallback copy，并记住本次 fallback
P2：Windows 下减少 junction/symlink 数量
P2：批量生成 .bin shim，避免重复写
P2：给用户提示 Defender 排除 .orix/store 和项目 node_modules
```

---

## 已落地（当前分支）

| 优先级 | 项 | 实现 |
| --- | --- | --- |
| P0 | 不全量 `unlink` | `Linker::prune_stale_layout`：仅删 graph 外虚拟 store 包、过期顶层链接、重建 `.bin`；pipeline 改用 prune 替代 `unlink` |
| P0 | marker 有效跳过 link | 已有 `is_layout_valid` + `validate_layout`；fast path / 主路径均跳过 unlink+link |
| P1 | 完整包跳过导入 | `is_package_import_complete`（`package.json` + integrity 文件列表）；移除重复 `import_package_files` |
| P1 | store 不 WalkDir | link 导入侧已用 integrity；`validate_layout` 仅遍历 `.orix` 内 symlink，不再扫整棵 `node_modules` 硬链接树 |
| P1 | 同盘 hardlink | `same_volume` 预判；跨卷整包 `use_copy` |
| P2（部分） | `.bin` 校验 | `bin_shims_are_valid` 改为单层 `read_dir` |
| P2（部分） | Defender 提示 | `emit_windows_link_performance_hint`（Windows + 链接文件数 ≥500） |

---

## 现象

- `orix install` 在 **Link 阶段结束或整次 install 刚完成** 后，整机出现数秒到数分钟的卡顿。
- 同一项目在 Linux/macOS 上通常不明显。

---

## 结论（根因）

卡顿来自 **大量同步文件 IO + 安全软件扫描 +（历史上）每包重复 hardlink**，link 结束后 AV/索引仍在收尾。

历史主因（已通过 P1 导入修复缓解）：

- `import_package_files` 每包调用两次，且 `package.json` 存在判断路径错误（查 parent 而非 `pkg_dir`），导致几乎从不 skip。

仍待 P2：

- 每个依赖边一次 `mklink /J`（`cmd` 子进程），图大时 junction 数量多。
- `.bin` shim 仍按包写入，未批量去重。

环境因素：Defender 实时扫描 `node_modules` / store；跨盘导致 copy 倍增。

---

## 待办（按优先级）

### P2

1. **Junction 原生 API**：`CreateSymbolicLinkW` / `DeviceIoControl`，避免 per-link `cmd.exe`。
2. **批量 `.bin`**：收集全局 bin 表，一次写入 `.bin`，避免重复 shim。
3. **可选**：依赖边 symlink 合并/共享（需与 pnpm 布局语义对齐后再做）。

### 运维

- 文档化：store 与 `node_modules` 同 NTFS 卷、Defender 排除路径（CLI 已在重 link 后打 hint）。

---

## 临时规避（用户侧）

1. 保持 lockfile/graph 稳定以命中 layout 快路径（日志 `layout valid, skipping`）。
2. Defender 排除：全局 store（`orix store path`）+ 项目 `node_modules`。
3. 避免在 OneDrive/同步目录 install。

---

## 验收标准

- [x] 二次 install 同 graph：`hardlinked_files` ≈ 0（`link_graph_skips_file_import_when_package_already_complete`）。
- [x] graph 缩小仅 prune 过期包，不删整棵 `node_modules`（`prune_stale_layout_removes_only_obsolete_virtual_store_entries`）。
- [ ] Windows 100+ 包：link 后系统恢复时间明显短于修复前（需实机对比）。
- [ ] junction 创建不再依赖 `cmd /C mklink`（P2 完成后）。
