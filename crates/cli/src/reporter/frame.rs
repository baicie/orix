//! UI frame rendering.

use super::color::Theme;
use super::state::{InstallState, PhaseState, StepStatus};
use orix_core::reporter::LockfileStatus;

/// A rendered frame with both colored and plain representations.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RenderedFrame {
    /// The full frame string (may contain ANSI escape sequences).
    pub frame: String,
    /// The frame stripped of ANSI codes, used for row count calculations.
    pub plain: String,
    /// Number of visual rows the plain frame occupies.
    pub row_count: usize,
}

impl RenderedFrame {
    #[allow(dead_code)]
    fn new(frame: String, width: usize) -> Self {
        let plain = super::strip_ansi(&frame);
        let row_count = super::terminal::visual_row_count(&plain, width);
        Self {
            frame,
            plain,
            row_count,
        }
    }
}

/// Renders an `InstallState` into a printable string frame.
pub struct FrameRenderer {
    /// Terminal width for bar calculations.
    pub width: usize,
    /// Whether to show recent packages list.
    pub show_recent_packages: bool,
    /// The color theme.
    theme: Theme,
}

impl FrameRenderer {
    #[allow(dead_code)]
    pub fn new(width: usize) -> Self {
        Self {
            width,
            show_recent_packages: true,
            theme: Theme::plain(),
        }
    }

    /// Create a renderer with an explicit theme.
    pub fn with_theme(width: usize, theme: Theme) -> Self {
        Self {
            width,
            show_recent_packages: true,
            theme,
        }
    }

    /// Render the current state into a complete frame.
    pub fn render(&self, state: &InstallState) -> RenderedFrame {
        let mut colored = String::new();
        let mut plain = String::new();

        self.push_header(&mut colored, &mut plain, state);
        self.push_summary(&mut colored, &mut plain, state);
        self.push_phases(&mut colored, &mut plain, state);

        if state.failed {
            self.push_error(&mut colored, &mut plain, state);
        } else if state.finished {
            self.push_done(&mut colored, &mut plain, state);
        }

        if !colored.ends_with('\n') {
            colored.push('\n');
        }
        if !plain.ends_with('\n') {
            plain.push('\n');
        }

        let row_count = super::terminal::visual_row_count(&plain, self.width);

        RenderedFrame {
            frame: colored,
            plain,
            row_count,
        }
    }

    fn push_header(&self, colored: &mut String, plain: &mut String, state: &InstallState) {
        colored.push_str(&self.theme.title(&state.command));
        colored.push('\n');
        colored.push_str(&self.theme.dim("-".repeat(40).as_str()));
        colored.push_str("\n\n");

        plain.push_str(&state.command);
        plain.push('\n');
        plain.push_str("-".repeat(40).as_str());
        plain.push_str("\n\n");
    }

    fn push_summary(&self, colored: &mut String, plain: &mut String, state: &InstallState) {
        if state.added > 0 || state.removed > 0 {
            colored.push_str(&self.theme.label("Packages:"));
            colored.push(' ');
            colored.push_str(&self.theme.added(&format!("+{}", state.added)));
            colored.push(' ');
            colored.push_str(&self.theme.removed(&format!("-{}", state.removed)));
            colored.push('\n');

            plain.push_str("Packages: ");
            plain.push_str(&format!("+{} -{}\n", state.added, state.removed));

            let bar_width = self.width.saturating_sub(2).min(80);
            let bar = render_diff_bar(state.added, state.removed, bar_width);

            if !bar.is_empty() {
                colored.push_str(&self.theme.added(&bar));
                colored.push('\n');
                plain.push_str(&bar);
                plain.push('\n');
            }
        } else if state.direct_packages > 0 || state.total_packages > 0 {
            let line = format!(
                "Packages: +{} direct, +{} total\n",
                state.direct_packages, state.total_packages
            );
            colored.push_str(&line);
            plain.push_str(&line);
        }

        if let Some(registry) = &state.registry {
            colored.push_str(&self.theme.label("Registry:"));
            colored.push(' ');
            colored.push_str(&self.theme.url(registry));
            colored.push('\n');

            plain.push_str("Registry: ");
            plain.push_str(registry);
            plain.push('\n');
        }

        if state.direct_packages > 0 || state.total_packages > 0 || state.registry.is_some() {
            colored.push('\n');
            plain.push('\n');
        }
    }

