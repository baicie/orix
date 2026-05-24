//! Script reference type.

/// A named script entry from package.json scripts.
#[derive(Debug, Clone)]
pub struct ScriptRef<'a> {
    /// Script name (e.g., "prebuild", "build", "postbuild").
    pub name: String,
    /// Script command string.
    pub command: &'a str,
}
