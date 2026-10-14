//! Dynamic loading of `libswipl` and safe FFI bindings.
//!
//! Discovery order:
//! 1. `MORK_METTA_LIBSWIPL` environment variable
//! 2. `swipl --dump-runtime-variables` → `PLLIBDIR`/`PLLIBSWIPL`
//! 3. Shared library names: `libswipl.so.10`, `libswipl.so.9`, `libswipl.so.8`, `libswipl.so`, `libswipl.dylib`

use std::ffi::{c_char, c_int, c_long, c_uint, c_void, CString};
use std::path::Path;
use std::sync::OnceLock;

use libloading::{Library, Symbol};

use crate::eval::prolog::error::PrologError;

/// Opaque engine handle.
pub type PL_engine_t = usize;
/// Main engine constant.
pub const PL_ENGINE_MAIN: PL_engine_t = 1;
pub const PL_ENGINE_CURRENT: PL_engine_t = 2;
pub const PL_ENGINE_NONE: PL_engine_t = 0;
/// Engine set return status.
pub const PL_ENGINE_SET: c_int = 0;

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
pub const PL_S_NOT_INNER: c_int = -2;

/// Term type tags (for `PL_term_type`).
pub const PL_VARIABLE: c_int = 1;
pub const PL_ATOM: c_int = 2;
pub const PL_INTEGER: c_int = 3;
pub const PL_FLOAT: c_int = 5;
pub const PL_STRING: c_int = 6;
pub const PL_TERM: c_int = 7;
pub const PL_NIL: c_int = 8;
pub const PL_LIST_PAIR: c_int = 10;
pub const PL_FUNCTOR: c_int = 11;

/// `PL_get_chars` flags.
pub const CVT_ATOM: c_uint = 0x0001;
pub const CVT_STRING: c_uint = 0x0002;
pub const CVT_LIST: c_uint = 0x0004;
pub const CVT_INTEGER: c_uint = 0x0008;
pub const CVT_FLOAT: c_uint = 0x0010;
pub const CVT_NUMBER: c_uint = CVT_INTEGER | CVT_FLOAT;
pub const CVT_ATOMIC: c_uint = CVT_ATOM | CVT_NUMBER | CVT_STRING;
pub const CVT_WRITE: c_uint = 0x0080;
pub const CVT_WRITE_CANONICAL: c_uint = 0x0100;
pub const CVT_WRITEQ: c_uint = 0x0200;
pub const CVT_ALL: c_uint = 0x00FF;
pub const CVT_VARIABLE: c_uint = 0x0400;
pub const CVT_EXCEPTION: c_uint = 0x0800;

/// `PL_unify` flags.
pub const PL_UNIFY_NORMAL: c_int = 0;

/// Foreign function flag: variable arity.
pub const PL_FA_VARARGS: c_int = 0x0001;

/// Prolog flag names.
pub const TABLE_SHARED_FLAG: &str = "table_shared";

/// Global API instance, initialized once.
static API: OnceLock<Result<Api, PrologError>> = OnceLock::new();
static LIBRARY: OnceLock<Library> = OnceLock::new();