    fn push_phases(&self, colored: &mut String, plain: &mut String, state: &InstallState) {
        let resolve_line = self.render_resolve_step(
            &state.resolve,
            "Resolving dependencies",
            "Resolved dependencies",
        );
        colored.push_str(&resolve_line.colored);
        plain.push_str(&resolve_line.plain);
        colored.push('\n');
        plain.push('\n');

        let fetch_line =
            self.render_fetch_step(&state.fetch, "Fetching packages", "Fetched packages");
        colored.push_str(&fetch_line.colored);
        plain.push_str(&fetch_line.plain);
        colored.push('\n');
        plain.push('\n');

        if self.show_recent_packages
            && state.fetch.status == StepStatus::Running
            && !state.recent_packages.is_empty()
        {
            for package in &state.recent_packages {
                let styled = format!("  {} {}", CHECKMARK, package);
                colored.push_str(&self.theme.success(&styled));
                colored.push('\n');

                plain.push_str("  ");
                plain.push_str(CHECKMARK);
                plain.push(' ');
                plain.push_str(package);
                plain.push('\n');
            }
        }

        let link_line =
            self.render_step(&state.link, "Linking dependencies", "Linked dependencies");
        colored.push_str(&link_line.colored);
        plain.push_str(&link_line.plain);
        colored.push('\n');
        plain.push('\n');

        match &state.lockfile_status {
            Some(LockfileStatus::Unchanged) => {
                let styled = format!("{} Lockfile unchanged", CHECKMARK);
                colored.push_str(&self.theme.lockfile_unchanged(&styled));
                colored.push('\n');
                plain.push_str(CHECKMARK);
                plain.push_str(" Lockfile unchanged\n");
            }
            Some(LockfileStatus::Written) => {
                let styled = format!("{} Lockfile written", CHECKMARK);
                colored.push_str(&self.theme.lockfile_written(&styled));
                colored.push('\n');
                plain.push_str(CHECKMARK);
                plain.push_str(" Lockfile written\n");
            }
            Some(LockfileStatus::Skipped) => {
                colored.push_str("- Lockfile skipped\n");
                plain.push_str("- Lockfile skipped\n");
            }
            None => {
                let line = self.render_step(&state.lockfile, "Writing lockfile", "Wrote lockfile");
                colored.push_str(&line.colored);
                plain.push_str(&line.plain);
                colored.push('\n');
                plain.push('\n');
            }
        }
    }

    fn push_error(&self, colored: &mut String, plain: &mut String, state: &InstallState) {
        colored.push('\n');
        plain.push('\n');

        colored.push_str(&self.theme.error_title("Error:"));
        colored.push('\n');
        colored.push_str("  ");
        plain.push_str("Error:\n  ");

        if let Some(message) = &state.error_message {
            colored.push_str(message);
            colored.push('\n');
            plain.push_str(message);
            plain.push('\n');
        }

        if let Some(hint) = &state.error_hint {
            colored.push('\n');
            plain.push('\n');

            colored.push_str(&self.theme.hint_title("Hint:"));
            colored.push('\n');
            colored.push_str("  ");
            plain.push_str("Hint:\n  ");

            colored.push_str(hint);
            colored.push('\n');
            plain.push_str(hint);
            plain.push('\n');
        }
    }

    fn push_done(&self, colored: &mut String, plain: &mut String, state: &InstallState) {
        if let Some(duration) = state.duration {
            colored.push('\n');
            plain.push('\n');

            let styled = format!("Done in {}", format_duration(duration));
            colored.push_str(&self.theme.done(&styled));
            colored.push('\n');
            plain.push_str(&styled);
            plain.push('\n');
        }
    }

    fn render_step(&self, phase: &PhaseState, pending: &str, done: &str) -> StepLine {
        match phase.status {
            StepStatus::Pending => {
                let text = format!("○ {pending}");
                StepLine {
                    colored: self.theme.pending(&text).into_owned(),
                    plain: text,
                }
            }
            StepStatus::Running => {
                let text = format!("● {pending}");
                StepLine {
                    colored: self.theme.running(&text).into_owned(),
                    plain: text,
                }
            }
            StepStatus::Done => {
                let text = format!("{CHECKMARK} {done}");
                StepLine {
                    colored: self.theme.success(&text).into_owned(),
                    plain: text,
                }
            }
            StepStatus::Failed => {
                let text = format!("{CROSS} {pending}");
                StepLine {
                    colored: self.theme.failed(&text).into_owned(),
                    plain: text,
                }
            }
            StepStatus::Skipped => StepLine {
                colored: format!("- {pending}"),
                plain: format!("- {pending}"),
            },
        }
    }

