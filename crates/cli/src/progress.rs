//! Progress rendering from pipeline events.

use std::io::Write;

use orix_core::InstallEvent;

/// Renderer that consumes [`InstallEvent`] from the pipeline and displays
/// a multi-line progress display to the terminal.
pub struct ProgressRenderer {
    /// Packages resolved so far (name list).
    resolving: Vec<String>,
    /// Packages fetched so far.
    fetched: Vec<String>,
    /// Total packages to fetch.
    fetch_total: Option<usize>,
    /// Package currently being fetched.
    fetching: Option<String>,
    /// Packages that failed to fetch.
    failures: Vec<String>,
    /// Current phase label.
    phase: String,
    /// Whether linking is in progress.
    linking: bool,
    /// Whether lockfile is being written.
    writing_lockfile: bool,
}

impl Default for ProgressRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressRenderer {
    pub fn new() -> Self {
        Self {
            resolving: Vec::new(),
            fetched: Vec::new(),
            fetch_total: None,
            fetching: None,
            failures: Vec::new(),
            phase: String::new(),
            linking: false,
            writing_lockfile: false,
        }
    }

    /// Process one install event and redraw the progress display.
    pub fn handle(&mut self, event: InstallEvent) {
        match event {
            InstallEvent::ResolvingTotal(n) => {
                self.resolving.reserve(n);
                self.phase = "Resolving".to_string();
            }
            InstallEvent::ResolvingPackage(name) => {
                self.resolving.push(name);
            }
            InstallEvent::FetchingTotal(n) => {
                self.fetch_total = Some(n);
                self.fetched.clear();
                self.phase = "Fetching".to_string();
            }
            InstallEvent::FetchingPackage(name) => {
                self.fetching = Some(name);
            }
            InstallEvent::FetchFailure(msg) => {
                self.failures.push(msg);
                self.fetching = None;
            }
            InstallEvent::Linking => {
                self.fetching = None;
                self.linking = true;
                self.phase = "Linking".to_string();
            }
            InstallEvent::WritingLockfile => {
                self.linking = false;
                self.writing_lockfile = true;
                self.phase = "Writing lockfile".to_string();
            }
        }
        self.render();
    }

    /// Mark that a package download completed.
    pub fn fetched_package(&mut self, name: String) {
        self.fetched.push(name);
        self.fetching = None;
        self.render();
    }

    fn render(&self) {
        let mut out = String::new();

        out.push_str("  orix install\n");
        out.push_str("  ──────────────\n\n");

        // Resolving section
        if !self.resolving.is_empty() || self.phase == "Resolving" {
            out.push_str("  ");
            out.push_str(&self.phase);
            out.push_str(" dependencies\n");
            for pkg in &self.resolving {
                out.push(' ');
                out.push_str(PULL);
                out.push(' ');
                out.push_str(pkg);
                out.push('\n');
            }
            out.push('\n');
        }

        // Fetching section
        if self.fetch_total.is_some() || self.phase == "Fetching" {
            let total = self.fetch_total.unwrap_or(0);
            out.push_str("  ");
            out.push_str(SPINNER);
            out.push_str(" Fetching packages");
            if let Some(ref name) = self.fetching {
                out.push_str(&format!(" ({}/{}): {}", self.fetched.len(), total, name));
            } else {
                out.push_str(&format!(" ({}/{})", self.fetched.len(), total));
            }
            out.push('\n');

            // Show last 3 fetched packages
            let recent: Vec<_> = self.fetched.iter().rev().take(3).collect();
            for pkg in recent.iter().rev() {
                out.push_str("    ");
                out.push_str(CHECK);
                out.push(' ');
                out.push_str(pkg);
                out.push('\n');
            }

            // Show failures
            for failure in &self.failures {
                out.push_str("    ");
                out.push_str(CROSS);
                out.push(' ');
                out.push_str(failure);
                out.push('\n');
            }
            out.push('\n');
        }

        // Linking section
        if self.linking {
            out.push_str("  ");
            out.push_str(SPINNER);
            out.push_str(" Linking packages\n\n");
        }

        // Writing lockfile section
        if self.writing_lockfile {
            out.push_str("  ");
            out.push_str(SPINNER);
            out.push_str(" Writing lockfile\n\n");
        }

        print!("{}", out);
        let _ = std::io::stdout().flush();
    }
}

const SPINNER: &str = "\u{25D4}";
const CHECK: &str = "\u{2713}";
const CROSS: &str = "\u{2717}";
const PULL: &str = "\u{2192}";