/// Safe wrapper around the loaded `libswipl` function pointers.
pub struct Api {
    // Function pointers
    pub initialise: unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int,
    pub cleanup: unsafe extern "C" fn(c_int) -> c_int,
    pub create_engine: unsafe extern "C" fn(*mut c_void) -> PL_engine_t,
    pub destroy_engine: unsafe extern "C" fn(PL_engine_t) -> c_int,
    pub set_engine: unsafe extern "C" fn(PL_engine_t, *mut PL_engine_t) -> c_int,
    pub current_engine: unsafe extern "C" fn() -> PL_engine_t,
    pub open_foreign_frame: unsafe extern "C" fn() -> fid_t,
    pub close_foreign_frame: unsafe extern "C" fn(fid_t),
    pub discard_foreign_frame: unsafe extern "C" fn(fid_t),
    pub clear_exception: unsafe extern "C" fn(),
    pub new_term_ref: unsafe extern "C" fn() -> term_t,
    pub new_term_refs: unsafe extern "C" fn(c_int) -> term_t,
    pub reset_term_refs: unsafe extern "C" fn(term_t) -> c_int,
    pub put_atom: unsafe extern "C" fn(term_t, atom_t) -> c_int,
    pub put_atom_chars: unsafe extern "C" fn(term_t, *const c_char) -> c_int,
    pub put_string_chars: unsafe extern "C" fn(term_t, *const c_char) -> c_int,
    pub put_integer: unsafe extern "C" fn(term_t, c_long) -> c_int,
    pub put_int64: unsafe extern "C" fn(term_t, i64) -> c_int,
    pub put_float: unsafe extern "C" fn(term_t, f64) -> c_int,
    pub put_functor: unsafe extern "C" fn(term_t, functor_t) -> c_int,
    pub put_list: unsafe extern "C" fn(term_t) -> c_int,
    pub put_nil: unsafe extern "C" fn(term_t) -> c_int,
    pub put_variable: unsafe extern "C" fn(term_t) -> c_int,
    pub put_term: unsafe extern "C" fn(term_t, term_t) -> c_int,
    pub get_atom: unsafe extern "C" fn(term_t, *mut atom_t) -> c_int,
    pub get_atom_chars: unsafe extern "C" fn(atom_t, *mut *const c_char) -> c_int,
    pub get_chars: unsafe extern "C" fn(term_t, *mut *const c_char, c_uint) -> c_int,
    pub get_integer: unsafe extern "C" fn(term_t, *mut c_long) -> c_int,
    pub get_int64: unsafe extern "C" fn(term_t, *mut i64) -> c_int,
    pub get_float: unsafe extern "C" fn(term_t, *mut f64) -> c_int,
    pub get_functor: unsafe extern "C" fn(term_t, *mut functor_t) -> c_int,
    pub get_arg: unsafe extern "C" fn(c_int, term_t, term_t) -> c_int,
    pub get_head: unsafe extern "C" fn(term_t, term_t) -> c_int,
    pub get_tail: unsafe extern "C" fn(term_t, term_t) -> c_int,
    pub get_nil: unsafe extern "C" fn(term_t) -> c_int,
    pub get_list: unsafe extern "C" fn(term_t, term_t, term_t) -> c_int,
    pub unify: unsafe extern "C" fn(term_t, term_t) -> c_int,
    pub term_type: unsafe extern "C" fn(term_t) -> c_int,
    pub new_functor: unsafe extern "C" fn(atom_t, c_int) -> functor_t,
    pub new_functor_sz: unsafe extern "C" fn(atom_t, usize) -> functor_t,
    pub get_name_arity: unsafe extern "C" fn(term_t, *mut atom_t, *mut c_int) -> c_int,
    pub get_name_arity_sz: unsafe extern "C" fn(term_t, *mut atom_t, *mut usize) -> c_int,
    pub new_atom: unsafe extern "C" fn(*const c_char) -> atom_t,
    pub new_atom_mbchars: unsafe extern "C" fn(c_int, c_int, *const c_char) -> atom_t,
    pub atom_chars: unsafe extern "C" fn(atom_t) -> *const c_char,
    pub unregister_atom: unsafe extern "C" fn(atom_t) -> c_int,
    pub predicate: unsafe extern "C" fn(*const c_char, c_int, *const c_char) -> predicate_t,
    pub new_module: unsafe extern "C" fn(atom_t) -> module_t,
    pub open_query: unsafe extern "C" fn(module_t, c_int, predicate_t, term_t) -> qid_t,
    pub next_solution: unsafe extern "C" fn(qid_t) -> c_int,
    pub cut_query: unsafe extern "C" fn(qid_t) -> c_int,
    pub close_query: unsafe extern "C" fn(qid_t) -> c_int,
    pub call_predicate: unsafe extern "C" fn(module_t, c_int, predicate_t, term_t) -> c_int,
    pub record: unsafe extern "C" fn(term_t) -> record_t,
    pub recorded: unsafe extern "C" fn(record_t, term_t) -> c_int,
    pub erase: unsafe extern "C" fn(record_t) -> c_int,
    pub exception: unsafe extern "C" fn(qid_t) -> term_t,
    pub set_prolog_flag: unsafe extern "C" fn(*const c_char, term_t) -> c_int,
    pub cons_functor_v: unsafe extern "C" fn(term_t, functor_t, term_t) -> c_int,
    pub cons_list: unsafe extern "C" fn(term_t, term_t, term_t) -> c_int,
    pub register_foreign: unsafe extern "C" fn(*const c_char, c_int, unsafe extern "C" fn() -> c_int, c_int) -> c_int,
    pub chars_to_term: unsafe extern "C" fn(*const c_char, term_t) -> c_int,
    pub call: unsafe extern "C" fn(term_t, module_t) -> c_int,
}

