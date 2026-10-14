//! Dedicated Prolog worker thread and command channel.

use std::collections::HashMap;
use std::sync::{mpsc, Arc, OnceLock};
use std::thread;

use crate::atom::Atom;
use crate::eval::prolog::engine::EngineManager;
use crate::eval::prolog::error::PrologError;
use crate::eval::prolog::ffi::{init_prolog, Api};
use crate::eval::prolog::ops::{
    self, ActiveStream, FunctionOutcome, StepStreamResult,
};

/// High-level commands sent to the dedicated Prolog worker thread.
#[derive(Debug)]
pub enum PrologCommand {
    /// Ping the worker thread to check liveness.
    Ping,
    /// Call a predicate goal `call(Goal)` and yield True per solution.
    CallPredicate {
        goal: Atom,
    },
    /// Call an imported function `functor(args..., Result)`.
    CallFunction {
        functor: String,
        args: Vec<Atom>,
    },
    /// Step an open query stream for next answer.
    NextAnswer {
        stream_id: usize,
    },
    /// Close an open query stream and return engine to pool.
    CloseStream {
        stream_id: usize,
    },
    /// Add a clause at the front (`asserta/1`).
    Asserta {
        clause: Atom,
    },
    /// Add a clause at the back (`assertz/1`).
    Assertz {
        clause: Atom,
    },
    /// Remove first matching clause (`retract/1`).
    Retract {
        clause: Atom,
    },
    /// Consult a Prolog file.
    Consult {
        path: String,
    },
    /// Load a snippet of Prolog code directly into the user module.
    LoadCode {
        code: String,
    },
    /// Find all defined arities for a predicate.
    CurrentArities {
        name: String,
    },
    /// Execute translatePredicate with input bindings and extract solution bindings.
    Translate {
        goal: Atom,
        in_bindings: Vec<(String, Atom)>,
    },
    /// Sync space atoms into Prolog dynamic clauses for `'&self'`.
    SyncSpace {
        atoms: Vec<Atom>,
    },
    /// Shutdown the worker thread.
    Shutdown,
}

/// Replies returned from the Prolog worker thread.
#[derive(Debug)]
pub enum PrologReply {
    Pong,
    Bool(bool),
    Atom(Atom),
    StreamOpened {
        stream_id: usize,
        first_answer: Option<Atom>,
    },
    NextAnswer(Option<Atom>),
    Solutions(Vec<Atom>),
    TranslateSolutions(Vec<Vec<(String, Atom)>>),
    Arities(Vec<usize>),
    Error(PrologError),
    Ack,
}

/// Client handle for communicating with the Prolog worker thread.
pub struct PrologClient {
    sender: mpsc::Sender<(PrologCommand, Option<mpsc::Sender<PrologReply>>)>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl PrologClient {
    /// Send a command and wait synchronously for a reply.
    pub fn request(&self, cmd: PrologCommand) -> Result<PrologReply, PrologError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.sender
            .send((cmd, Some(reply_tx)))
            .map_err(|_| PrologError::ThreadGone)?;
        reply_rx.recv().map_err(|_| PrologError::ThreadGone)
    }

    /// Send a fire-and-forget notification (does not wait for reply).
    pub fn notify(&self, cmd: PrologCommand) -> Result<(), PrologError> {
        self.sender
            .send((cmd, None))
            .map_err(|_| PrologError::ThreadGone)
    }

    /// Send a ping to check if the worker is alive.
    pub fn ping(&self) -> Result<(), PrologError> {
        match self.request(PrologCommand::Ping) {
            Ok(PrologReply::Pong) => Ok(()),
            Ok(PrologReply::Error(e)) => Err(e),
            _ => Err(PrologError::ThreadGone),
        }
    }

