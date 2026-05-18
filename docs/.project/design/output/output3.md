可以，`orix` 这里我建议你按 **pnpm / cargo / uv 那种事件驱动 reporter 架构** 做，而不是在安装流程里直接 `println!`。

核心设计是：

```txt
pipeline 安装逻辑
  ↓ 发事件
InstallEvent
  ↓ 修改状态
InstallState
  ↓ 生成整块 UI 字符串
FrameRenderer
  ↓ 终端原地刷新
LiveTerminal / PlainTerminal / JsonReporter
```

---

# 1. 最终目录结构建议

```txt
src/
  cli/
    reporter/
      mod.rs
      event.rs          # InstallEvent 定义
      state.rs          # InstallState 状态机
      frame.rs          # 根据 state 生成 UI frame
      terminal.rs       # crossterm 原地刷新
      interactive.rs    # TTY 动态 reporter
      plain.rs          # CI / 非 TTY reporter
      json.rs           # 后续 --json reporter
```

你后续的安装流程里只做：

```rust
reporter.on_event(InstallEvent::PhaseStarted(InstallPhase::Resolve));
reporter.on_event(InstallEvent::Resolved { direct: 2, total: 6 });
reporter.on_event(InstallEvent::FetchProgress { done: 3, total: 6 });
```

不要在 install pipeline 里直接输出 UI。

---

# 2. Cargo 依赖

```toml
[dependencies]
crossterm = "0.28"
unicode-width = "0.1"
```

如果你的项目已经用了新版 `crossterm`，按现有 lock 走也行，下面用到的 API 都比较稳定。

---

# 3. 事件层设计

`src/cli/reporter/event.rs`

```rust
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstallPhase {
    Resolve,
    Fetch,
    Link,
    Lockfile,
    Scripts,
}

#[derive(Debug, Clone)]
pub enum LockfileStatus {
    Unchanged,
    Written,
    Skipped,
}

#[derive(Debug, Clone)]
pub enum InstallEvent {
    Started {
        command: String,
    },

    RegistrySelected {
        url: String,
        authenticated: bool,
    },

    DirectPackages {
        count: usize,
        names: Vec<String>,
    },

    PhaseStarted {
        phase: InstallPhase,
    },

    Resolved {
        direct: usize,
        total: usize,
        added: usize,
        removed: usize,
    },

    FetchProgress {
        done: usize,
        total: usize,
        package: Option<String>,
    },

    PackageFetched {
        name: String,
        version: Option<String>,
        cached: bool,
    },

    PhaseFinished {
        phase: InstallPhase,
    },

    Lockfile {
        status: LockfileStatus,
    },

    Finished {
        installed: usize,
        duration: Duration,
    },

    Failed {
        phase: Option<InstallPhase>,
        message: String,
        hint: Option<String>,
    },
}
```

重点是：**事件只描述发生了什么，不关心怎么显示。**

---

# 4. 状态层设计

`src/cli/reporter/state.rs`

