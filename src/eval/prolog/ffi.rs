//! Dynamic loading of `libswipl` and typed FFI bindings.
//!
//! The bindings are declared once with [`dlopen2`]'s `WrapperApi` derive,
//! which generates the symbol lookup and forwarding methods. Discovery order:
//!
//! 1. `MORK_METTA_LIBSWIPL` environment variable
//! 2. `swipl --dump-runtime-variables` → `PLLIBDIR`/`PLLIBSWIPL`
//! 3. Shared library names: `libswipl.so.10`, `libswipl.so.9`, `libswipl.so.8`,
//!    `libswipl.so`, `libswipl.dylib`

use std::ffi::{c_char, c_int, c_uint, c_void, CString};
use std::path::Path;
use std::sync::OnceLock;

use dlopen2::wrapper::{Container, WrapperApi};

use crate::eval::prolog::error::PrologError;

/// Opaque engine handle.
pub type PL_engine_t = usize;
/// Main engine constant.
pub const PL_ENGINE_MAIN: PL_engine_t = 1;
/// Engine set return status.
pub const PL_ENGINE_NONE: PL_engine_t = 0;

/// Opaque foreign frame handle.
pub type fid_t = usize;

/// Opaque term reference.
pub type term_t = usize;
/// Opaque atom handle.
pub type atom_t = usize;
/// Opaque functor handle.
pub type functor_t = usize;
/// Opaque module handle.
pub type module_t = usize;
/// Opaque predicate handle.
pub type predicate_t = usize;
/// Opaque record handle.
pub type record_t = usize;
/// Opaque query id.
pub type qid_t = usize;

/// `PL_open_query` flags.
pub const PL_Q_NORMAL: c_int = 0x0002;
pub const PL_Q_NODEBUG: c_int = 0x0004;
pub const PL_Q_CATCH_EXCEPTION: c_int = 0x0008;
pub const PL_Q_EXT_STATUS: c_int = 0x0040;

/// `PL_next_solution` return codes.
pub const PL_S_FALSE: c_int = 0;
pub const PL_S_TRUE: c_int = 1;
pub const PL_S_LAST: c_int = 2;
pub const PL_S_EXCEPTION: c_int = -1;

/// Term type tags (for `PL_term_type`).
pub const PL_VARIABLE: c_int = 1;
pub const PL_ATOM: c_int = 2;
pub const PL_INTEGER: c_int = 3;
pub const PL_FLOAT: c_int = 5;
pub const PL_STRING: c_int = 6;
pub const PL_TERM: c_int = 7;
pub const PL_NIL: c_int = 8;
pub const PL_LIST_PAIR: c_int = 10;

/// `PL_get_chars` flags.
pub const CVT_ATOM: c_uint = 0x0001;
pub const CVT_STRING: c_uint = 0x0002;
pub const CVT_INTEGER: c_uint = 0x0008;
pub const CVT_FLOAT: c_uint = 0x0010;
pub const CVT_WRITE: c_uint = 0x0080;

