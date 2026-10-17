//! Prolog bridge error types.

use std::fmt;

#[derive(Debug, Clone)]
pub enum PrologError {
    /// Could not locate or load `libswipl`.
    LibraryNotFound(String),
    /// `PL_initialise` failed.
    InitFailed(String),
    /// Prolog exception during query execution.
    Exception(String),
    /// Conversion between MeTTa and Prolog terms failed.
    Conversion(String),
    /// Internal invariant violation.
    Internal(String),
}

impl fmt::Display for PrologError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrologError::LibraryNotFound(e) => write!(f, "SWI-Prolog library not found: {e}"),
            PrologError::InitFailed(e) => write!(f, "PL_initialise failed: {e}"),
            PrologError::Exception(e) => write!(f, "Prolog exception: {e}"),
            PrologError::Conversion(e) => write!(f, "term conversion failed: {e}"),
            PrologError::Internal(e) => write!(f, "internal error: {e}"),
        }
    }
}

impl std::error::Error for PrologError {}

impl From<std::ffi::NulError> for PrologError {
    fn from(e: std::ffi::NulError) -> Self {
        PrologError::Conversion(e.to_string())
    }
}

impl PrologError {
    /// Convert this error into a MeTTa `(Error <context> "<message>")` atom.
    pub fn to_error_atom(&self, context: &crate::atom::Atom) -> crate::atom::Atom {
        crate::atom::Atom::expr(vec![
            crate::atom::Atom::sym("Error"),
            context.clone(),
            crate::atom::Atom::str_val(&self.to_string()),
        ])
    }
}