```rust
use std::collections::VecDeque;
use std::time::Duration;

use super::event::{InstallEvent, InstallPhase, LockfileStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct PhaseState {
    pub status: StepStatus,
    pub done: usize,
    pub total: usize,
}

impl Default for PhaseState {
    fn default() -> Self {
        Self {
            status: StepStatus::Pending,
            done: 0,
            total: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstallState {
    pub command: String,

    pub registry: Option<String>,
    pub authenticated: bool,

    pub direct_packages: usize,
    pub total_packages: usize,

    pub added: usize,
    pub removed: usize,

    pub resolve: PhaseState,
    pub fetch: PhaseState,
    pub link: PhaseState,
    pub lockfile: PhaseState,
    pub scripts: PhaseState,

    pub lockfile_status: Option<LockfileStatus>,

    pub recent_packages: VecDeque<String>,
    pub max_recent_packages: usize,

    pub finished: bool,
    pub failed: bool,
    pub error_message: Option<String>,
    pub error_hint: Option<String>,

    pub duration: Option<Duration>,
}

impl Default for InstallState {
    fn default() -> Self {
        Self {
            command: "orix install".to_string(),

            registry: None,
            authenticated: false,

            direct_packages: 0,
            total_packages: 0,

            added: 0,
            removed: 0,

            resolve: PhaseState::default(),
            fetch: PhaseState::default(),
            link: PhaseState::default(),
            lockfile: PhaseState::default(),
            scripts: PhaseState::default(),

            lockfile_status: None,

            recent_packages: VecDeque::new(),
            max_recent_packages: 5,

            finished: false,
            failed: false,
            error_message: None,
            error_hint: None,

            duration: None,
        }
    }
}

impl InstallState {
    pub fn apply(&mut self, event: InstallEvent) {
        match event {
            InstallEvent::Started { command } => {
                self.command = command;
            }

            InstallEvent::RegistrySelected { url, authenticated } => {
                self.registry = Some(url);
                self.authenticated = authenticated;
            }

            InstallEvent::DirectPackages { count, names } => {
                self.direct_packages = count;

                for name in names {
                    self.push_recent_package(name);
                }
            }

            InstallEvent::PhaseStarted { phase } => {
                self.phase_mut(phase).status = StepStatus::Running;
            }

            InstallEvent::Resolved {
                direct,
                total,
                added,
                removed,
            } => {
                self.direct_packages = direct;
                self.total_packages = total;
                self.added = added;
                self.removed = removed;
                self.resolve.status = StepStatus::Done;
            }

            InstallEvent::FetchProgress {
                done,
                total,
                package,
            } => {
                self.fetch.status = if done >= total && total > 0 {
                    StepStatus::Done
                } else {
                    StepStatus::Running
                };

                self.fetch.done = done;
                self.fetch.total = total;

                if let Some(package) = package {
                    self.push_recent_package(package);
                }
            }

            InstallEvent::PackageFetched {
                name,
                version,
                cached,
            } => {
                let label = match version {
                    Some(version) => {
                        if cached {
                            format!("{name}@{version} cached")
                        } else {
                            format!("{name}@{version}")
                        }
                    }
                    None => {
                        if cached {
                            format!("{name} cached")
                        } else {
                            name
                        }
                    }
                };

                self.push_recent_package(label);
            }

            InstallEvent::PhaseFinished { phase } => {
                self.phase_mut(phase).status = StepStatus::Done;
            }

            InstallEvent::Lockfile { status } => {
                self.lockfile_status = Some(status);
                self.lockfile.status = StepStatus::Done;
            }

            InstallEvent::Finished {
                installed: _,
                duration,
            } => {
                self.finished = true;
                self.duration = Some(duration);

                if self.resolve.status == StepStatus::Running {
                    self.resolve.status = StepStatus::Done;
                }

                if self.fetch.status == StepStatus::Running {
                    self.fetch.status = StepStatus::Done;
                }

                if self.link.status == StepStatus::Running {
                    self.link.status = StepStatus::Done;
                }

                if self.lockfile.status == StepStatus::Running {
                    self.lockfile.status = StepStatus::Done;
                }
            }

            InstallEvent::Failed {
                phase,
                message,
                hint,
            } => {
                self.failed = true;
                self.error_message = Some(message);
                self.error_hint = hint;

                if let Some(phase) = phase {
                    self.phase_mut(phase).status = StepStatus::Failed;
                }
            }
        }
    }

    fn phase_mut(&mut self, phase: InstallPhase) -> &mut PhaseState {
        match phase {
            InstallPhase::Resolve => &mut self.resolve,
            InstallPhase::Fetch => &mut self.fetch,
            InstallPhase::Link => &mut self.link,
            InstallPhase::Lockfile => &mut self.lockfile,
            InstallPhase::Scripts => &mut self.scripts,
        }
    }

    fn push_recent_package(&mut self, package: String) {
        if package.is_empty() {
            return;
        }

        if self.recent_packages.back() == Some(&package) {
            return;
        }

        self.recent_packages.push_back(package);

        while self.recent_packages.len() > self.max_recent_packages {
            self.recent_packages.pop_front();
        }
    }
}
```

---

# 5. Frame 渲染层设计

`src/cli/reporter/frame.rs`