/// The subset of the SWI-Prolog foreign interface used by the bridge.
///
/// Only the symbols the bridge actually calls are declared; the derive macro
/// resolves them lazily and returns an error when one is missing, which keeps
/// builds and startup independent of any SWI headers or link-time library.
#[derive(WrapperApi)]
pub struct Api {
    #[dlopen2_name = "PL_initialise"]
    initialise: unsafe extern "C" fn(argc: c_int, argv: *mut *mut c_char) -> c_int,
    #[dlopen2_name = "PL_create_engine"]
    create_engine: unsafe extern "C" fn(attr: *mut c_void) -> PL_engine_t,
    #[dlopen2_name = "PL_destroy_engine"]
    destroy_engine: unsafe extern "C" fn(engine: PL_engine_t) -> c_int,
    #[dlopen2_name = "PL_set_engine"]
    set_engine: unsafe extern "C" fn(engine: PL_engine_t, old: *mut PL_engine_t) -> c_int,
    #[dlopen2_name = "PL_open_foreign_frame"]
    open_foreign_frame: unsafe extern "C" fn() -> fid_t,
    #[dlopen2_name = "PL_discard_foreign_frame"]
    discard_foreign_frame: unsafe extern "C" fn(fid: fid_t),
    #[dlopen2_name = "PL_clear_exception"]
    clear_exception: unsafe extern "C" fn(),
    #[dlopen2_name = "PL_new_term_ref"]
    new_term_ref: unsafe extern "C" fn() -> term_t,
    #[dlopen2_name = "PL_new_term_refs"]
    new_term_refs: unsafe extern "C" fn(n: c_int) -> term_t,
    #[dlopen2_name = "PL_put_atom"]
    put_atom: unsafe extern "C" fn(t: term_t, a: atom_t) -> c_int,
    #[dlopen2_name = "PL_put_atom_chars"]
    put_atom_chars: unsafe extern "C" fn(t: term_t, s: *const c_char) -> c_int,
    #[dlopen2_name = "PL_put_string_chars"]
    put_string_chars: unsafe extern "C" fn(t: term_t, s: *const c_char) -> c_int,
    #[dlopen2_name = "PL_put_int64"]
    put_int64: unsafe extern "C" fn(t: term_t, v: i64) -> c_int,
    #[dlopen2_name = "PL_put_float"]
    put_float: unsafe extern "C" fn(t: term_t, v: f64) -> c_int,
    #[dlopen2_name = "PL_put_nil"]
    put_nil: unsafe extern "C" fn(t: term_t) -> c_int,
    #[dlopen2_name = "PL_put_variable"]
    put_variable: unsafe extern "C" fn(t: term_t) -> c_int,
    #[dlopen2_name = "PL_put_term"]
    put_term: unsafe extern "C" fn(t: term_t, from: term_t) -> c_int,
    #[dlopen2_name = "PL_get_chars"]
    get_chars: unsafe extern "C" fn(t: term_t, s: *mut *const c_char, flags: c_uint) -> c_int,
    #[dlopen2_name = "PL_get_int64"]
    get_int64: unsafe extern "C" fn(t: term_t, v: *mut i64) -> c_int,
    #[dlopen2_name = "PL_get_float"]
    get_float: unsafe extern "C" fn(t: term_t, v: *mut f64) -> c_int,
    #[dlopen2_name = "PL_get_arg"]
    get_arg: unsafe extern "C" fn(index: c_int, t: term_t, a: term_t) -> c_int,
    #[dlopen2_name = "PL_get_name_arity"]
    get_name_arity: unsafe extern "C" fn(t: term_t, name: *mut atom_t, arity: *mut c_int) -> c_int,
    #[dlopen2_name = "PL_get_list"]
    get_list: unsafe extern "C" fn(t: term_t, head: term_t, tail: term_t) -> c_int,
    #[dlopen2_name = "PL_new_functor"]
    new_functor: unsafe extern "C" fn(a: atom_t, arity: c_int) -> functor_t,
    #[dlopen2_name = "PL_new_functor_sz"]
    new_functor_sz: unsafe extern "C" fn(a: atom_t, arity: usize) -> functor_t,
    #[dlopen2_name = "PL_new_atom"]
    new_atom: unsafe extern "C" fn(s: *const c_char) -> atom_t,
    #[dlopen2_name = "PL_atom_chars"]
    atom_chars: unsafe extern "C" fn(a: atom_t) -> *const c_char,
    #[dlopen2_name = "PL_predicate"]
    predicate: unsafe extern "C" fn(name: *const c_char, arity: c_int, module: *const c_char) -> predicate_t,
    #[dlopen2_name = "PL_new_module"]
    new_module: unsafe extern "C" fn(name: atom_t) -> module_t,
    #[dlopen2_name = "PL_open_query"]
    open_query: unsafe extern "C" fn(module: module_t, flags: c_int, pred: predicate_t, args: term_t) -> qid_t,
    #[dlopen2_name = "PL_next_solution"]
    next_solution: unsafe extern "C" fn(qid: qid_t) -> c_int,
    #[dlopen2_name = "PL_cut_query"]
    cut_query: unsafe extern "C" fn(qid: qid_t) -> c_int,
    #[dlopen2_name = "PL_close_query"]
    close_query: unsafe extern "C" fn(qid: qid_t) -> c_int,
    #[dlopen2_name = "PL_record"]
    record: unsafe extern "C" fn(t: term_t) -> record_t,
    #[dlopen2_name = "PL_recorded"]
    recorded: unsafe extern "C" fn(r: record_t, t: term_t) -> c_int,
    #[dlopen2_name = "PL_exception"]
    exception: unsafe extern "C" fn(qid: qid_t) -> term_t,
    #[dlopen2_name = "PL_chars_to_term"]
    chars_to_term: unsafe extern "C" fn(s: *const c_char, t: term_t) -> c_int,
    #[dlopen2_name = "PL_call"]
    call: unsafe extern "C" fn(t: term_t, module: module_t) -> c_int,
    #[dlopen2_name = "PL_term_type"]
    term_type: unsafe extern "C" fn(t: term_t) -> c_int,
    #[dlopen2_name = "PL_cons_functor_v"]
    cons_functor_v: unsafe extern "C" fn(t: term_t, f: functor_t, args: term_t) -> c_int,
    #[dlopen2_name = "PL_cons_list"]
    cons_list: unsafe extern "C" fn(t: term_t, head: term_t, tail: term_t) -> c_int,
}

static CONTAINER: OnceLock<Result<Container<Api>, PrologError>> = OnceLock::new();