impl Api {
    /// Get or initialize the global `Api` instance.
    pub fn get() -> Result<&'static Api, PrologError> {
        API.get_or_init(|| Self::load()).as_ref().map_err(Clone::clone)
    }

    fn load() -> Result<Self, PrologError> {
        let lib = Self::load_library()?;
        unsafe {
            // Set SWI_HOME_DIR from PLBASE if discovered
            if let Ok(base) = std::env::var("PLBASE") {
                std::env::set_var("SWI_HOME_DIR", &base);
            }

            // Store the library so it stays alive for the program duration
            let _ = LIBRARY.set(lib);

            let api = Self::from_library(LIBRARY.get().unwrap())?;
            // Initialize Prolog (argv[0] = "swipl", -q, --nosignals, pass --home if SWI_HOME_DIR is set)
            let mut argv_vec: Vec<*mut c_char> = vec![
                CString::new("swipl").unwrap().into_raw(),
                CString::new("-q").unwrap().into_raw(),
                CString::new("--nosignals").unwrap().into_raw(),
            ];
            if let Ok(swi_home) = std::env::var("SWI_HOME_DIR") {
                argv_vec.push(CString::new(format!("--home={}", swi_home)).unwrap().into_raw());
            }
            argv_vec.push(std::ptr::null_mut());

            let rc = (api.initialise)((argv_vec.len() - 1) as c_int, argv_vec.as_ptr() as *mut *mut c_char);
            if rc == 0 {
                return Err(PrologError::InitFailed(
                    "PL_initialise returned FALSE".into(),
                ));
            }
            // Release the main engine from the initialisation thread so that the dedicated
            // Prolog worker thread can attach to it.
            (api.set_engine)(PL_ENGINE_NONE, std::ptr::null_mut());
            Ok(api)
        }
    }

    fn load_library() -> Result<Library, PrologError> {
        // 1. Env override
        if let Ok(path) = std::env::var("MORK_METTA_LIBSWIPL") {
            if let Ok(lib) = unsafe { Library::new(&path) } {
                // Try to derive PLBASE from the library path
                // e.g., /home/user/.local/swipl/swipl/lib/x86_64-linux/libswipl.so.10
                // -> PLBASE = /home/user/.local/swipl/swipl
                let path_buf = Path::new(&path);
                if let Some(lib_dir) = path_buf.parent() {
                    if let Some(swipl_dir) = lib_dir.parent() {
                        if let Some(plbase) = swipl_dir.parent() {
                            unsafe { std::env::set_var("PLBASE", plbase) };
                        }
                    }
                }
                return Ok(lib);
            }
        }

        // 2. swipl --dump-runtime-variables
        let home_bin = std::env::var("HOME").map(|h| format!("{h}/.local/bin/swipl")).unwrap_or_default();
        let swipl_bins = ["swipl", &home_bin, "/usr/bin/swipl", "/usr/local/bin/swipl"];
        for swipl_bin in swipl_bins {
            if let Ok(output) = std::process::Command::new(swipl_bin)
                .arg("--dump-runtime-variables")
                .output()
            {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let mut plbase = None;
                    let mut pllibdir = None;
                    let mut pllibswipl = None;
                    for line in stdout.lines() {
                        let clean = line.trim();
                        if let Some(v) = clean.strip_prefix("PLBASE=") {
                            plbase = Some(v.trim_matches(|c| c == '"' || c == ';' || c == '\'').trim().to_string());
                        } else if let Some(v) = clean.strip_prefix("PLLIBDIR=") {
                            pllibdir = Some(v.trim_matches(|c| c == '"' || c == ';' || c == '\'').trim().to_string());
                        } else if let Some(v) = clean.strip_prefix("PLLIBSWIPL=") {
                            pllibswipl = Some(v.trim_matches(|c| c == '"' || c == ';' || c == '\'').trim().to_string());
                        }
                    }
                    if let Some(ref base) = plbase {
                        unsafe {
                            std::env::set_var("SWI_HOME_DIR", base);
                            std::env::set_var("PLBASE", base);
                        }
                    }
                    if let Some(ref libswipl) = pllibswipl {
                        if let Ok(lib) = unsafe { Library::new(libswipl) } {
                            return Ok(lib);
                        }
                    }
                    if let Some(libdir) = pllibdir {
                        let candidates = [
                            format!("{}/{}", libdir, "libswipl.so.10"),
                            format!("{}/{}", libdir, "libswipl.so.9"),
                            format!("{}/{}", libdir, "libswipl.so.8"),
                            format!("{}/{}", libdir, "libswipl.so"),
                            format!("{}/{}", libdir, "libswipl.dylib"),
                        ];
                        for cand in candidates {
                            if let Ok(lib) = unsafe { Library::new(&cand) } {
                                return Ok(lib);
                            }
                        }
                    }
                }
            }
        }

        // 3. Common names via system loader
        for name in [
            "libswipl.so.10",
            "libswipl.so.9",
            "libswipl.so.8",
            "libswipl.so",
            "libswipl.dylib",
        ] {
            if let Ok(lib) = unsafe { Library::new(name) } {
                return Ok(lib);
            }
        }

        Err(PrologError::LibraryNotFound(
            "could not locate libswipl (tried env MORK_METTA_LIBSWIPL, swipl --dump-runtime-variables, and common shared library names)".into(),
        ))
    }

    unsafe fn from_library(lib: &Library) -> Result<Self, PrologError> {
        macro_rules! sym {
            ($name:expr, $ty:ty) => {
                match unsafe { lib.get::<$ty>($name.as_bytes()) } {
                    Ok(s) => Ok(unsafe { *s.into_raw() }),
                    Err(e) => Err(PrologError::SymbolMissing(format!("{}: {:?}", $name, e))),
                }
            };
        }

        Ok(Self {
            initialise: sym!("PL_initialise", unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int)?,
            cleanup: sym!("PL_cleanup", unsafe extern "C" fn(c_int) -> c_int)?,
            create_engine: sym!("PL_create_engine", unsafe extern "C" fn(*mut c_void) -> PL_engine_t)?,
            destroy_engine: sym!("PL_destroy_engine", unsafe extern "C" fn(PL_engine_t) -> c_int)?,
            set_engine: sym!("PL_set_engine", unsafe extern "C" fn(PL_engine_t, *mut PL_engine_t) -> c_int)?,
            current_engine: sym!("PL_current_engine", unsafe extern "C" fn() -> PL_engine_t)?,
            open_foreign_frame: sym!("PL_open_foreign_frame", unsafe extern "C" fn() -> fid_t)?,
            close_foreign_frame: sym!("PL_close_foreign_frame", unsafe extern "C" fn(fid_t))?,
            discard_foreign_frame: sym!("PL_discard_foreign_frame", unsafe extern "C" fn(fid_t))?,
            clear_exception: sym!("PL_clear_exception", unsafe extern "C" fn())?,
            new_term_ref: sym!("PL_new_term_ref", unsafe extern "C" fn() -> term_t)?,
            new_term_refs: sym!("PL_new_term_refs", unsafe extern "C" fn(c_int) -> term_t)?,
            reset_term_refs: sym!("PL_reset_term_refs", unsafe extern "C" fn(term_t) -> c_int)?,
            put_atom: sym!("PL_put_atom", unsafe extern "C" fn(term_t, atom_t) -> c_int)?,
            put_atom_chars: sym!("PL_put_atom_chars", unsafe extern "C" fn(term_t, *const c_char) -> c_int)?,
            put_string_chars: sym!("PL_put_string_chars", unsafe extern "C" fn(term_t, *const c_char) -> c_int)?,
            put_integer: sym!("PL_put_integer", unsafe extern "C" fn(term_t, c_long) -> c_int)?,
            put_int64: sym!("PL_put_int64", unsafe extern "C" fn(term_t, i64) -> c_int)?,
            put_float: sym!("PL_put_float", unsafe extern "C" fn(term_t, f64) -> c_int)?,
            put_functor: sym!("PL_put_functor", unsafe extern "C" fn(term_t, functor_t) -> c_int)?,
            put_list: sym!("PL_put_list", unsafe extern "C" fn(term_t) -> c_int)?,
            put_nil: sym!("PL_put_nil", unsafe extern "C" fn(term_t) -> c_int)?,
            put_variable: sym!("PL_put_variable", unsafe extern "C" fn(term_t) -> c_int)?,
            put_term: sym!("PL_put_term", unsafe extern "C" fn(term_t, term_t) -> c_int)?,
            get_atom: sym!("PL_get_atom", unsafe extern "C" fn(term_t, *mut atom_t) -> c_int)?,
            get_atom_chars: sym!("PL_get_atom_chars", unsafe extern "C" fn(atom_t, *mut *const c_char) -> c_int)?,
            get_chars: sym!("PL_get_chars", unsafe extern "C" fn(term_t, *mut *const c_char, c_uint) -> c_int)?,
            get_integer: sym!("PL_get_integer", unsafe extern "C" fn(term_t, *mut c_long) -> c_int)?,
            get_int64: sym!("PL_get_int64", unsafe extern "C" fn(term_t, *mut i64) -> c_int)?,
            get_float: sym!("PL_get_float", unsafe extern "C" fn(term_t, *mut f64) -> c_int)?,
            get_functor: sym!("PL_get_functor", unsafe extern "C" fn(term_t, *mut functor_t) -> c_int)?,
            get_arg: sym!("PL_get_arg", unsafe extern "C" fn(c_int, term_t, term_t) -> c_int)?,
            get_head: sym!("PL_get_head", unsafe extern "C" fn(term_t, term_t) -> c_int)?,
            get_tail: sym!("PL_get_tail", unsafe extern "C" fn(term_t, term_t) -> c_int)?,
            get_nil: sym!("PL_get_nil", unsafe extern "C" fn(term_t) -> c_int)?,
            get_list: sym!("PL_get_list", unsafe extern "C" fn(term_t, term_t, term_t) -> c_int)?,
            unify: sym!("PL_unify", unsafe extern "C" fn(term_t, term_t) -> c_int)?,
            term_type: sym!("PL_term_type", unsafe extern "C" fn(term_t) -> c_int)?,
            new_functor: sym!("PL_new_functor", unsafe extern "C" fn(atom_t, c_int) -> functor_t)?,
            new_functor_sz: sym!("PL_new_functor_sz", unsafe extern "C" fn(atom_t, usize) -> functor_t)?,
            get_name_arity: sym!("PL_get_name_arity", unsafe extern "C" fn(term_t, *mut atom_t, *mut c_int) -> c_int)?,
            get_name_arity_sz: sym!("PL_get_name_arity_sz", unsafe extern "C" fn(term_t, *mut atom_t, *mut usize) -> c_int)?,
            new_atom: sym!("PL_new_atom", unsafe extern "C" fn(*const c_char) -> atom_t)?,
            new_atom_mbchars: sym!("PL_new_atom_mbchars", unsafe extern "C" fn(c_int, c_int, *const c_char) -> atom_t)?,
            atom_chars: sym!("PL_atom_chars", unsafe extern "C" fn(atom_t) -> *const c_char)?,
            unregister_atom: sym!("PL_unregister_atom", unsafe extern "C" fn(atom_t) -> c_int)?,
            predicate: sym!("PL_predicate", unsafe extern "C" fn(*const c_char, c_int, *const c_char) -> predicate_t)?,
            new_module: sym!("PL_new_module", unsafe extern "C" fn(atom_t) -> module_t)?,
            open_query: sym!("PL_open_query", unsafe extern "C" fn(module_t, c_int, predicate_t, term_t) -> qid_t)?,
            next_solution: sym!("PL_next_solution", unsafe extern "C" fn(qid_t) -> c_int)?,
            cut_query: sym!("PL_cut_query", unsafe extern "C" fn(qid_t) -> c_int)?,
            close_query: sym!("PL_close_query", unsafe extern "C" fn(qid_t) -> c_int)?,
            call_predicate: sym!("PL_call_predicate", unsafe extern "C" fn(module_t, c_int, predicate_t, term_t) -> c_int)?,
            record: sym!("PL_record", unsafe extern "C" fn(term_t) -> record_t)?,
            recorded: sym!("PL_recorded", unsafe extern "C" fn(record_t, term_t) -> c_int)?,
            erase: sym!("PL_erase", unsafe extern "C" fn(record_t) -> c_int)?,
            exception: sym!("PL_exception", unsafe extern "C" fn(qid_t) -> term_t)?,
            set_prolog_flag: sym!("PL_set_prolog_flag", unsafe extern "C" fn(*const c_char, term_t) -> c_int)?,
            cons_functor_v: sym!("PL_cons_functor_v", unsafe extern "C" fn(term_t, functor_t, term_t) -> c_int)?,
            cons_list: sym!("PL_cons_list", unsafe extern "C" fn(term_t, term_t, term_t) -> c_int)?,
            register_foreign: sym!("PL_register_foreign", unsafe extern "C" fn(*const c_char, c_int, unsafe extern "C" fn() -> c_int, c_int) -> c_int)?,
            chars_to_term: sym!("PL_chars_to_term", unsafe extern "C" fn(*const c_char, term_t) -> c_int)?,
            call: sym!("PL_call", unsafe extern "C" fn(term_t, module_t) -> c_int)?,
        })
    }
}

// RAII guard to call PL_cleanup on drop.
pub struct PrologInitGuard;
impl Drop for PrologInitGuard {
    fn drop(&mut self) {
        // Do not call PL_cleanup here; it would shut down Prolog for other tests.
        // The OS will clean up resources on process exit.
    }
}

/// Initialize the prolog runtime and return a guard that cleans up on drop.
pub fn init_prolog() -> Result<PrologInitGuard, PrologError> {
    Api::get()?;
    Ok(PrologInitGuard)
}