# Windows install/link 失败修复方案

## 背景

复现场景来自 Windows 11 / PowerShell：

```txt
Registry: https://registry.npmmirror.com/
✓ Resolved dependencies 1000/1000
✓ Fetched packages 1000/1000
✗ Linking dependencies
○ Writing lockfile
Error: 系统找不到指定的路径。 (os error 3)
Hint: Check file permissions and disk space.
⚠ An error occurred: link failed
```

对应 debug log 中最后的有效阶段：

```txt
fetched packages success=1000 failures=0
orix_linker::linker: linking package files pkg=@antfu/eslint-config@9.0.0 store=C:\Users\20555\.orix/store/v1\v1\packages\@antfu/eslint-config@9.0.0
orix_linker::linker: link summary pkg=@antfu/eslint-config@9.0.0 hardlink_ok=0 copy_ok=10 hardlink_fail=0 copy_fail=0
orix_core::pipeline: link failed error=系统找不到指定的路径。 (os error 3)
```

这说明：

- resolver 和 fetcher 已经跑完，失败点在 `crates/linker`。
- 第一个包文件已复制成功，失败更可能发生在创建依赖链接或 bin 链接时。
- 当前代码在 Windows 上先尝试 `symlink_dir`，权限不足时回退 `mklink /J`，而内部依赖链接传入的是相对 target。

## 问题 1：resolved 最多显示 1k

### 现状

代码里没有显式 `1000` 上限。显示数字来自 `crates/core/src/pipeline.rs` 的进度事件和最终 `graph.len()`。

但当前转发 resolver 进度时语义反了：

```rust
InstallEvent::ResolveProgress {
    done: event.discovered,
    total: event.resolved,
    package: Some(event.id.to_string()),
}
```

`ResolveProgressEvent` 的定义是：

- `discovered`：目前发现的包总数，运行中的估计值。
- `resolved`：目前已经解析完成的包数量。

因此 UI 应该显示 `resolved / discovered`，当前代码却显示 `discovered / resolved`。最后 `Resolved` 事件又把它改成 `graph.len()/graph.len()`，所以大项目上会看起来像卡到某个整数，例如 `1000/1000`。

### 修复

在 `crates/core/src/pipeline.rs` 调整事件转发：

```rust
InstallEvent::ResolveProgress {
    done: event.resolved,
    total: event.discovered,
    package: Some(event.id.to_string()),
}
```

同时建议把 UI 文案语义改成“已解析 / 已发现”，避免用户误以为 total 是最终总数：

```txt
Resolving dependencies 842/1037 discovered
```

最终解析完成后仍由 `InstallEvent::Resolved { total: graph.len(), ... }` 输出稳定结果：

```txt
✓ Resolved dependencies 1037/1037
```

### 测试

- 在 `crates/cli/src/reporter/state.rs` 增加单元测试：`ResolveProgress { done: 1001, total: 1200 }` 必须保留超过 1000 的值。
- 在 `crates/core/src/pipeline.rs` 或 reporter 层加回归测试：resolver 事件 `discovered=1200, resolved=1001` 转成 install 事件后应为 `done=1001,total=1200`。

## 问题 2：终端输出重复且闪屏

### 现状

交互式 reporter 每个事件都会尝试重绘整帧。`InteractiveReporter::render` 中只要有阶段处于 Running，就绕过节流：

```rust
let any_running = ...;

if !force
    && !any_running
    && now.duration_since(self.last_render_at) < self.min_render_interval
{
    return Ok(());
}
```

fetch / resolve 会产生大量事件，Windows 终端里频繁执行“上移光标、清行、重写整帧”很容易看到闪屏；如果终端不完整支持 cursor movement，或者行宽估算与真实显示宽度不一致，就会留下重复的：

```txt
orix install
----------------------------------------
```

另外 `PlainReporter` 在非 TTY / `--no-progress` 下会为每个 progress event 输出一行，日志非常嘈杂。

### 修复

