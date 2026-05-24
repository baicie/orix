# Install 终端颜色输出方案

## 背景

`orix install` 默认输出已经收敛为简洁的阶段摘要：

```txt
orix install
----------------------------------------

Packages: +50 -0
++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies 50/50
✓ Fetched packages 50/50
✓ Linked dependencies
✓ Lockfile written

Done in 47s
```

下一步可以增加颜色，但颜色应服务于信息识别，而不是装饰。目标是让用户一眼看出：

- 哪些阶段成功、失败或正在运行。
- 本次安装新增/删除了多少包。
- registry URL、耗时、错误提示等关键字段。

## 目标

- 默认 TTY 输出使用语义色。
- CI / non-TTY / pipe 输出默认无 ANSI 颜色。
- 尊重 `NO_COLOR`。
- 支持 `--color auto|always|never`。
- 保持现有输出文案和布局稳定。
- 不把 ANSI 控制码散落到业务逻辑中。

## 非目标

- 不重新设计 install 输出结构。
- 不给所有文本上色。
- 不改变 plain / JSON 输出语义。
- 不引入新 UI 框架。

## 配色原则

颜色只表达语义：

| 元素 | 颜色 | 说明 |
| --- | --- | --- |
| `orix install` | bold 默认色 | 命令标题，稳重但可识别 |
| 分隔线 | dim 灰 | 降低视觉噪音 |
| `Packages:` label | dim 默认色 | label 不抢信息焦点 |
| `+50` | green | 新增包 |
| `-0` | dim；非 0 时 red 或 yellow | 删除包为 0 时弱化，非 0 时强调 |
| 进度条 `++++` | green | 与新增包语义一致 |
| `Registry:` label | dim 默认色 | label 弱化 |
| registry URL | cyan | URL 是可复制的信息点 |
| `○ Pending` | dim | 未开始 |
| `● Running` | cyan | 当前进行中 |
| `✓ Done` | green | 成功完成 |
| `✗ Failed` | red | 失败 |
| `Lockfile written` | green | 有实际写入 |
| `Lockfile unchanged` | dim green 或 dim | 成功但无变化 |
| `Done in 47s` | bold green | 成功总结 |
| `Error:` | bold red | 失败标题 |
| `Hint:` | yellow | 修复建议 |

## 默认成功输出

无色文本仍保持为：

```txt
orix install
----------------------------------------

Packages: +50 -0
++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies 50/50
✓ Fetched packages 50/50
✓ Linked dependencies
✓ Lockfile written

Done in 47s
```

TTY 有色渲染语义：

- `orix install`：bold。
- 分隔线：dim。
- `Packages:`：dim。
- `+50`：green。
- `-0`：dim；如果是 `-3` 则 red 或 yellow。
- 进度条：green。
- `Registry:`：dim。
- URL：cyan。
- 每个 `✓`：green。
- 成功阶段文字：默认色或 green；推荐只给符号上色，避免整屏发绿。
- `Done in 47s`：bold green。

## 运行中输出

运行中状态示例：

```txt
orix install
----------------------------------------

Packages: +50 -0
++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies 50/50
● Fetching packages 12/50
  ✓ is-buffer
  ✓ is-even
○ Linking dependencies
○ Writing lockfile
```

TTY 有色渲染语义：

- 已完成 `✓`：green。
- 当前阶段 `●`：cyan。
- 当前阶段计数 `12/50`：cyan 或默认色。
- 最近完成包名前的 `✓`：green。
- 未开始 `○`：dim。

完成后，动态区域仍刷新为最终摘要，不保留过程态。

## 失败输出

示例：

```txt
orix install
----------------------------------------

Packages: +50 -0
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies 50/50
✗ Fetching packages

Error:
  failed to fetch package left-pad@1.3.0: integrity mismatch

Hint:
  Check network connection or try --offline.
```

TTY 有色渲染语义：

- 成功阶段 `✓`：green。
- 失败阶段 `✗`：red。
- `Error:`：bold red。
- 错误正文：默认色。
- `Hint:`：yellow。
- hint 正文：默认色。

## Color 模式

支持三种模式：

| 模式 | 行为 |
| --- | --- |
| `auto` | stderr 是 TTY 且没有 `NO_COLOR` 时启用颜色 |
| `always` | 强制启用颜色，用于手动管道或截图 |
| `never` | 禁用颜色 |

优先级：

```txt
--color never
  > NO_COLOR
  > --color always
  > --color auto + is_terminal
```

说明：

- `NO_COLOR` 应禁用 `auto` 输出颜色。
- 如果用户显式传 `--color always`，可以覆盖 `NO_COLOR`，但需在实现前确认项目偏好。
- CI / non-TTY 默认走无色 PlainReporter。

## 实现方案

### 1. 增加颜色策略类型

位置建议：`crates/cli/src/reporter/color.rs`

```rust
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

pub struct ColorChoice {
    pub enabled: bool,
}
```

判断入口：