```rust
use super::event::LockfileStatus;
use super::state::{InstallState, PhaseState, StepStatus};

pub struct FrameRenderer {
    pub width: usize,
    pub show_recent_packages: bool,
}

impl FrameRenderer {
    pub fn new(width: usize) -> Self {
        Self {
            width,
            show_recent_packages: true,
        }
    }

    pub fn render(&self, state: &InstallState) -> String {
        let mut out = String::new();

        self.push_header(&mut out, state);
        self.push_summary(&mut out, state);
        self.push_phases(&mut out, state);

        if state.failed {
            self.push_error(&mut out, state);
        } else if state.finished {
            self.push_done(&mut out, state);
        }

        if !out.ends_with('\n') {
            out.push('\n');
        }

        out
    }

    fn push_header(&self, out: &mut String, state: &InstallState) {
        out.push_str(&state.command);
        out.push('\n');
        out.push_str(&"-".repeat(40));
        out.push_str("\n\n");
    }

    fn push_summary(&self, out: &mut String, state: &InstallState) {
        if state.added > 0 || state.removed > 0 {
            out.push_str(&format!("Packages: +{} -{}\n", state.added, state.removed));

            let bar_width = self.width.saturating_sub(2).min(80);
            let bar = render_diff_bar(state.added, state.removed, bar_width);

            if !bar.is_empty() {
                out.push_str(&bar);
                out.push('\n');
            }
        } else if state.direct_packages > 0 || state.total_packages > 0 {
            out.push_str(&format!(
                "Packages: +{} direct, +{} total\n",
                state.direct_packages, state.total_packages
            ));
        }

        if let Some(registry) = &state.registry {
            out.push_str(&format!("Registry: {registry}\n"));
        }

        if state.direct_packages > 0 || state.total_packages > 0 || state.registry.is_some() {
            out.push('\n');
        }
    }

    fn push_phases(&self, out: &mut String, state: &InstallState) {
        out.push_str(&render_step(
            &state.resolve,
            "Resolving dependencies",
            "Resolved dependencies",
        ));
        out.push('\n');

        out.push_str(&render_fetch_step(&state.fetch));
        out.push('\n');

        if self.show_recent_packages
            && state.fetch.status == StepStatus::Running
            && !state.recent_packages.is_empty()
        {
            for package in &state.recent_packages {
                out.push_str("  ✓ ");
                out.push_str(package);
                out.push('\n');
            }
        }

        out.push_str(&render_step(
            &state.link,
            "Linking dependencies",
            "Linked dependencies",
        ));
        out.push('\n');

        match &state.lockfile_status {
            Some(LockfileStatus::Unchanged) => {
                out.push_str("✓ Lockfile unchanged\n");
            }
            Some(LockfileStatus::Written) => {
                out.push_str("✓ Lockfile written\n");
            }
            Some(LockfileStatus::Skipped) => {
                out.push_str("- Lockfile skipped\n");
            }
            None => {
                out.push_str(&render_step(
                    &state.lockfile,
                    "Writing lockfile",
                    "Wrote lockfile",
                ));
                out.push('\n');
            }
        }
    }

    fn push_error(&self, out: &mut String, state: &InstallState) {
        out.push('\n');

        if let Some(message) = &state.error_message {
            out.push_str("Error:\n");
            out.push_str("  ");
            out.push_str(message);
            out.push('\n');
        }

        if let Some(hint) = &state.error_hint {
            out.push('\n');
            out.push_str("Hint:\n");
            out.push_str("  ");
            out.push_str(hint);
            out.push('\n');
        }
    }

    fn push_done(&self, out: &mut String, state: &InstallState) {
        if let Some(duration) = state.duration {
            out.push('\n');
            out.push_str(&format!("Done in {}\n", format_duration(duration)));
        }
    }
}

fn render_step(phase: &PhaseState, running: &str, done: &str) -> String {
    match phase.status {
        StepStatus::Pending => format!("○ {running}"),
        StepStatus::Running => format!("● {running}"),
        StepStatus::Done => format!("✓ {done}"),
        StepStatus::Failed => format!("✗ {running}"),
        StepStatus::Skipped => format!("- {running}"),
    }
}

fn render_fetch_step(phase: &PhaseState) -> String {
    match phase.status {
        StepStatus::Pending => "○ Fetching packages".to_string(),
        StepStatus::Running => {
            if phase.total > 0 {
                format!("● Fetching packages {}/{}", phase.done, phase.total)
            } else {
                "● Fetching packages".to_string()
            }
        }
        StepStatus::Done => {
            if phase.total > 0 {
                format!("✓ Fetched packages {}/{}", phase.done, phase.total)
            } else {
                "✓ Fetched packages".to_string()
            }
        }
        StepStatus::Failed => "✗ Fetching packages".to_string(),
        StepStatus::Skipped => "- Fetching packages".to_string(),
    }
}

fn render_diff_bar(added: usize, removed: usize, width: usize) -> String {
    let total = added + removed;

    if total == 0 || width == 0 {
        return String::new();
    }

    let plus = ((width as f64) * (added as f64 / total as f64)).round() as usize;
    let plus = plus.min(width);
    let minus = width.saturating_sub(plus);

    format!("{}{}", "+".repeat(plus), "-".repeat(minus))
}

fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs_f64();

    if secs < 1.0 {
        format!("{:.2}s", secs)
    } else if secs < 10.0 {
        format!("{:.1}s", secs)
    } else {
        format!("{}s", duration.as_secs())
    }
}
```

