//! Engine manager: main engine + pooled engines with shared tabling.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use crate::eval::prolog::error::PrologError;
use crate::eval::prolog::ffi::{Api, PL_engine_t, init_prolog, term_t};

/// Maximum number of idle engines to keep in the pool.
const MAX_IDLE_ENGINES: usize = 8;

/// Engine manager handling main engine and pooled engines.
pub struct EngineManager {
    main_engine: PL_engine_t,
    /// Pool of idle engines available for query execution.
    idle_engines: Mutex<VecDeque<PL_engine_t>>,
    /// Tracks which engines are currently checked out (for debugging).
    checked_out: Mutex<usize>,
}

fn set_table_shared(api: &Api) {
    unsafe {
        let frame = api.open_foreign_frame();
        let p_set_flag = api.predicate(
            b"set_prolog_flag\0".as_ptr() as *const _,
            2,
            b"system\0".as_ptr() as *const _,
        );
        let targs = api.new_term_refs(2);
        api.put_atom_chars(targs, b"table_shared\0".as_ptr() as *const _);
        api.put_atom_chars(targs + 1, b"true\0".as_ptr() as *const _);
        let q = api.open_query(0, crate::eval::prolog::ffi::PL_Q_NORMAL, p_set_flag, targs);
        if q != 0 {
            api.next_solution(q);
            api.close_query(q);
        }
        api.discard_foreign_frame(frame);
    }
}

impl EngineManager {
    /// Get or initialize the global engine manager.
    pub fn get() -> Result<Arc<EngineManager>, PrologError> {
        static MANAGER: OnceLock<Result<Arc<EngineManager>, PrologError>> = OnceLock::new();
        MANAGER.get_or_init(|| Self::new()).as_ref().map_err(Clone::clone).cloned()
    }

    fn new() -> Result<Arc<EngineManager>, PrologError> {
        // Initialize Prolog (loads library, calls PL_initialise)
        let _guard = init_prolog()?;

        let api = Api::get()?;

        let main_engine = crate::eval::prolog::ffi::PL_ENGINE_MAIN;

        // Enable shared tabling on the main engine
        let mut old_engine = 0;
        unsafe { api.set_engine(main_engine, &mut old_engine) };
        set_table_shared(api);
        unsafe { api.set_engine(old_engine, std::ptr::null_mut()) };

        let manager = Arc::new(EngineManager {
            main_engine,
            idle_engines: Mutex::new(VecDeque::new()),
            checked_out: Mutex::new(0),
        });

        Ok(manager)
    }

    /// Create a new Prolog engine with shared tabling enabled.
    fn create_engine(&self) -> Result<PL_engine_t, PrologError> {
        let api = Api::get()?;
        let engine = unsafe { api.create_engine(std::ptr::null_mut()) };
        if engine == 0 {
            return Err(PrologError::Internal("PL_create_engine returned NULL".into()));
        }

        // Enable shared tabling on the new engine
        let mut old_engine: PL_engine_t = 0;
        unsafe { api.set_engine(engine, &mut old_engine) };
        set_table_shared(api);
        unsafe { api.set_engine(old_engine, std::ptr::null_mut()) };

        Ok(engine)
    }

    /// Get the main engine (for mutations like assert/retract).
    pub fn main_engine(&self) -> PL_engine_t {
        self.main_engine
    }

    /// Check out an engine from the pool for query execution.
    /// Returns a `PooledEngine` that returns the engine to the pool on drop.
    pub fn checkout(self: &Arc<Self>) -> Result<PooledEngine, PrologError> {
        let mut idle = self.idle_engines.lock().unwrap();
        let engine = if let Some(engine) = idle.pop_front() {
            engine
        } else {
            self.create_engine()?
        };
        *self.checked_out.lock().unwrap() += 1;
        Ok(PooledEngine {
            manager: Arc::clone(self),
            engine: Some(engine),
        })
    }

    /// Return an engine to the pool.
    fn return_engine(&self, engine: PL_engine_t) {
        let mut idle = self.idle_engines.lock().unwrap();
        *self.checked_out.lock().unwrap() -= 1;
        if idle.len() < MAX_IDLE_ENGINES {
            idle.push_back(engine);
        } else {
            // Pool is full, destroy the excess engine
            let api = Api::get().unwrap();
            unsafe { api.destroy_engine(engine) };
        }
    }

    /// Get the number of currently checked out engines.
    pub fn checked_out_count(&self) -> usize {
        *self.checked_out.lock().unwrap()
    }

    /// Get the number of idle engines in the pool.
    pub fn idle_count(&self) -> usize {
        self.idle_engines.lock().unwrap().len()
    }
}

/// RAII handle for a checked-out pooled engine.
pub struct PooledEngine {
    manager: Arc<EngineManager>,
    engine: Option<PL_engine_t>,
}

impl PooledEngine {
    /// Get the engine handle.
    pub fn engine(&self) -> PL_engine_t {
        self.engine.unwrap()
    }

    /// Manually return the engine to the pool early.
    pub fn return_early(mut self) {
        if let Some(engine) = self.engine.take() {
            self.manager.return_engine(engine);
        }
    }
}

impl Drop for PooledEngine {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            self.manager.return_engine(engine);
        }
    }
}