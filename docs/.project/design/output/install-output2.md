# Install Output 设计（二次优化）

本文在 `install-output.md` 的基础上继续收敛默认输出。核心调整是：

**默认完成态只保留最终摘要，不把运行中的过程态也留在终端历史里。**

也就是说，运行中可以出现：

```txt
● Fetching packages 3/6
```

但安装结束后，最终留在终端里的内容应该刷新成：

```txt
✓ Fetched packages 6/6
```

不要同时留下“正在 fetch”与“fetch 完成摘要”两套表达。

## 输出模式

orix install 输出分为四种模式：

| 模式 | 目标 | 行为 |
| --- | --- | --- |
| 默认 TTY | 给人看的交互输出 | 运行中原地刷新，完成后只留下最终摘要 |
| CI / non-TTY | 给日志系统看的稳定输出 | 逐行追加，不使用 spinner，不依赖 ANSI 原地刷新 |
| verbose | 给调试看的详细输出 | 展示包级 resolve / fetch / link 细节 |
| json | 给工具集成看的事件流 | 输出 NDJSON 事件，不混入人类 UI |

默认模式追求信息密度，不展示完整包列表，不混入 tracing `INFO` 日志。

## 默认成功输出

推荐默认完成态：

```txt
orix install
----------------------------------------

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✓ Fetched packages 6/6
✓ Linked dependencies
✓ Lockfile unchanged

Done in 0.21s
```

如果有缓存命中，可以在 fetch 行下方追加一行轻量统计：

```txt
orix install
----------------------------------------

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✓ Fetched packages 6/6
  4 cached, 2 downloaded
✓ Linked dependencies
✓ Lockfile unchanged

Done in 0.21s
```

### 完成态规则

- 完成态里只保留 `✓` / `✗` 这种结果符号。
- 不保留 `● Resolving dependencies`、`● Fetching packages 3/6` 这样的过程态。
- 不重复打印 `orix install` 标题。
- 不默认打印每个包的完整列表。
- 不默认输出 timestamp、crate path、`duration_ms` 字段名或 tracing target。

## TTY 安装中输出

TTY 模式下维护一块动态区域，运行中原地刷新。

示例：

```txt
orix install
----------------------------------------

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
● Fetching packages 3/6
  ✓ is-buffer
  ✓ is-even
  ✓ is-number
```

完成后，同一块动态区域刷新为最终摘要：

```txt
orix install
----------------------------------------

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✓ Fetched packages 6/6
✓ Linked dependencies
✓ Lockfile unchanged

Done in 0.21s
```

### 动态区域规则

- 当前阶段用 `●` 表示进行中。
- 已完成阶段用 `✓` 表示成功。
- 失败阶段用 `✗` 表示失败。
- 少量包可以展示最近完成的包名。
- 大量包只展示最近若干个，不展开全部依赖。
- 动态区域最终必须被完成态替换。

## CI / non-TTY 输出

CI 模式不能依赖原地刷新，所以采用 append-only 日志：

```txt
orix install
packages: 2 direct, 6 total
registry: https://registry.npmmirror.com/

[1/4] resolving dependencies
[2/4] fetching packages 6/6
[3/4] linking dependencies
[4/4] writing lockfile

done in 0.21s
```

CI 输出规则：

- 不使用 spinner。
- 不输出 ANSI 动画。
- 每个阶段最多输出一行主要进度。
- 失败时直接输出失败阶段、失败对象和原因。

## verbose 输出

`orix install --verbose` 用于调试，允许展示包级细节：

```txt
orix install
----------------------------------------

Registry:
  url: https://registry.npmmirror.com/
  auth: true

Resolution:
  is-even@latest -> 1.0.0
  left-pad@latest -> 1.3.0

Fetch:
  is-buffer@1.1.6 cached
  is-even@1.0.0 downloaded 12ms
  is-number@3.0.0 downloaded 8ms
  is-odd@0.1.2 cached
  kind-of@3.2.2 cached
  left-pad@1.3.0 downloaded 20ms

Link:
  node_modules/is-even -> node_modules/.pnpm/is-even@1.0.0/node_modules/is-even
  node_modules/left-pad -> node_modules/.pnpm/left-pad@1.3.0/node_modules/left-pad

Lockfile:
  unchanged

Done in 0.21s
```