1. 不要因为 `any_running` 绕过节流。只有这些事件强制刷新：
   - `Started`
   - `PhaseStarted`
   - `PhaseFinished`
   - `Lockfile`
   - `Finished`
   - `Failed`

2. 普通 `ResolveProgress` / `FetchProgress` 按固定频率刷新，例如 15-30 FPS：

```rust
if !force && now.duration_since(self.last_render_at) < self.min_render_interval {
    return Ok(());
}
```

3. `PlainReporter` 聚合进度，只输出阶段开始、每 N 个包、阶段结束和失败。建议规则：
   - resolve/fetch 每 100 个包或每 1 秒输出一次。
   - 最后一条一定输出。
   - `PackageFetched` 默认不逐包输出，除非 `ORIX_LOG=debug`。

4. `LiveTerminal` 清理策略改成更保守的“从当前帧顶部清到屏幕底部”：

```rust
MoveUp(last_rows)
MoveToColumn(0)
Clear(ClearType::FromCursorDown)
```

避免逐行清理时行数估算偏差导致残影。

### 测试

- `InteractiveReporter`：连续发送 1000 个 `FetchProgress`，断言实际 render 次数远小于事件数。
- `LiveTerminal`：上一帧比下一帧长时，输出必须包含 `ClearType::FromCursorDown`。
- `PlainReporter`：1000 个 fetch progress 事件不应产生 1000 行输出。

## 问题 3：Windows link 阶段 `os error 3`

### 根因

`crates/linker/src/linker.rs` 中内部依赖链接使用相对 target：

```rust
let symlink_target = relative_path(parent, &target);
Self::create_symlink(&symlink_target, &symlink_path)?;
```

Windows 实现：

```rust
match std::os::windows::fs::symlink_dir(target, link) {
    Ok(_) => Ok(()),
    Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
        Self::create_junction(target, link)
    }
    Err(e) => Err(e),
}
```

在未开启 Developer Mode 或无 symlink 权限时会进入 junction fallback。`mklink /J` 对 target 要求更严格，实际需要绝对目录路径；当前把类似下面的相对路径传给 `mklink /J`：

```txt
..\..\..\dep@1.0.0\node_modules\dep
```

这会导致：

```txt
系统找不到指定的路径。 (os error 3)
```

### 修复

把链接 API 拆成目录链接和文件链接，并让 Windows junction fallback 永远使用绝对 target。

建议接口：

```rust
fn create_dir_link(target: &Path, link: &Path) -> io::Result<()>;
fn create_file_link(target: &Path, link: &Path) -> io::Result<()>;
```

目录链接实现：

```rust
#[cfg(windows)]
fn create_dir_link(target: &Path, link: &Path) -> io::Result<()> {
    if let Ok(()) = std::os::windows::fs::symlink_dir(target, link) {
        return Ok(());
    }

    let absolute_target = absolutize_link_target(target, link)?;
    create_junction(&absolute_target, link)
}

#[cfg(windows)]
fn absolutize_link_target(target: &Path, link: &Path) -> io::Result<PathBuf> {
    if target.is_absolute() {
        return Ok(target.to_path_buf());
    }

    let parent = link.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "link path has no parent")
    })?;

    Ok(parent.join(target).canonicalize()?)
}
```

注意：`canonicalize()` 要求 target 已存在。linker 当前先创建物理包目录，再创建依赖链接，顺序满足这个条件。如果后续并行化 linker，必须保持 target 目录先存在。

`create_junction` 也要输出更明确的错误：

```rust
Err(io::Error::new(
    io::ErrorKind::Other,
    format!(
        "failed to create junction {} -> {}: {}{}",
        link.display(),
        absolute_target.display(),
        String::from_utf8_lossy(&o.stderr),
        String::from_utf8_lossy(&o.stdout),
    ),
))
```

bin 链接不能继续复用目录链接。当前 `.bin/<cmd>` 也调用 `create_symlink`，但它指向文件：

```rust
Self::create_symlink(&relative_target, &global_bin_link)?;
```

