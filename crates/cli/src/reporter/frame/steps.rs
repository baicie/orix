//! Step line rendering.

use super::super::state::{PhaseState, StepStatus};
use super::util::{CHECKMARK, CROSS};
use super::FrameRenderer;

/// A step line with both colored and plain variants.
pub(super) struct StepLine {
    pub colored: String,
    pub plain: String,
}

impl FrameRenderer {
    pub(super) fn render_step(&self, phase: &PhaseState, pending: &str, done: &str) -> StepLine {
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

    pub(super) fn render_fetch_step(
        &self,
        phase: &PhaseState,
        pending: &str,
        done: &str,
    ) -> StepLine {
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

    pub(super) fn render_resolve_step(
        &self,
        phase: &PhaseState,
        pending: &str,
        done: &str,
    ) -> StepLine {
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
