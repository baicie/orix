/// Errors that can occur when operating on the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A generic store error with a message.
    #[error("store error: {0}")]
    Other(String),
}