```txt
ColorMode + stderr_is_terminal() + NO_COLOR -> color_enabled
```

### 2. 给 FrameRenderer 注入 theme

当前 `FrameRenderer` 负责拼装 install frame。建议让它接收一个轻量 theme：

```rust
pub struct FrameRenderer {
    pub width: usize,
    pub show_recent_packages: bool,
    pub theme: Theme,
}
```

`Theme` 只提供语义方法：

```rust
theme.title("orix install")
theme.dim("----------------------------------------")
theme.added("+50")
theme.removed("-3")
theme.success_symbol("✓")
theme.running_symbol("●")
theme.pending_symbol("○")
theme.url("https://registry.npmmirror.com/")
theme.done("Done in 47s")
theme.error_title("Error:")
theme.hint_title("Hint:")
```

这样 ANSI 细节集中在 theme 内部，frame 逻辑仍然读起来像普通字符串拼装。

### 3. 复用 crossterm

项目已经依赖 `crossterm`，第一版不需要引入新库。

可选实现：

- 使用 `crossterm::style::Stylize` 生成 ANSI 字符串。
- 或封装手写 ANSI escape。

推荐优先使用 `crossterm::style::Stylize`，减少手写控制码错误。

### 4. Reporter 传入 color_enabled

当前 reporter 自动选择 interactive / plain。建议扩展：

```txt
Reporter::auto(no_progress, color_mode)
  -> InteractiveReporter::new(color_enabled)
  -> PlainReporter::new(color_enabled_for_plain)
```

第一版建议：

- InteractiveReporter 支持颜色。
- PlainReporter 默认无色。
- PlainReporter 只有 `--color always` 时才启用颜色。

### 5. CLI 接线

CLI 已有 `--color` 设计意图，接线时应：

- 将 CLI color 参数传给 `run_with_progress`。
- `run_with_progress` 创建 reporter 时传入 color mode。
- 后续 `print_summary` 也应使用同一 theme，避免 fallback summary 和 interactive summary 颜色不一致。

## 测试策略

### 无色测试

保留现有 frame 测试：

```txt
assert!(frame.contains("✓ Resolved dependencies 8/8"));
```

默认 `FrameRenderer::new(width)` 应仍生成无色输出，避免大量测试重写。

### 有色测试

新增少量 focused tests：

- success symbol 包含 green ANSI。
- running symbol 包含 cyan ANSI。
- error title 包含 red/bold ANSI。
- `Theme::plain()` 输出不包含 ANSI。

### strip ANSI

可以提供测试 helper：

```txt
strip_ansi(rendered_colored_frame) == plain_frame
```

这样能验证有色输出没有改变文本语义。

## 第一版范围

第一版只处理 install progress frame：

| 文件 | 改动 |
| --- | --- |
| `crates/cli/src/reporter/frame.rs` | 注入 Theme，渲染语义色 |
| `crates/cli/src/reporter/interactive.rs` | 创建有色 FrameRenderer |
| `crates/cli/src/reporter/plain.rs` | 默认无色，预留 always |
| `crates/cli/src/reporter/mod.rs` | Reporter::auto 接收 color mode |
| `crates/cli/src/main.rs` | CLI color 参数接线 |

暂不处理：

- JSON 输出。
- tracing 日志颜色。
- 非 install 命令输出。
- 大规模快照测试。

## 风险

| 风险 | 缓解 |
| --- | --- |
| ANSI 长度影响终端清屏行数 | `visual_row_count` 必须忽略 ANSI escape；或者 row count 基于 plain frame |
| CI 日志出现控制字符 | non-TTY 默认禁用颜色 |
| 颜色过多降低可读性 | 只给符号、数字、URL、标题上色 |
| Windows 终端兼容性 | 使用 crossterm；CI 覆盖 Windows |
| 测试脆弱 | 只测试少量 ANSI 片段，并用 strip ANSI 验证文本语义 |

## 重要注意：ANSI 与宽度计算

当前 terminal renderer 会用 `unicode_width` 计算 frame 视觉行数。加入颜色后，ANSI escape 不能计入宽度，否则动态刷新可能再次出现残留或清屏错位。

实现时必须满足其中一个条件：

1. `LiveTerminal` 保存 plain frame 用于 row count，colored frame 只用于写入。
2. `visual_row_count` 在计算前 strip ANSI。
3. Theme 能同时输出 plain 和 styled 两个视图。

推荐第一版使用方案 2：给 `visual_row_count` 增加 ANSI stripping，并添加回归测试。

## 验收

- TTY 下 install 输出出现语义色。
- non-TTY 下输出不包含 ANSI。
- `NO_COLOR=1 orix install` 不包含 ANSI。
- `orix install --color never` 不包含 ANSI。
- `orix install --color always` 在 TTY / non-TTY 都包含 ANSI。
- 动态刷新无残留、无错位。
- `cargo test -p orix-cli` 通过。
- `cargo clippy -p orix-cli -- -D warnings` 通过。