verbose 可以暴露更多内部信息，但仍然不应该泄露 auth token。

## JSON 输出

`orix install --json` 预留给插件、CI 分析和 agent 集成，输出 NDJSON：

```json
{"type":"phase_start","phase":"resolve"}
{"type":"package_resolved","name":"is-even","version":"1.0.0"}
{"type":"phase_done","phase":"resolve","duration_ms":20}
{"type":"phase_start","phase":"fetch"}
{"type":"package_fetched","name":"left-pad","version":"1.3.0","cached":false}
{"type":"phase_done","phase":"fetch","total":6,"success":6,"failed":0}
{"type":"done","duration_ms":210}
```

JSON 模式不输出装饰性文本，不混入 tracing 日志。

## 默认失败输出

失败输出要回答四件事：

1. 哪个阶段失败。
2. 哪个包或文件失败。
3. 为什么失败。
4. 用户下一步可以做什么。

示例：

```txt
orix install
----------------------------------------

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✗ Failed to fetch left-pad@1.3.0

Reason:
  Integrity check failed

Expected:
  sha512-NcdALwpXkTm5Zvvbk7owOUSvVvBKDgKP5/ewfXEznmQFfs4ZRmanOeKBTjRVjka3QFoN6XJ+9F3USqfHqTaU5w==

Received:
  sha512-35c7402f0a579139b966fbdb93ba303944af56f04a0e028fe7f7b07d71339e64057ece194666a739e2814e34558e46b7405a0de9727ef45dd44aa7c7a93694e7

Hint:
  Run:
  orix cache clean left-pad
  orix install
```

失败态同样不保留运行中的过程态。如果 fetch 进行中失败，最终输出应直接刷新为 `✗ Failed to fetch ...`。

## 阶段顺序

默认展示阶段按安装管道顺序排列：

```txt
1. Resolving dependencies
2. Fetching packages
3. Linking dependencies
4. Running lifecycle scripts
5. Writing lockfile
6. Done
```

当前未执行的阶段不展示。例如 MVP 不运行 lifecycle scripts 时，不显示这一行。

lockfile 文案：

```txt
✓ Lockfile unchanged
✓ Updated orix-lock.yaml
✗ Frozen lockfile check failed
```

## Reporter 架构建议

pipeline 不直接 `println!`，只发送结构化事件：

```rust
pub enum InstallEvent {
    Started {
        command: String,
    },
    RegistrySelected {
        url: String,
        authenticated: bool,
    },
    PhaseStarted {
        phase: InstallPhase,
    },
    PackageFetched {
        name: String,
        version: String,
        cached: bool,
    },
    PhaseFinished {
        phase: InstallPhase,
        duration_ms: u64,
    },
    LockfileUnchanged,
    LockfileWritten {
        path: PathBuf,
    },
    Finished {
        direct: usize,
        total: usize,
        duration_ms: u64,
    },
    Failed {
        phase: InstallPhase,
        message: String,
        hint: Option<String>,
    },
}
```

渲染层根据输出模式实现同一套事件：

```rust
pub trait Reporter {
    fn on_event(&mut self, event: InstallEvent);
}
```

实现：

```txt
InteractiveReporter  # TTY 动态 UI，最终只留下摘要
PlainReporter        # CI / non-TTY append-only 日志
VerboseReporter      # 调试输出
JsonReporter         # NDJSON
```

## 最终取舍

默认输出采用：

```txt
简洁摘要 + TTY 动态刷新 + 完成态替换
```

明确不采用：

```txt
过程态逐次落盘 + 最终摘要再次打印
```

这样终端历史里只留下用户真正需要回看的结果：装了多少包、使用哪个 registry、各阶段是否成功、lockfile 是否变化，以及总耗时。
