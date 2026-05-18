//! UI frame rendering.

use super::state::{InstallState, PhaseState, StepStatus};
use orix_core::reporter::LockfileStatus;

/// Renders an `InstallState` into a printable string frame.
pub struct FrameRenderer {
    /// Terminal width for bar calculations.
    pub width: usize,
    /// Whether to show recent packages list.
    pub show_recent_packages: bool,
}

impl FrameRenderer {
    /// Create a new renderer with the given terminal width.
    pub fn new(width: usize) -> Self {
        Self {
            width,
            show_recent_packages: true,
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
                out.push(' ');
                out.push_str(CHECKMARK);
                out.push(' ');
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
                out.push_str(CHECKMARK);
                out.push_str(" Lockfile unchanged\n");
            }
            Some(LockfileStatus::Written) => {
                out.push_str(CHECKMARK);
                out.push_str(" Lockfile written\n");
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
        out.push_str("Error:\n");
        out.push_str("  ");

        if let Some(message) = &state.error_message {
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

fn render_step(phase: &PhaseState, pending: &str, done: &str) -> String {
    match phase.status {
        StepStatus::Pending => format!("○ {pending}"),
        StepStatus::Running => format!("● {pending}"),
        StepStatus::Done => format!("{CHECKMARK} {done}"),
        StepStatus::Failed => format!("{CROSS} {pending}"),
        StepStatus::Skipped => format!("- {pending}"),
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
                format!(
                    "{CHECKMARK} Fetched packages {}/{}",
                    phase.done, phase.total
                )
            } else {
                format!("{CHECKMARK} Fetched packages")
            }
        }
        StepStatus::Failed => format!("{CROSS} Fetching packages"),
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

const CHECKMARK: &str = "\u{2713}";
const CROSS: &str = "\u{2717}";

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
        let renderer = FrameRenderer::new(80);
        let frame = renderer.render(&make_state());
        assert!(frame.contains("orix install"));
        assert!(frame.contains("Packages: +2 direct, +6 total"));
        assert!(frame.contains("Registry: https://registry.npmmirror.com/"));
        assert!(frame.contains("Done in 0.21s"));
    }

    #[test]
    fn test_render_diff_bar() {
        assert_eq!(render_diff_bar(10, 0, 10), "++++++++++");
        assert_eq!(render_diff_bar(0, 10, 10), "----------");
        assert_eq!(render_diff_bar(5, 5, 10), "+++++-----");
        assert_eq!(render_diff_bar(0, 0, 10), "");
        assert_eq!(render_diff_bar(5, 5, 0), "");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_millis(500)), "0.50s");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(format_duration(Duration::from_secs(15)), "15s");
    }

    #[test]
    fn test_render_fetch_running() {
        let mut state = make_state();
        state.fetch.status = StepStatus::Running;
        state.fetch.done = 4;
        state.fetch.total = 6;

        let renderer = FrameRenderer::new(80);
        let frame = renderer.render(&state);
        assert!(frame.contains("● Fetching packages 4/6"));
    }

    #[test]
    fn test_render_error() {
        let mut state = make_state();
        state.fetch.status = StepStatus::Failed;
        state.failed = true;
        state.error_message = Some("Integrity check failed".to_string());
        state.error_hint = Some("Run `orix cache clean`".to_string());

        let renderer = FrameRenderer::new(80);
        let frame = renderer.render(&state);
        assert!(frame.contains("Error:"));
        assert!(frame.contains("Integrity check failed"));
        assert!(frame.contains("Hint:"));
    }
}