    /// Load a snippet of Prolog code directly on the worker thread.
    pub fn load_code(&self, code: &str) -> Result<(), PrologError> {
        match self.request(PrologCommand::LoadCode {
            code: code.to_string(),
        }) {
            Ok(PrologReply::Ack) => Ok(()),
            Ok(PrologReply::Error(e)) => Err(e),
            Err(e) => Err(e),
            _ => Err(PrologError::Internal("unexpected reply to load_code".into())),
        }
    }

    /// Shutdown the worker thread.
    pub fn shutdown(mut self) -> Result<(), PrologError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.sender
            .send((PrologCommand::Shutdown, Some(reply_tx)))
            .map_err(|_| PrologError::ThreadGone)?;
        reply_rx.recv().map_err(|_| PrologError::ThreadGone)?;

        if let Some(handle) = self.thread_handle.take() {
            handle.join().map_err(|_| PrologError::ThreadGone)?;
        }
        Ok(())
    }
}

impl Drop for PrologClient {
    fn drop(&mut self) {
        let _ = self.sender.send((PrologCommand::Shutdown, None));
    }
}

/// Global client instance, initialized once.
static CLIENT: OnceLock<Result<PrologClient, PrologError>> = OnceLock::new();

/// Get or initialize the global Prolog client.
pub fn client() -> Result<&'static PrologClient, PrologError> {
    CLIENT.get_or_init(|| start_worker()).as_ref().map_err(Clone::clone)
}

/// Start the Prolog worker thread and return a client.
fn start_worker() -> Result<PrologClient, PrologError> {
    let _guard = init_prolog()?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<(PrologCommand, Option<mpsc::Sender<PrologReply>>)>();

    let thread_handle = thread::Builder::new()
        .name("mork-prolog".to_string())
        .spawn(move || {
            worker_loop(cmd_rx);
        })
        .map_err(|e| PrologError::Internal(format!("failed to spawn prolog worker: {e}")))?;

    Ok(PrologClient {
        sender: cmd_tx,
        thread_handle: Some(thread_handle),
    })
}