这个会生成你想要的：

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

如果有大规模变更：

```txt
Packages: +1130 -867
+++++++++++++++++++++++++++++++++++++++++++++-----------------------------------
```

---

# 6. crossterm 原地刷新层

`src/cli/reporter/terminal.rs`

```rust
use std::io::{self, IsTerminal, Write};

use crossterm::{
    cursor::{Hide, MoveDown, MoveToColumn, MoveUp, Show},
    queue,
    terminal::{self, Clear, ClearType},
};
use unicode_width::UnicodeWidthStr;

pub struct LiveTerminal<W: Write> {
    writer: W,
    last_rows: usize,
    hidden_cursor: bool,
}

impl<W: Write> LiveTerminal<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            last_rows: 0,
            hidden_cursor: false,
        }
    }

    pub fn render(&mut self, frame: &str) -> io::Result<()> {
        self.hide_cursor_once()?;
        self.clear_previous()?;

        write!(self.writer, "{frame}")?;
        self.writer.flush()?;

        let columns = terminal_width();
        self.last_rows = visual_row_count(frame, columns);

        Ok(())
    }

    pub fn finish(mut self, frame: &str) -> io::Result<()> {
        self.render(frame)?;
        queue!(self.writer, Show)?;
        self.writer.flush()?;
        self.hidden_cursor = false;
        Ok(())
    }

    fn hide_cursor_once(&mut self) -> io::Result<()> {
        if !self.hidden_cursor {
            queue!(self.writer, Hide)?;
            self.hidden_cursor = true;
        }

        Ok(())
    }

    fn clear_previous(&mut self) -> io::Result<()> {
        if self.last_rows == 0 {
            return Ok(());
        }

        queue!(
            self.writer,
            MoveUp(self.last_rows as u16),
            MoveToColumn(0)
        )?;

        for row in 0..self.last_rows {
            queue!(
                self.writer,
                Clear(ClearType::CurrentLine),
                MoveToColumn(0)
            )?;

            if row + 1 < self.last_rows {
                queue!(self.writer, MoveDown(1), MoveToColumn(0))?;
            }
        }

        queue!(
            self.writer,
            MoveUp(self.last_rows.saturating_sub(1) as u16),
            MoveToColumn(0)
        )?;

        Ok(())
    }
}

impl<W: Write> Drop for LiveTerminal<W> {
    fn drop(&mut self) {
        if self.hidden_cursor {
            let _ = queue!(self.writer, Show);
            let _ = self.writer.flush();
        }
    }
}

pub fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
}

pub fn stderr_is_terminal() -> bool {
    io::stderr().is_terminal()
}

pub fn terminal_width() -> usize {
    terminal::size()
        .map(|(width, _)| width as usize)
        .unwrap_or(80)
        .max(20)
}

fn visual_row_count(frame: &str, columns: usize) -> usize {
    let columns = columns.max(1);

    let rows = frame
        .lines()
        .map(|line| {
            let width = UnicodeWidthStr::width(line);
            let rows = width.div_ceil(columns);
            rows.max(1)
        })
        .sum::<usize>();

    rows.max(1)
}
```

