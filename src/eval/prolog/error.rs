use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum PrologError {
    #[error("SWI-Prolog library not found: {0}")]
    LibraryNotFound(String),
    #[error("PL_initialise failed: {0}")]
    InitFailed(String),
    #[error("Prolog exception: {0}")]
    Exception(String),
    #[error("term conversion failed: {0}")]
    Conversion(String),
    #[error("internal error: {0}")]
    Internal(String),
}
impl From<std::ffi::NulError> for PrologError {
    fn from(e: std::ffi::NulError) -> Self { PrologError::Conversion(e.to_string()) }
}
impl PrologError {
    pub fn to_error_atom(&self, context: &crate::atom::Atom) -> crate::atom::Atom {
        crate::atom::Atom::expr(vec![
            crate::atom::Atom::sym("Error"),
            context.clone(),
            crate::atom::Atom::str_val(&self.to_string()),
        ])
    }
}