/// Main loop for the Prolog worker thread.
fn worker_loop(cmd_rx: mpsc::Receiver<(PrologCommand, Option<mpsc::Sender<PrologReply>>)>) {
    let api = match Api::get() {
        Ok(a) => a,
        Err(e) => {
            for (_, reply_tx) in cmd_rx {
                if let Some(tx) = reply_tx {
                    let _ = tx.send(PrologReply::Error(e.clone()));
                }
            }
            return;
        }
    };

    // Attach mork-prolog worker thread to the main engine
    unsafe {
        let rc = (api.set_engine)(crate::eval::prolog::ffi::PL_ENGINE_MAIN, std::ptr::null_mut());
        if rc != crate::eval::prolog::ffi::PL_ENGINE_SET {
            eprintln!("Warning: could not attach mork-prolog worker thread to PL_ENGINE_MAIN, rc={rc}");
        }
    }

    let manager = match EngineManager::get() {
        Ok(m) => m,
        Err(e) => {
            for (_, reply_tx) in cmd_rx {
                if let Some(tx) = reply_tx {
                    let _ = tx.send(PrologReply::Error(e.clone()));
                }
            }
            return;
        }
    };

    // Load standard arithmetic support and space definitions
    if let Err(e) = ops::init_support(api, &manager) {
        eprintln!("Warning: could not initialize Prolog support clauses: {e}");
    }

    let mut active_streams: HashMap<usize, ActiveStream> = HashMap::new();
    let mut next_stream_id: usize = 1;

    for (cmd, reply_tx) in cmd_rx {
        let is_shutdown = matches!(cmd, PrologCommand::Shutdown);
        let result = match cmd {
            PrologCommand::Ping => PrologReply::Pong,
            PrologCommand::CallPredicate { goal } => {
                match ops::call_predicate(api, &manager, &goal) {
                    Ok(solutions) => PrologReply::Solutions(solutions),
                    Err(e) => PrologReply::Error(e),
                }
            }
            PrologCommand::CallFunction { functor, args } => {
                match ops::call_function(
                    api,
                    &manager,
                    &functor,
                    &args,
                    &mut active_streams,
                    &mut next_stream_id,
                ) {
                    Ok(FunctionOutcome::Single(atom)) => PrologReply::Atom(atom),
                    Ok(FunctionOutcome::Stream {
                        stream_id,
                        first_answer,
                    }) => PrologReply::StreamOpened {
                        stream_id,
                        first_answer,
                    },
                    Ok(FunctionOutcome::Empty) => PrologReply::Solutions(Vec::new()),
                    Ok(FunctionOutcome::Exception(msg)) => {
                        PrologReply::Error(PrologError::Exception(msg))
                    }
                    Err(e) => PrologReply::Error(e),
                }
            }
            PrologCommand::NextAnswer { stream_id } => {
                if let Some(mut stream) = active_streams.remove(&stream_id) {
                    match ops::step_stream(api, &mut stream) {
                        StepStreamResult::Answer(atom, is_last) => {
                            if !is_last {
                                active_streams.insert(stream_id, stream);
                            } else {
                                ops::close_stream(api, stream);
                            }
                            PrologReply::NextAnswer(Some(atom))
                        }
                        StepStreamResult::Done => {
                            ops::close_stream(api, stream);
                            PrologReply::NextAnswer(None)
                        }
                        StepStreamResult::Error(e) => {
                            ops::close_stream(api, stream);
                            PrologReply::Error(e)
                        }
                    }
                } else {
                    PrologReply::NextAnswer(None)
                }
            }
            PrologCommand::CloseStream { stream_id } => {
                if let Some(stream) = active_streams.remove(&stream_id) {
                    ops::close_stream(api, stream);
                }
                PrologReply::Ack
            }
            PrologCommand::Asserta { clause } => {
                match ops::asserta(api, &manager, &clause) {
                    Ok(()) => PrologReply::Bool(true),
                    Err(e) => PrologReply::Error(e),
                }
            }
            PrologCommand::Assertz { clause } => {
                match ops::assertz(api, &manager, &clause) {
                    Ok(()) => PrologReply::Bool(true),
                    Err(e) => PrologReply::Error(e),
                }
            }
            PrologCommand::Retract { clause } => {
                match ops::retract(api, &manager, &clause) {
                    Ok(removed) => PrologReply::Bool(removed),
                    Err(e) => PrologReply::Error(e),
                }
            }
            PrologCommand::Consult { path } => {
                match ops::consult(api, &manager, &path) {
                    Ok(()) => PrologReply::Ack,
                    Err(e) => PrologReply::Error(e),
                }
            }
            PrologCommand::LoadCode { code } => {
                match ops::load_prolog_code(api, &manager, &code) {
                    Ok(()) => PrologReply::Ack,
                    Err(e) => PrologReply::Error(e),
                }
            }
            PrologCommand::CurrentArities { name } => {
                match ops::current_arities(api, &manager, &name) {
                    Ok(arities) => PrologReply::Arities(arities),
                    Err(e) => PrologReply::Error(e),
                }
            }
            PrologCommand::Translate { goal, in_bindings } => {
                match ops::translate(api, &manager, &goal, &in_bindings) {
                    Ok(solutions) => PrologReply::TranslateSolutions(solutions),
                    Err(e) => PrologReply::Error(e),
                }
            }
            PrologCommand::SyncSpace { atoms } => {
                match ops::sync_space(api, &manager, &atoms) {
                    Ok(()) => PrologReply::Ack,
                    Err(e) => PrologReply::Error(e),
                }
            }
            PrologCommand::Shutdown => {
                for (_, stream) in active_streams.drain() {
                    ops::close_stream(api, stream);
                }
                PrologReply::Ack
            }
        };

        if let Some(tx) = reply_tx {
            let _ = tx.send(result);
        }

        if is_shutdown {
            break;
        }
    }
}