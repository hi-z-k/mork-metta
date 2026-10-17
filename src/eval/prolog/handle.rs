//! Opaque handle for foreign Prolog terms (`PrologForeign`).

use std::any::Any;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::atom::Grounded;
use crate::eval::prolog::ffi::record_t;

/// Global counter for generating unique foreign term IDs.
static FOREIGN_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// Opaque handle for a Prolog term recorded with `PL_record`.
/// Implements `Grounded` so it can be stored in the MeTTa atomspace.
#[derive(Debug)]
pub struct PrologForeign {
    /// The recorded term handle from SWI-Prolog.
    pub record: record_t,
    /// Unique ID for this foreign term (for equality/hashing).
    id: usize,
}

impl PrologForeign {
    /// Create a new `PrologForeign` from a recorded term handle.
    pub fn new(record: record_t) -> Self {
        let id = FOREIGN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        PrologForeign { record, id }
    }
}

impl Grounded for PrologForeign {
    fn display_metta(&self) -> String {
        format!("<prolog-foreign:#{}:{}>", self.id, self.record)
    }

    fn eq_gnd(&self, other: &dyn Grounded) -> bool {
        if let Some(other) = other.as_any().downcast_ref::<PrologForeign>() {
            self.record == other.record
        } else {
            false
        }
    }

    fn hash_gnd(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.record.hash(&mut hasher);
        hasher.finish()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl PartialEq for PrologForeign {
    fn eq(&self, other: &Self) -> bool {
        self.record == other.record
    }
}

impl Eq for PrologForeign {}

impl Hash for PrologForeign {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.record.hash(state);
    }
}

impl fmt::Display for PrologForeign {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_metta())
    }
}
