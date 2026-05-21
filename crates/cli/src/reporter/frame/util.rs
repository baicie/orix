//! Frame rendering utilities.

pub(super) fn render_diff_bar(added: usize, removed: usize, width: usize) -> String {
    let total = added + removed;

    if total == 0 || width == 0 {
        return String::new();
    }

    let plus = ((width as f64) * (added as f64 / total as f64)).round() as usize;
    let plus = plus.min(width);
    let minus = width.saturating_sub(plus);

    format!("{}{}", "+".repeat(plus), "-".repeat(minus))
}

pub(super) fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs_f64();

    if secs < 1.0 {
        format!("{:.2}s", secs)
    } else if secs < 10.0 {
        format!("{:.1}s", secs)
    } else {
        format!("{}s", duration.as_secs())
    }
}

pub(super) const CHECKMARK: &str = "\u{2713}";
pub(super) const CROSS: &str = "\u{2717}";
