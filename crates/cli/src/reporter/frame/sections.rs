//! Frame section builders.

use super::super::state::{InstallState, StepStatus};
use super::util::{format_duration, render_diff_bar, CHECKMARK};
use super::FrameRenderer;
use orix_core::reporter::LockfileStatus;

impl FrameRenderer {
    pub(super) fn push_header(
        &self,
        colored: &mut String,
        plain: &mut String,
        state: &InstallState,
    ) {
        colored.push_str(&self.theme.title(&state.command));
        colored.push('\n');
        colored.push_str(&self.theme.dim("-".repeat(40).as_str()));
        colored.push_str("\n\n");

        plain.push_str(&state.command);
        plain.push('\n');
        plain.push_str("-".repeat(40).as_str());
        plain.push_str("\n\n");
    }

    pub(super) fn push_summary(
        &self,
        colored: &mut String,
        plain: &mut String,
        state: &InstallState,
    ) {
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

    pub(super) fn push_phases(
        &self,
        colored: &mut String,
        plain: &mut String,
        state: &InstallState,
    ) {
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

    pub(super) fn push_error(
        &self,
        colored: &mut String,
        plain: &mut String,
        state: &InstallState,
    ) {
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

    pub(super) fn push_done(&self, colored: &mut String, plain: &mut String, state: &InstallState) {
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
}
