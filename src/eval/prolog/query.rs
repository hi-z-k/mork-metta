//! Prolog query lifecycle: open, step, and close with extended status.

use std::ffi::{c_char, c_int, CStr};

use crate::eval::prolog::error::PrologError;
use crate::eval::prolog::ffi::{
    Api, module_t, predicate_t, qid_t, term_t, CVT_WRITE, PL_Q_CATCH_EXCEPTION,
    PL_Q_EXT_STATUS, PL_Q_NODEBUG, PL_S_EXCEPTION, PL_S_FALSE, PL_S_LAST, PL_S_TRUE,
};

/// Result of stepping a Prolog query with `PL_Q_EXT_STATUS`.
#[derive(Debug)]
pub enum StepStatus {
    /// Solution available and there may be more (choicepoint open).
    True,
    /// Solution available and this is the last one (choicepoint closed).
    Last,
    /// No more solutions (query failed/completed).
    False,
    /// Uncaught Prolog exception occurred.
    Exception(String),
}

/// Represents an active Prolog query.
pub struct Query {
    qid: qid_t,
}

impl Query {
    /// Open a query with extended status and exception catching.
    pub fn open(
        api: &Api,
        module: module_t,
        predicate: predicate_t,
        args: term_t,
    ) -> Result<Self, PrologError> {
        let flags = PL_Q_CATCH_EXCEPTION | PL_Q_EXT_STATUS | PL_Q_NODEBUG;
        let qid = unsafe { api.open_query(module, flags, predicate, args) };
        if qid == 0 {
            return Err(PrologError::Internal("PL_open_query returned NULL".into()));
        }
        Ok(Self { qid })
    }

    /// Step to the next solution.
    pub fn step(&self, api: &Api) -> StepStatus {
        let rc = unsafe { api.next_solution(self.qid) };
        match rc {
            PL_S_LAST => StepStatus::Last,
            PL_S_TRUE => StepStatus::True,
            PL_S_FALSE => StepStatus::False,
            PL_S_EXCEPTION => {
                let ex = unsafe { api.exception(self.qid) };
                let mut ptr: *const c_char = std::ptr::null();
                let msg = if unsafe { api.get_chars(ex, &mut ptr, CVT_WRITE) } != 0 && !ptr.is_null() {
                    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
                } else {
                    "unknown Prolog exception".to_string()
                };
                unsafe { api.clear_exception() };
                StepStatus::Exception(msg)
            }
            _ => StepStatus::False,
        }
    }

    /// Cut remaining choicepoints without closing the query.
    pub fn cut(&self, api: &Api) {
        unsafe { api.cut_query(self.qid) };
    }

    /// Close the query.
    pub fn close(self, api: &Api) {
        unsafe { api.close_query(self.qid) };
    }

    /// Get raw query id.
    pub fn qid(&self) -> qid_t {
        self.qid
    }
}