应改为：

- Windows：优先 hardlink/copy bin 文件，必要时生成 `.cmd` shim。
- Unix：使用 `symlink` 文件。

最小修复可以先做：

```rust
#[cfg(windows)]
fn create_file_link(target: &Path, link: &Path) -> io::Result<()> {
    let absolute_target = absolutize_link_target(target, link)?;
    match fs::hard_link(&absolute_target, link) {
        Ok(_) => Ok(()),
        Err(_) => fs::copy(&absolute_target, link).map(|_| ()),
    }
}
```

### 额外修复：避免 `C:\Users\...\store\v1\v1`

日志中 store 路径为：

```txt
C:\Users\20555\.orix/store/v1\v1\packages\...
```

说明配置层传给 `Store::open` 的路径已经带了 `v1`，而 `Store::open` 内部又追加了一次 `STORE_VERSION`。

修复方向二选一：

- 推荐：`Config.store_dir` 永远表示 store base dir，例如 `~/.orix/store`；只允许 `Store::open` 追加 `v1`。
- 或者：`Store::open` 检测末尾已经是 `v1` 时不重复追加。

推荐第一种，因为 crate 边界更清楚。

### 测试

新增 Windows 专用回归测试，模拟“symlink 不可用，只能走 junction”的路径转换：

```rust
#[cfg(windows)]
#[test]
fn windows_junction_fallback_absolutizes_relative_target() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("node_modules/.orix/dep@1.0.0/node_modules/dep");
    let link = temp.path().join("node_modules/.orix/parent@1.0.0/node_modules/parent/node_modules/dep");
    fs::create_dir_all(&target)?;
    fs::create_dir_all(link.parent().unwrap())?;

    let relative = relative_path(link.parent().unwrap(), &target);
    let absolute = absolutize_link_target(&relative, &link)?;

    assert!(absolute.is_absolute());
    assert_eq!(absolute, target.canonicalize()?);
    Ok(())
}
```

再加一个端到端 linker 测试：

- 构建 parent -> child 的依赖图。
- 强制使用 junction fallback。
- 验证 `node_modules/.orix/parent@.../node_modules/parent/node_modules/child` 存在且可解析。

为了可测，建议把 Windows link 后端抽象成 trait 或枚举：

```rust
enum LinkBackend {
    Auto,
    ForceJunction,
    ForceCopy,
}
```

生产默认 `Auto`，测试使用 `ForceJunction`。

## 实施顺序

1. 修 linker Windows 目录链接：相对 target fallback 到 junction 前转绝对路径。
2. 修 bin 文件链接：目录链接和文件链接拆开。
3. 加 Windows linker 回归测试，至少覆盖相对 target -> junction。
4. 修 resolver 进度字段顺序，并加 >1000 的 UI 状态测试。
5. 修 reporter 刷新节流，降低闪屏和重复输出。
6. 修 store `v1\v1` 路径语义。
7. 跑完整检查：

```powershell
cargo fmt
cargo test -p orix-linker
cargo test -p orix-cli reporter
cargo test -p orix-core
make check
```

## 临时绕过方案

在正式修复前，用户侧可以先：

```powershell
orix install --no-progress --debug
```

这只能降低终端闪屏并保留日志，不能修复 link 失败。

如果是 symlink 权限触发的 junction fallback，可临时开启 Windows Developer Mode 或用管理员终端运行，以避免进入有问题的 junction 分支。但这不是最终方案，orix 应该在普通 Windows 用户权限下完成安装。

## 验收标准

- Windows 11 普通 PowerShell 中，未开启 Developer Mode 也能完成 `orix install`。
- 大项目 resolve/fetch 显示真实数量，超过 1000 时不截断。
- 交互式进度不持续刷出重复 header。
- `--no-progress` / 非 TTY 输出不会逐包刷屏。
- `node_modules` 中 direct dependency 可解析，transitive dependency 只在对应包内部可解析。
- `make check` 通过。