    fn render_fetch_step(&self, phase: &PhaseState, pending: &str, done: &str) -> StepLine {
        match phase.status {
            StepStatus::Pending => {
                let text = format!("○ {pending}");
                StepLine {
                    colored: self.theme.pending(&text).into_owned(),
                    plain: text,
                }
            }
            StepStatus::Running => {
                let text = if phase.total > 0 {
                    format!("● {pending} {}/{}", phase.done, phase.total)
                } else {
                    format!("● {pending}")
                };
                StepLine {
                    colored: self.theme.running(&text).into_owned(),
                    plain: text,
                }
            }
            StepStatus::Done => {
                let text = if phase.total > 0 {
                    format!("{CHECKMARK} {done} {}/{}", phase.done, phase.total)
                } else {
                    format!("{CHECKMARK} {done}")
                };
                StepLine {
                    colored: self.theme.success(&text).into_owned(),
                    plain: text,
                }
            }
            StepStatus::Failed => {
                let text = format!("{CROSS} {pending}");
                StepLine {
                    colored: self.theme.failed(&text).into_owned(),
                    plain: text,
                }
            }
            StepStatus::Skipped => StepLine {
                colored: format!("- {pending}"),
                plain: format!("- {pending}"),
            },
        }
    }

    fn render_resolve_step(&self, phase: &PhaseState, pending: &str, done: &str) -> StepLine {
        if phase.status == StepStatus::Done && phase.total > phase.done && phase.done > 0 {
            let text = format!(
                "{CHECKMARK} {done} {} packages ({} scanned)",
                phase.done, phase.total
            );
            return StepLine {
                colored: self.theme.success(&text).into_owned(),
                plain: text,
            };
        }

        self.render_fetch_step(phase, pending, done)
    }
}

/// A step line with both colored and plain variants.
struct StepLine {
    colored: String,
    plain: String,
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
        assert!(frame.plain.contains("orix install"));
        assert!(frame.plain.contains("Packages: +2 direct, +6 total"));
        assert!(frame
            .plain
            .contains("Registry: https://registry.npmmirror.com/"));
        assert!(frame.plain.contains("Done in 0.21s"));
    }

    #[test]
    fn test_render_colored_has_ansi() {
        let renderer = FrameRenderer::with_theme(80, Theme::always_color());
        let frame = renderer.render(&make_state());
        // Colored version should have ANSI codes
        assert!(frame.frame.starts_with('\x1b'));
        // Plain version should be identical to plain()
        let plain_renderer = FrameRenderer::new(80);
        let plain_frame = plain_renderer.render(&make_state());
        assert_eq!(frame.plain, plain_frame.plain);
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
        assert!(frame.plain.contains("● Fetching packages 4/6"));
    }

    #[test]
    fn test_render_resolve_running() {
        let mut state = make_state();
        state.resolve.status = StepStatus::Running;
        state.resolve.done = 3;
        state.resolve.total = 8;

        let renderer = FrameRenderer::new(80);
        let frame = renderer.render(&state);
        assert!(frame.plain.contains("● Resolving dependencies 3/8"));
    }

    #[test]
    fn test_render_resolve_done() {
        let mut state = make_state();
        state.resolve.status = StepStatus::Done;
        state.resolve.done = 8;
        state.resolve.total = 8;

        let renderer = FrameRenderer::new(80);
        let frame = renderer.render(&state);
        assert!(frame.plain.contains("\u{2713} Resolved dependencies 8/8"));
    }

    #[test]
    fn test_render_resolve_done_with_scanned_total() {
        let mut state = make_state();
        state.resolve.status = StepStatus::Done;
        state.resolve.done = 1000;
        state.resolve.total = 1600;

        let renderer = FrameRenderer::new(80);
        let frame = renderer.render(&state);
        assert!(frame
            .plain
            .contains("\u{2713} Resolved dependencies 1000 packages (1600 scanned)"));
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
        assert!(frame.plain.contains("Error:"));
        assert!(frame.plain.contains("Integrity check failed"));
        assert!(frame.plain.contains("Hint:"));
    }

    #[test]
    fn test_row_count_uses_plain() {
        let renderer = FrameRenderer::new(80);
        let frame = renderer.render(&make_state());
        // Row count should be based on plain text
        assert!(frame.row_count >= 1);
        // Plain should not contain ANSI
        assert!(!frame.plain.contains("\x1b"));
    }
}