注意：我建议动态进度输出到 **stderr**，不是 stdout。这样以后你支持：

```bash
orix install --json > result.json
```

不会被进度 UI 污染。

---

# 7. Interactive Reporter

`src/cli/reporter/interactive.rs`

```rust
use std::io;
use std::time::{Duration, Instant};

use super::event::InstallEvent;
use super::frame::FrameRenderer;
use super::state::InstallState;
use super::terminal::{terminal_width, LiveTerminal};

pub struct InteractiveReporter {
    state: InstallState,
    terminal: Option<LiveTerminal<io::Stderr>>,
    last_rendered_frame: String,
    last_render_at: Instant,
    min_render_interval: Duration,
}

impl InteractiveReporter {
    pub fn new() -> Self {
        Self {
            state: InstallState::default(),
            terminal: Some(LiveTerminal::new(io::stderr())),
            last_rendered_frame: String::new(),
            last_render_at: Instant::now() - Duration::from_secs(1),
            min_render_interval: Duration::from_millis(33),
        }
    }

    pub fn on_event(&mut self, event: InstallEvent) -> io::Result<()> {
        self.state.apply(event);

        let force = self.state.finished || self.state.failed;
        self.render(force)
    }

    fn render(&mut self, force: bool) -> io::Result<()> {
        let now = Instant::now();

        if !force && now.duration_since(self.last_render_at) < self.min_render_interval {
            return Ok(());
        }

        let width = terminal_width();
        let renderer = FrameRenderer::new(width);
        let frame = renderer.render(&self.state);

        if !force && frame == self.last_rendered_frame {
            return Ok(());
        }

        self.last_rendered_frame = frame.clone();
        self.last_render_at = now;

        if let Some(terminal) = self.terminal.as_mut() {
            terminal.render(&frame)?;
        }

        if force {
            if let Some(terminal) = self.terminal.take() {
                terminal.finish(&frame)?;
            }
        }

        Ok(())
    }
}
```

这里有几个关键点：

```txt
1. 33ms 节流，避免 fetch 很快时疯狂刷新
2. frame 没变化就不刷新
3. finished / failed 强制刷新
4. Drop 时恢复光标
5. 输出到 stderr
```

---

# 8. Plain Reporter

CI / 非 TTY 模式不要动态刷新。

`src/cli/reporter/plain.rs`

