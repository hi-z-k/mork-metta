//! Lazy solution stream for non-deterministic Prolog queries (`PrologStream`).

use crate::atom::Atom;
use crate::eval::prolog::thread::{client, PrologCommand, PrologReply};

/// Iterator yielding solutions from an active Prolog query.
/// Implements `Send + 'static` so it can be passed to `NDet::stream`.
pub struct PrologStream {
    stream_id: usize,
    first_answer: Option<Atom>,
    exhausted: bool,
}

impl PrologStream {
    /// Create a new `PrologStream`.
    pub fn new(stream_id: usize, first_answer: Option<Atom>) -> Self {
        PrologStream {
            stream_id,
            first_answer,
            exhausted: false,
        }
    }
}

impl Iterator for PrologStream {
    type Item = Atom;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(ans) = self.first_answer.take() {
            return Some(ans);
        }
        if self.exhausted {
            return None;
        }

        let client = client().ok()?;
        match client.request(PrologCommand::NextAnswer {
            stream_id: self.stream_id,
        }) {
            Ok(PrologReply::NextAnswer(Some(atom))) => Some(atom),
            Ok(PrologReply::NextAnswer(None)) => {
                self.exhausted = true;
                None
            }
            Ok(PrologReply::Error(err)) => {
                self.exhausted = true;
                Some(err.to_error_atom(&Atom::sym("prolog_stream")))
            }
            _ => {
                self.exhausted = true;
                None
            }
        }
    }
}

impl Drop for PrologStream {
    fn drop(&mut self) {
        if !self.exhausted {
            self.exhausted = true;
            if let Ok(client) = client() {
                let _ = client.notify(PrologCommand::CloseStream {
                    stream_id: self.stream_id,
                });
            }
        }
    }
}
