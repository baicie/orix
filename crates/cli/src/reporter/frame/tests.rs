//! Frame renderer tests.

use std::time::Duration;

use super::super::color::Theme;
use super::super::state::{InstallState, PhaseState, StepStatus};
use super::util::{format_duration, render_diff_bar};
use super::*;
use orix_core::reporter::LockfileStatus;

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
fn test_render_body_omits_command_header_but_keeps_summary() {
    let renderer = FrameRenderer::new(80);
    let frame = renderer.render_body(&make_state());

    assert!(!frame.plain.contains("orix install"));
    assert!(frame.plain.contains("Packages: +2 direct, +6 total"));
    assert!(frame.plain.contains("Done in 0.21s"));
}

#[test]
fn test_render_colored_has_ansi() {
    let renderer = FrameRenderer::with_theme(80, Theme::always_color());
    let frame = renderer.render(&make_state());
    assert!(frame.frame.starts_with('\x1b'));
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
    assert!(frame.row_count >= 1);
    assert!(!frame.plain.contains("\x1b"));
}