```rust
use std::io::{self, Write};

use super::event::{InstallEvent, InstallPhase, LockfileStatus};

pub struct PlainReporter {
    writer: io::Stderr,
}

impl PlainReporter {
    pub fn new() -> Self {
        Self {
            writer: io::stderr(),
        }
    }

    pub fn on_event(&mut self, event: InstallEvent) -> io::Result<()> {
        match event {
            InstallEvent::Started { command } => {
                writeln!(self.writer, "{command}")?;
            }

            InstallEvent::RegistrySelected { url, .. } => {
                writeln!(self.writer, "registry: {url}")?;
            }

            InstallEvent::DirectPackages { count, .. } => {
                writeln!(self.writer, "packages: {count} direct")?;
            }

            InstallEvent::PhaseStarted { phase } => {
                writeln!(self.writer, "[{}] {}", phase_index(phase), phase_label(phase))?;
            }

            InstallEvent::Resolved { direct, total, .. } => {
                writeln!(
                    self.writer,
                    "resolved dependencies: {direct} direct, {total} total"
                )?;
            }

            InstallEvent::FetchProgress { done, total, .. } => {
                writeln!(self.writer, "fetching packages: {done}/{total}")?;
            }

            InstallEvent::PackageFetched { name, version, cached } => {
                let version = version.unwrap_or_default();
                let cached = if cached { " cached" } else { "" };

                if version.is_empty() {
                    writeln!(self.writer, "fetched {name}{cached}")?;
                } else {
                    writeln!(self.writer, "fetched {name}@{version}{cached}")?;
                }
            }

            InstallEvent::PhaseFinished { phase } => {
                writeln!(self.writer, "finished {}", phase_label(phase))?;
            }

            InstallEvent::Lockfile { status } => match status {
                LockfileStatus::Unchanged => {
                    writeln!(self.writer, "lockfile unchanged")?;
                }
                LockfileStatus::Written => {
                    writeln!(self.writer, "lockfile written")?;
                }
                LockfileStatus::Skipped => {
                    writeln!(self.writer, "lockfile skipped")?;
                }
            },

            InstallEvent::Finished { duration, .. } => {
                writeln!(self.writer, "done in {:.2}s", duration.as_secs_f64())?;
            }

            InstallEvent::Failed { message, hint, .. } => {
                writeln!(self.writer, "error: {message}")?;

                if let Some(hint) = hint {
                    writeln!(self.writer, "hint: {hint}")?;
                }
            }
        }

        self.writer.flush()
    }
}

fn phase_index(phase: InstallPhase) -> usize {
    match phase {
        InstallPhase::Resolve => 1,
        InstallPhase::Fetch => 2,
        InstallPhase::Link => 3,
        InstallPhase::Lockfile => 4,
        InstallPhase::Scripts => 5,
    }
}

fn phase_label(phase: InstallPhase) -> &'static str {
    match phase {
        InstallPhase::Resolve => "resolving dependencies",
        InstallPhase::Fetch => "fetching packages",
        InstallPhase::Link => "linking dependencies",
        InstallPhase::Lockfile => "writing lockfile",
        InstallPhase::Scripts => "running lifecycle scripts",
    }
}
```

CI 里会输出类似：

```txt
orix install
registry: https://registry.npmmirror.com/
packages: 2 direct
[1] resolving dependencies
resolved dependencies: 2 direct, 6 total
[2] fetching packages
fetching packages: 6/6
[3] linking dependencies
finished linking dependencies
lockfile unchanged
done in 0.21s
```

---

# 9. Reporter 统一入口

`src/cli/reporter/mod.rs`

```rust
mod event;
mod frame;
mod interactive;
mod plain;
mod state;
mod terminal;

pub use event::{InstallEvent, InstallPhase, LockfileStatus};

use std::io;

use interactive::InteractiveReporter;
use plain::PlainReporter;
use terminal::stderr_is_terminal;

pub enum Reporter {
    Interactive(InteractiveReporter),
    Plain(PlainReporter),
}

impl Reporter {
    pub fn auto(no_progress: bool) -> Self {
        if !no_progress && stderr_is_terminal() {
            Self::Interactive(InteractiveReporter::new())
        } else {
            Self::Plain(PlainReporter::new())
        }
    }

    pub fn on_event(&mut self, event: InstallEvent) -> io::Result<()> {
        match self {
            Reporter::Interactive(reporter) => reporter.on_event(event),
            Reporter::Plain(reporter) => reporter.on_event(event),
        }
    }
}
```

CLI 参数建议：

```txt
--no-progress  强制 PlainReporter
--json         JsonReporter
--verbose      PlainReporter + 详细事件
```

---

# 10. install pipeline 里怎么用

你的安装流程大概这样接：

