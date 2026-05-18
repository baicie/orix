//! UI frame rendering.

use super::state::{InstallState, PhaseState, StepStatus};
use crate::styles::{ColorState, Style};
use orix_core::reporter::LockfileStatus;

/// Renders an `InstallState` into a printable string frame.
pub struct FrameRenderer {
    /// Terminal width for bar calculations.
    pub width: usize,
    /// Whether to show recent packages list.
    pub show_recent_packages: bool,
    /// Color state.
    color_state: ColorState,
}

impl FrameRenderer {
    /// Create a new renderer with the given terminal width.
    pub fn new(width: usize, color_state: ColorState) -> Self {
        Self {
            width,
            show_recent_packages: true,
            color_state,
        }
    }

    /// Render the current state into a complete frame string.
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
        let cmd = Style::Bold.paint(&state.command, self.color_state);
        out.push_str(&cmd);
        out.push('\n');

        let sep = "-".repeat(40);
        let sep = Style::Header.paint(&sep, self.color_state);
        out.push_str(&sep);
        out.push_str("\n\n");
    }

    fn push_summary(&self, out: &mut String, state: &InstallState) {
        if state.added > 0 || state.removed > 0 {
            out.push_str(&Style::Bold.paint("Packages:", self.color_state));
            out.push(' ');

            if state.added > 0 {
                out.push_str(
                    &Style::DiffAdded.paint(&format!("+{}", state.added), self.color_state),
                );
                out.push(' ');
            }
            if state.removed > 0 {
                out.push_str(
                    &Style::DiffRemoved.paint(&format!("-{}", state.removed), self.color_state),
                );
            }
            out.push('\n');

            let bar_width = self.width.saturating_sub(2).min(80);
            let bar = self.render_diff_bar(state.added, state.removed, bar_width);

            if !bar.is_empty() {
                out.push_str(&bar);
                out.push('\n');
            }
        } else if state.direct_packages > 0 || state.total_packages > 0 {
            out.push_str(&Style::Bold.paint("Packages:", self.color_state));
            out.push_str(" +");
            out.push_str(&self.style_num(state.direct_packages, Style::Success));
            out.push_str(" direct, +");
            out.push_str(&self.style_num(state.total_packages, Style::Success));
            out.push_str(" total\n");
        }

        if let Some(registry) = &state.registry {
            let label = Style::Muted.paint("Registry:", self.color_state);
            let url = Style::Registry.paint(registry, self.color_state);
            out.push_str(&label);
            out.push(' ');
            out.push_str(&url);
            out.push('\n');
        }

        if state.direct_packages > 0 || state.total_packages > 0 || state.registry.is_some() {
            out.push('\n');
        }
    }

    fn style_num(&self, n: usize, style: Style) -> String {
        style.paint(&n.to_string(), self.color_state)
    }

    fn push_phases(&self, out: &mut String, state: &InstallState) {
        let resolve_line = self.render_fetch_step(
            &state.resolve,
            "Resolving dependencies",
            "Resolved dependencies",
        );
        out.push_str(&resolve_line);
        out.push('\n');

        let fetch_line =
            self.render_fetch_step(&state.fetch, "Fetching packages", "Fetched packages");
        out.push_str(&fetch_line);
        out.push('\n');

        if self.show_recent_packages
            && state.fetch.status == StepStatus::Running
            && !state.recent_packages.is_empty()
        {
            for package in &state.recent_packages {
                out.push(' ');
                let check = Style::Checkmark.paint("\u{2713}", self.color_state);
                out.push_str(&check);
                out.push(' ');
                out.push_str(&Style::PackageName.paint(package, self.color_state));
                out.push('\n');
            }
        }

        let link_line =
            self.render_step(&state.link, "Linking dependencies", "Linked dependencies");
        out.push_str(&link_line);
        out.push('\n');

        match &state.lockfile_status {
            Some(LockfileStatus::Unchanged) => {
                let check = Style::Checkmark.paint("\u{2713}", self.color_state);
                out.push_str(&check);
                out.push(' ');
                out.push_str(&Style::Muted.paint("Lockfile unchanged", self.color_state));
                out.push('\n');
            }
            Some(LockfileStatus::Written) => {
                let check = Style::Checkmark.paint("\u{2713}", self.color_state);
                out.push_str(&check);
                out.push(' ');
                out.push_str(&Style::Success.paint("Lockfile written", self.color_state));
                out.push('\n');
            }
            Some(LockfileStatus::Skipped) => {
                let dash = Style::Muted.paint("-", self.color_state);
                out.push_str(&dash);
                out.push_str(" Lockfile skipped\n");
            }
            None => {
                let lockfile_line =
                    self.render_step(&state.lockfile, "Writing lockfile", "Wrote lockfile");
                out.push_str(&lockfile_line);
                out.push('\n');
            }
        }
    }

    fn push_error(&self, out: &mut String, _state: &InstallState) {
        out.push('\n');
        out.push_str(&Style::Error.paint("Error:", self.color_state));
        out.push_str(" ");
        out.push_str(&Style::Muted.paint("see details below.", self.color_state));
    }

    fn push_done(&self, out: &mut String, state: &InstallState) {
        if let Some(duration) = state.duration {
            out.push('\n');
            let label = Style::Muted.paint("Done in", self.color_state);
            let value = Style::Duration.paint(&Self::format_duration(duration), self.color_state);
            out.push_str(&label);
            out.push(' ');
            out.push_str(&value);
            out.push('\n');
        }
    }

    fn render_step(&self, phase: &PhaseState, pending: &str, done: &str) -> String {
        match phase.status {
            StepStatus::Pending => {
                let icon = Style::Muted.paint("○", self.color_state);
                format!(
                    "{} {}",
                    icon,
                    Style::PhasePending.paint(pending, self.color_state)
                )
            }
            StepStatus::Running => {
                let icon = Style::PhaseRunning.paint("●", self.color_state);
                format!(
                    "{} {}",
                    icon,
                    Style::PhaseRunning.paint(pending, self.color_state)
                )
            }
            StepStatus::Done => {
                let icon = Style::Checkmark.paint("\u{2713}", self.color_state);
                format!(
                    "{} {}",
                    icon,
                    Style::PhaseDone.paint(done, self.color_state)
                )
            }
            StepStatus::Failed => {
                let icon = Style::Cross.paint("\u{2717}", self.color_state);
                format!(
                    "{} {}",
                    icon,
                    Style::PhaseFailed.paint(pending, self.color_state)
                )
            }
            StepStatus::Skipped => {
                let icon = Style::Muted.paint("-", self.color_state);
                format!("{} {}", icon, Style::Muted.paint(pending, self.color_state))
            }
        }
    }

    fn render_fetch_step(&self, phase: &PhaseState, pending: &str, done: &str) -> String {
        match phase.status {
            StepStatus::Pending => {
                let icon = Style::Muted.paint("○", self.color_state);
                format!(
                    "{} {}",
                    icon,
                    Style::PhasePending.paint(pending, self.color_state)
                )
            }
            StepStatus::Running => {
                let icon = Style::PhaseRunning.paint("●", self.color_state);
                let label = Style::PhaseRunning.paint(pending, self.color_state);
                if phase.total > 0 {
                    let done_str = Style::Bold.paint(&phase.done.to_string(), self.color_state);
                    let total_str = Style::Muted.paint(&phase.total.to_string(), self.color_state);
                    format!("{} {} {}/{}", icon, label, done_str, total_str)
                } else {
                    format!("{} {}", icon, label)
                }
            }
            StepStatus::Done => {
                let icon = Style::Checkmark.paint("\u{2713}", self.color_state);
                let label = Style::PhaseDone.paint(done, self.color_state);
                if phase.total > 0 {
                    let done_str = Style::Bold.paint(&phase.done.to_string(), self.color_state);
                    let total_str = Style::Muted.paint(&phase.total.to_string(), self.color_state);
                    format!("{} {} {}/{}", icon, label, done_str, total_str)
                } else {
                    format!("{} {}", icon, label)
                }
            }
            StepStatus::Failed => {
                let icon = Style::Cross.paint("\u{2717}", self.color_state);
                format!(
                    "{} {}",
                    icon,
                    Style::PhaseFailed.paint(pending, self.color_state)
                )
            }
            StepStatus::Skipped => {
                let icon = Style::Muted.paint("-", self.color_state);
                format!("{} {}", icon, Style::Muted.paint(pending, self.color_state))
            }
        }
    }

    fn render_diff_bar(&self, added: usize, removed: usize, width: usize) -> String {
        let total = added + removed;

        if total == 0 || width == 0 {
            return String::new();
        }

        let plus = ((width as f64) * (added as f64 / total as f64)).round() as usize;
        let plus = plus.min(width);
        let minus = width.saturating_sub(plus);

        let bar = format!("{}{}", "+".repeat(plus), "-".repeat(minus));
        let colored = if removed > 0 && added > 0 {
            // Mixed: split colors at the boundary
            let plus_part = "+".repeat(plus);
            let minus_part = "-".repeat(minus);
            format!(
                "{}{}",
                Style::DiffAdded.paint(&plus_part, self.color_state),
                Style::DiffRemoved.paint(&minus_part, self.color_state)
            )
        } else if added > 0 {
            Style::DiffAdded.paint(&bar, self.color_state)
        } else {
            Style::DiffRemoved.paint(&bar, self.color_state)
        };

        colored
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_state() -> InstallState {
        InstallState {
            command: "orix install".to_string(),
            registry: Some("https://registry.npmmirror.com/".to_string()),
            direct_packages: 2,
            total_packages: 6,
            resolve: PhaseState {
                status: StepStatus::Done,
                ..Default::default()
            },
            fetch: PhaseState {
                status: StepStatus::Done,
                done: 6,
                total: 6,
            },
            link: PhaseState {
                status: StepStatus::Done,
                ..Default::default()
            },
            lockfile_status: Some(LockfileStatus::Unchanged),
            finished: true,
            duration: Some(Duration::from_millis(210)),
            ..Default::default()
        }
    }

    #[test]
    fn test_render_done() {
        let color_state = ColorState::Disabled;
        let renderer = FrameRenderer::new(80, color_state);
        let frame = renderer.render(&make_state());
        assert!(frame.contains("orix install"));
        assert!(frame.contains("Packages: +"));
        assert!(frame.contains("Registry: https://registry.npmmirror.com/"));
        assert!(frame.contains("Done in"));
    }

    #[test]
    fn test_render_diff_bar() {
        let color_state = ColorState::Disabled;
        let renderer = FrameRenderer::new(80, color_state);
        assert_eq!(renderer.render_diff_bar(10, 0, 10), "++++++++++");
        assert_eq!(renderer.render_diff_bar(0, 10, 10), "----------");
        assert_eq!(renderer.render_diff_bar(5, 5, 10), "+++++-----");
        assert_eq!(renderer.render_diff_bar(0, 0, 10), "");
        assert_eq!(renderer.render_diff_bar(5, 5, 0), "");
    }

    #[test]
    fn test_render_fetch_running() {
        let color_state = ColorState::Disabled;
        let mut state = make_state();
        state.fetch.status = StepStatus::Running;
        state.fetch.done = 4;
        state.fetch.total = 6;

        let renderer = FrameRenderer::new(80, color_state);
        let frame = renderer.render(&state);
        assert!(frame.contains("Fetching packages 4/6"));
    }

    #[test]
    fn test_render_resolve_running() {
        let color_state = ColorState::Disabled;
        let mut state = make_state();
        state.resolve.status = StepStatus::Running;
        state.resolve.done = 3;
        state.resolve.total = 8;

        let renderer = FrameRenderer::new(80, color_state);
        let frame = renderer.render(&state);
        assert!(frame.contains("Resolving dependencies 3/8"));
    }

    #[test]
    fn test_render_error() {
        let color_state = ColorState::Disabled;
        let mut state = make_state();
        state.fetch.status = StepStatus::Failed;
        state.failed = true;
        state.error_message = Some("Integrity check failed".to_string());
        state.error_hint = Some("Run `orix cache clean`".to_string());

        let renderer = FrameRenderer::new(80, color_state);
        let frame = renderer.render(&state);
        assert!(frame.contains("Error:")); // shows "Error: see details below."
        assert!(frame.contains("see details below."));
    }

    #[test]
    fn test_render_with_colors() {
        let color_state = ColorState::Enabled;
        let renderer = FrameRenderer::new(80, color_state);
        let frame = renderer.render(&make_state());
        // With colors enabled, there should be ANSI escape sequences
        assert!(frame.contains("\x1b["));
    }
}
