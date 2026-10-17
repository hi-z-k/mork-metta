//! Lazy solution stream for non-deterministic Prolog queries.
//!
//! A [`PrologStream`] owns the pooled engine, foreign frame and open query for
//! one goal. It is `Send`, so a Rayon worker may pull from it on a different
//! thread than the one that created it: each `next()` attaches the owned engine
//! to the calling thread for the duration of the step, then detaches it.
//! Dropping the stream closes the query, discards the frame and returns the
//! engine to the pool.

use crate::atom::Atom;
use crate::eval::prolog::engine::PooledEngine;
use crate::eval::prolog::ffi::{Api, fid_t, qid_t, term_t};
use crate::eval::prolog::ops::{self, StepStreamResult};

/// Iterator yielding solutions from an active Prolog query.
pub struct PrologStream {
    engine: PooledEngine,
    qid: qid_t,
    frame: fid_t,
    result: term_t,
    first: Option<Atom>,
    done: bool,
}

impl PrologStream {
    pub(crate) fn new(
        engine: PooledEngine,
        qid: qid_t,
        frame: fid_t,
        result: term_t,
        first: Option<Atom>,
    ) -> Self {
        PrologStream {
            engine,
            qid,
            frame,
            result,
            first,
            done: false,
        }
    }

    /// Close the query and discard the frame, attaching the owned engine.
    fn finish(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        if Api::get().is_ok() {
            let _ = crate::eval::prolog::engine::with_engine(self.engine.engine(), |api| unsafe {
                api.close_query(self.qid);
                api.discard_foreign_frame(self.frame);
            });
        }
    }
}

impl Iterator for PrologStream {
    type Item = Atom;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(answer) = self.first.take() {
            return Some(answer);
        }
        if self.done {
            return None;
        }

        if Api::get().is_err() {
            self.done = true;
            return None;
        }
        let stepped = crate::eval::prolog::engine::with_engine(self.engine.engine(), |api| {
            ops::step_query(api, self.qid, self.result)
        });

        match stepped {
            Ok(StepStreamResult::Answer(atom, is_last)) => {
                if is_last {
                    self.finish();
                }
                Some(atom)
            }
            Ok(StepStreamResult::Done) => {
                self.finish();
                None
            }
            Ok(StepStreamResult::Error(err)) => {
                self.finish();
                Some(err.to_error_atom(&Atom::sym("prolog_stream")))
            }
            Err(err) => {
                self.finish();
                Some(err.to_error_atom(&Atom::sym("prolog_stream")))
            }
        }
    }
}

impl Drop for PrologStream {
    fn drop(&mut self) {
        self.finish();
    }
}
