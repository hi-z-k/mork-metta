use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::eval::prolog::error::PrologError;
use crate::eval::prolog::ffi::{Api, PL_ENGINE_MAIN, PL_Q_NORMAL, PL_engine_t};

pub const MAX_IDLE_ENGINES: usize = 8;

pub struct EngineManager {
    main_lock: Mutex<()>,
    idle: Mutex<Vec<PL_engine_t>>,
    checked_out: AtomicUsize,
}

pub fn manager() -> Result<&'static Arc<EngineManager>, PrologError> {
    static MANAGER: OnceLock<Result<Arc<EngineManager>, PrologError>> = OnceLock::new();
    MANAGER
        .get_or_init(|| {
            let manager = EngineManager::new()?;
            crate::eval::prolog::ops::init_support(&manager)?;
            Ok(manager)
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub fn with_engine<R>(engine: PL_engine_t, f: impl FnOnce(&Api) -> R) -> Result<R, PrologError> {
    let api = Api::get()?;
    let mut old: PL_engine_t = 0;
    unsafe { api.set_engine(engine, &mut old) };
    let result = f(api);
    unsafe { api.set_engine(old, std::ptr::null_mut()) };
    Ok(result)
}

impl EngineManager {
    pub fn get() -> Result<Arc<EngineManager>, PrologError> {
        manager().map(Arc::clone)
    }

    fn new() -> Result<Arc<Self>, PrologError> {
        let api = Api::get()?;
        let mut old: PL_engine_t = 0;
        unsafe { api.set_engine(PL_ENGINE_MAIN, &mut old) };
        set_table_shared(api);
        unsafe { api.set_engine(old, std::ptr::null_mut()) };
        Ok(Arc::new(EngineManager {
            main_lock: Mutex::new(()),
            idle: Mutex::new(Vec::new()),
            checked_out: AtomicUsize::new(0),
        }))
    }

    pub fn with_main<R>(&self, f: impl FnOnce(&Api) -> R) -> Result<R, PrologError> {
        let _guard = self.main_lock.lock().unwrap();
        with_engine(PL_ENGINE_MAIN, f)
    }

    fn create_engine(&self) -> Result<PL_engine_t, PrologError> {
        let api = Api::get()?;
        let engine = unsafe { api.create_engine(std::ptr::null_mut()) };
        if engine == 0 {
            return Err(PrologError::Internal("PL_create_engine returned NULL".into()));
        }
        let mut old: PL_engine_t = 0;
        unsafe { api.set_engine(engine, &mut old) };
        set_table_shared(api);
        unsafe { api.set_engine(old, std::ptr::null_mut()) };
        Ok(engine)
    }

    pub fn checkout(self: &Arc<Self>) -> Result<PooledEngine, PrologError> {
        let engine = match self.idle.lock().unwrap().pop() {
            Some(engine) => engine,
            None => self.create_engine()?,
        };
        self.checked_out.fetch_add(1, Ordering::Relaxed);
        Ok(PooledEngine { manager: Arc::clone(self), engine: Some(engine) })
    }

    fn return_engine(&self, engine: PL_engine_t) {
        self.checked_out.fetch_sub(1, Ordering::Relaxed);
        let mut idle = self.idle.lock().unwrap();
        if idle.len() < MAX_IDLE_ENGINES {
            idle.push(engine);
        } else if let Ok(api) = Api::get() {
            unsafe { api.destroy_engine(engine) };
        }
    }

    pub fn checked_out_count(&self) -> usize {
        self.checked_out.load(Ordering::Relaxed)
    }

    pub fn idle_count(&self) -> usize {
        self.idle.lock().unwrap().len()
    }
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
        let q = api.open_query(0, PL_Q_NORMAL, p_set_flag, targs);
        if q != 0 {
            api.next_solution(q);
            api.close_query(q);
        }
        api.discard_foreign_frame(frame);
    }
}

pub struct PooledEngine {
    manager: Arc<EngineManager>,
    engine: Option<PL_engine_t>,
}

impl PooledEngine {
    pub fn engine(&self) -> PL_engine_t {
        self.engine.unwrap()
    }

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