```rust
use std::time::Instant;

use crate::cli::reporter::{
    InstallEvent, InstallPhase, LockfileStatus, Reporter,
};

pub fn run_install(no_progress: bool) -> anyhow::Result<()> {
    let started_at = Instant::now();

    let mut reporter = Reporter::auto(no_progress);

    reporter.on_event(InstallEvent::Started {
        command: "orix install".to_string(),
    })?;

    reporter.on_event(InstallEvent::RegistrySelected {
        url: "https://registry.npmmirror.com/".to_string(),
        authenticated: true,
    })?;

    reporter.on_event(InstallEvent::DirectPackages {
        count: 2,
        names: vec!["is-even".to_string(), "left-pad".to_string()],
    })?;

    reporter.on_event(InstallEvent::PhaseStarted {
        phase: InstallPhase::Resolve,
    })?;

    // resolve...
    reporter.on_event(InstallEvent::Resolved {
        direct: 2,
        total: 6,
        added: 0,
        removed: 0,
    })?;

    reporter.on_event(InstallEvent::PhaseStarted {
        phase: InstallPhase::Fetch,
    })?;

    let packages = [
        "is-buffer",
        "is-even",
        "is-number",
        "is-odd",
        "kind-of",
        "left-pad",
    ];

    for (index, package) in packages.iter().enumerate() {
        reporter.on_event(InstallEvent::FetchProgress {
            done: index,
            total: packages.len(),
            package: Some((*package).to_string()),
        })?;

        // fetch one package...

        reporter.on_event(InstallEvent::PackageFetched {
            name: (*package).to_string(),
            version: None,
            cached: false,
        })?;

        reporter.on_event(InstallEvent::FetchProgress {
            done: index + 1,
            total: packages.len(),
            package: Some((*package).to_string()),
        })?;
    }

    reporter.on_event(InstallEvent::PhaseFinished {
        phase: InstallPhase::Fetch,
    })?;

    reporter.on_event(InstallEvent::PhaseStarted {
        phase: InstallPhase::Link,
    })?;

    // link...
    reporter.on_event(InstallEvent::PhaseFinished {
        phase: InstallPhase::Link,
    })?;

    reporter.on_event(InstallEvent::PhaseStarted {
        phase: InstallPhase::Lockfile,
    })?;

    // write lockfile...
    reporter.on_event(InstallEvent::Lockfile {
        status: LockfileStatus::Unchanged,
    })?;

    reporter.on_event(InstallEvent::Finished {
        installed: 6,
        duration: started_at.elapsed(),
    })?;

    Ok(())
}
```

---

# 11. 最终输出效果

安装中：

```txt
orix install
----------------------------------------

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
● Fetching packages 4/6
  ✓ is-buffer
  ✓ is-even
  ✓ is-number
  ✓ is-odd
○ Linking dependencies
○ Writing lockfile
```

完成后终端只留下：

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

不会留下中间那些重复过程。

---

# 12. 错误输出设计

失败时，状态机应该输出：

```txt
orix install
----------------------------------------

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✗ Fetching packages
○ Linking dependencies
○ Writing lockfile

Error:
  Integrity check failed for left-pad@1.3.0

Hint:
  Run `orix cache clean left-pad` and try again.
```

pipeline 里这样发：

```rust
reporter.on_event(InstallEvent::Failed {
    phase: Some(InstallPhase::Fetch),
    message: "Integrity check failed for left-pad@1.3.0".to_string(),
    hint: Some("Run `orix cache clean left-pad` and try again.".to_string()),
})?;
```

---

# 13. 关键规则

你这个 UI 最重要的规则是：

```txt
1. pipeline 不直接 println
2. 所有输出都通过 InstallEvent
3. TTY 动态刷新
4. 非 TTY 普通逐行输出
5. 最终只保留 final frame
6. progress 输出到 stderr
7. JSON 输出到 stdout
8. 错误必须带 phase、message、hint
```

---

# 14. 和 pnpm 的区别

pnpm 更像：

```txt
state -> frame -> ansi-diff 计算最小差异 -> 更新终端
```

你的方案是：

```txt
state -> frame -> crossterm 清空旧 frame -> 重画新 frame
```

早期完全够用，复杂度低很多。等以后你觉得闪烁明显，再做真正的 diff renderer。

---

# 15. 我建议你先实现的最小 MVP

第一版只做这几个事件：

```rust
Started
RegistrySelected
Resolved
FetchProgress
PhaseStarted
PhaseFinished
Lockfile
Finished
Failed
```

第一版 reporter 只做两个：

```txt
InteractiveReporter
PlainReporter
```

等稳定后再加：

```txt
JsonReporter
VerboseReporter
Package-level detailed logs
```

---

最终架构就定成：

```txt
InstallEvent
  -> InstallState::apply()
  -> FrameRenderer::render()
  -> LiveTerminal::render()
```

这套结构后面你无论要模仿 pnpm、cargo、uv，还是给 VS Code 插件/agent 输出 JSON 事件，都不会推倒重来。