impl Api {
    /// Get or initialize the global `Api` instance.
    pub fn get() -> Result<&'static Api, PrologError> {
        CONTAINER
            .get_or_init(Self::load)
            .as_ref()
            .map(|c| &**c)
            .map_err(Clone::clone)
    }

    fn load() -> Result<Container<Api>, PrologError> {
        unsafe {
            if let Ok(base) = std::env::var("PLBASE") {
                std::env::set_var("SWI_HOME_DIR", &base);
            }

            let path = Self::locate_library()?;
            let container: Container<Api> = Container::load(&path).map_err(|e| {
                PrologError::LibraryNotFound(format!("failed to load '{path}': {e}"))
            })?;

            let mut argv_vec: Vec<*mut c_char> = vec![
                CString::new("swipl").unwrap().into_raw(),
                CString::new("-q").unwrap().into_raw(),
                CString::new("--nosignals").unwrap().into_raw(),
            ];
            if let Ok(swi_home) = std::env::var("SWI_HOME_DIR") {
                argv_vec.push(
                    CString::new(format!("--home={swi_home}"))
                        .unwrap()
                        .into_raw(),
                );
            }
            argv_vec.push(std::ptr::null_mut());

            let rc = container.initialise(
                (argv_vec.len() - 1) as c_int,
                argv_vec.as_ptr() as *mut *mut c_char,
            );
            if rc == 0 {
                return Err(PrologError::InitFailed("PL_initialise returned FALSE".into()));
            }
            // Release the calling thread from the main engine so any thread can
            // attach to it later (under a lock).
            container.set_engine(PL_ENGINE_NONE, std::ptr::null_mut());
            Ok(container)
        }
    }

    /// Locate `libswipl` and return a path (or bare name) for the loader.
    fn locate_library() -> Result<String, PrologError> {
        // 1. Env override
        if let Ok(path) = std::env::var("MORK_METTA_LIBSWIPL") {
            // Derive PLBASE from the library path, e.g.
            // /home/u/.local/swipl/swipl/lib/x86_64-linux/libswipl.so.10
            //   -> PLBASE = /home/u/.local/swipl/swipl
            if let Some(plbase) = Path::new(&path)
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
            {
                unsafe { std::env::set_var("PLBASE", plbase) };
            }
            return Ok(path);
        }

        // 2. swipl --dump-runtime-variables
        let home_bin = std::env::var("HOME")
            .map(|h| format!("{h}/.local/bin/swipl"))
            .unwrap_or_default();
        for swipl_bin in ["swipl", &home_bin, "/usr/bin/swipl", "/usr/local/bin/swipl"] {
            let Ok(output) = std::process::Command::new(swipl_bin)
                .arg("--dump-runtime-variables")
                .output()
            else {
                continue;
            };
            if !output.status.success() {
                continue;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut plbase = None;
            let mut pllibdir = None;
            let mut pllibswipl = None;
            for line in stdout.lines() {
                let clean = line.trim();
                let unquote = |v: &str| {
                    v.trim_matches(|c| c == '"' || c == ';' || c == '\'')
                        .trim()
                        .to_string()
                };
                if let Some(v) = clean.strip_prefix("PLBASE=") {
                    plbase = Some(unquote(v));
                } else if let Some(v) = clean.strip_prefix("PLLIBDIR=") {
                    pllibdir = Some(unquote(v));
                } else if let Some(v) = clean.strip_prefix("PLLIBSWIPL=") {
                    pllibswipl = Some(unquote(v));
                }
            }
            if let Some(base) = plbase {
                unsafe {
                    std::env::set_var("SWI_HOME_DIR", &base);
                    std::env::set_var("PLBASE", &base);
                }
            }
            if let Some(libswipl) = pllibswipl {
                return Ok(libswipl);
            }
            if let Some(libdir) = pllibdir {
                let candidates = Self::library_names()
                    .iter()
                    .map(|name| format!("{libdir}/{name}"));
                return Self::first_existing(candidates)
                    .ok_or_else(|| Self::not_found(&libdir));
            }
        }

        // 3. Common names via the system loader.
        Self::first_existing(Self::library_names().iter().map(|n| n.to_string()))
            .ok_or_else(|| {
                PrologError::LibraryNotFound(
                    "could not locate libswipl (tried env MORK_METTA_LIBSWIPL, \
                     swipl --dump-runtime-variables, and common shared library names)"
                        .into(),
                )
            })
    }

    fn library_names() -> &'static [&'static str] {
        &[
            "libswipl.so.10",
            "libswipl.so.9",
            "libswipl.so.8",
            "libswipl.so",
            "libswipl.dylib",
        ]
    }

    /// Return the first candidate that can actually be loaded (or, for bare
    /// names, the first one — the system loader resolves it).
    fn first_existing<I: Iterator<Item = String>>(candidates: I) -> Option<String> {
        let mut bare = None;
        for cand in candidates {
            if Path::new(&cand).is_file() {
                return Some(cand);
            }
            if !cand.contains('/') {
                bare = Some(cand);
            }
        }
        bare
    }

    fn not_found(dir: &str) -> PrologError {
        PrologError::LibraryNotFound(format!("could not locate libswipl in {dir}"))
    }
}
