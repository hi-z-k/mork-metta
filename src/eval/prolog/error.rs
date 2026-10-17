//! Prolog bridge error types.

use std::fmt;

#[derive(Debug, Clone)]
pub enum PrologError {
    LibraryNotFound(String),
    InitFailed(String),
    Exception(String),
    Conversion(String),
    Internal(String),
}

impl fmt::Display for PrologError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (what, e) = match self {
            PrologError::LibraryNotFound(e) => ("SWI-Prolog library not found", e),
            PrologError::InitFailed(e) => ("PL_initialise failed", e),
            PrologError::Exception(e) => ("Prolog exception", e),
            PrologError::Conversion(e) => ("term conversion failed", e),
            PrologError::Internal(e) => ("internal error", e),
        };
        write!(f, "{what}: {e}")
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
