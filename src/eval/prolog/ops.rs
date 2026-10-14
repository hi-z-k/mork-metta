//! Internal operations executed on the dedicated Prolog worker thread.

use std::collections::{BTreeSet, HashMap};
use std::ffi::CString;
use std::sync::Arc;

use crate::atom::Atom;
use crate::eval::prolog::convert::{atom_to_term, goal_to_term, term_to_atom, VarMap};
use crate::eval::prolog::engine::{EngineManager, PooledEngine};
use crate::eval::prolog::error::PrologError;
use crate::eval::prolog::ffi::{
    Api, fid_t, module_t, predicate_t, qid_t, term_t, PL_Q_CATCH_EXCEPTION, PL_Q_EXT_STATUS,
    PL_Q_NODEBUG, PL_Q_NORMAL, PL_S_LAST, PL_S_TRUE, PL_S_FALSE, PL_S_EXCEPTION,
};
use crate::eval::prolog::query::{Query, StepStatus};

/// Outcome of invoking an imported Prolog function.
pub enum FunctionOutcome {
    /// Deterministic solution (last answer). Engine already returned to pool.
    Single(Atom),
    /// Open choicepoint. Query remains open under stream id.
    Stream {
        stream_id: usize,
        first_answer: Option<Atom>,
    },
    /// Goal failed.
    Empty,
    /// Uncaught Prolog exception.
    Exception(String),
}

/// Represents an active non-deterministic query stream.
pub struct ActiveStream {
    pub pooled_engine: PooledEngine,
    pub query: Query,
    pub frame: fid_t,
    pub result_term: term_t,
}

pub enum StepStreamResult {
    Answer(Atom, bool /* is_last */),
    Done,
    Error(PrologError),
}

/// Load Prolog support clauses (arithmetic predicates and dynamic space declarations).
pub fn init_support(api: &Api, manager: &Arc<EngineManager>) -> Result<(), PrologError> {
    let code = r#"
:- dynamic '&self'/1.
:- dynamic '&self'/2.
:- dynamic '&self'/3.
:- dynamic '&self'/4.
:- dynamic '&self'/5.
:- dynamic '&self'/6.
:- dynamic '&self'/7.
:- dynamic '&self'/8.

'+'(X, Y, Z) :- Z is X + Y.
'-'(X, Y, Z) :- Z is X - Y.
'*'(X, Y, Z) :- Z is X * Y.
'/'(X, Y, Z) :- Z is X / Y.
'mod'(X, Y, Z) :- Z is X mod Y.
'rem'(X, Y, Z) :- Z is X rem Y.
'abs'(X, Y) :- Y is abs(X).
'<='(X, Y) :- X =< Y.

:- multifile user:file_search_path/2.
:- dynamic user:file_search_path/2.
user:file_search_path(library, './lib').
user:file_search_path(library, 'lib').
"#;
    load_prolog_code(api, manager, code)
}

/// Helper to load a string of Prolog code into the `user` module.
pub fn load_prolog_code(api: &Api, manager: &Arc<EngineManager>, code: &str) -> Result<(), PrologError> {
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(manager.main_engine(), &mut old_engine);

        let frame = (api.open_foreign_frame)();
        let p_open_str = (api.predicate)(
            b"open_string\0".as_ptr() as *const _,
            2,
            b"system\0".as_ptr() as *const _,
        );
        let sargs = (api.new_term_refs)(2);
        let c_code = CString::new(code).map_err(|e| PrologError::Conversion(e.to_string()))?;
        (api.put_string_chars)(sargs, c_code.as_ptr());

        let q = (api.open_query)(0, PL_Q_NORMAL, p_open_str, sargs);
        if q == 0 || (api.next_solution)(q) == 0 {
            if q != 0 {
                (api.close_query)(q);
            }
            (api.discard_foreign_frame)(frame);
            (api.set_engine)(old_engine, std::ptr::null_mut());
            return Err(PrologError::Internal("failed to open_string".into()));
        }
        (api.cut_query)(q);

        let stream_term = sargs + 1;
        let lf_args = (api.new_term_refs)(2);
        (api.put_atom_chars)(lf_args, b"user\0".as_ptr() as *const _);

        let opt = (api.new_term_ref)();
        let stream_atom = (api.new_atom)(b"stream\0".as_ptr() as *const _);
        let stream_functor = (api.new_functor)(stream_atom, 1);
        (api.cons_functor_v)(opt, stream_functor, stream_term);

        let opts = (api.new_term_ref)();
        (api.put_nil)(opts);
        (api.cons_list)(opts, opt, opts);
        (api.put_term)(lf_args + 1, opts);

        let p_load = (api.predicate)(
            b"load_files\0".as_ptr() as *const _,
            2,
            b"system\0".as_ptr() as *const _,
        );
        let q_load = (api.open_query)(0, PL_Q_NORMAL, p_load, lf_args);
        if q_load != 0 {
            (api.next_solution)(q_load);
            (api.close_query)(q_load);
        }
        (api.discard_foreign_frame)(frame);
        (api.set_engine)(old_engine, std::ptr::null_mut());
        Ok(())
    }
}

/// Execute `call(Goal)` and return True for each solution.
pub fn call_predicate(
    api: &Api,
    manager: &Arc<EngineManager>,
    goal: &Atom,
) -> Result<Vec<Atom>, PrologError> {
    let pooled = manager.checkout()?;
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(pooled.engine(), &mut old_engine);

        let frame = (api.open_foreign_frame)();
        let mut var_map = VarMap::new();
        let goal_term = match goal_to_term(goal, api, &mut var_map) {
            Ok(t) => t,
            Err(e) => {
                (api.discard_foreign_frame)(frame);
                (api.set_engine)(old_engine, std::ptr::null_mut());
                return Err(e);
            }
        };

        let p_call = (api.predicate)(
            b"call\0".as_ptr() as *const _,
            1,
            b"system\0".as_ptr() as *const _,
        );
        let query = Query::open(api, 0, p_call, goal_term)?;

        let mut solutions = Vec::new();
        loop {
            match query.step(api) {
                StepStatus::True => {
                    solutions.push(Atom::sym("true"));
                }
                StepStatus::Last => {
                    solutions.push(Atom::sym("true"));
                    break;
                }
                StepStatus::False => {
                    break;
                }
                StepStatus::Exception(msg) => {
                    query.close(api);
                    (api.discard_foreign_frame)(frame);
                    (api.set_engine)(old_engine, std::ptr::null_mut());
                    return Err(PrologError::Exception(msg));
                }
            }
        }

        query.close(api);
        (api.discard_foreign_frame)(frame);
        (api.set_engine)(old_engine, std::ptr::null_mut());
        Ok(solutions)
    }
}

/// Call an imported Prolog function `functor(args..., Result)`.
pub fn call_function(
    api: &Api,
    manager: &Arc<EngineManager>,
    functor_name: &str,
    args: &[Atom],
    active_streams: &mut HashMap<usize, ActiveStream>,
    next_stream_id: &mut usize,
) -> Result<FunctionOutcome, PrologError> {
    let pooled = manager.checkout()?;
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(pooled.engine(), &mut old_engine);

        let frame = (api.open_foreign_frame)();
        let mut var_map = VarMap::new();

        let c_head = CString::new(functor_name).map_err(|e| PrologError::Conversion(e.to_string()))?;
        let head_atom = (api.new_atom)(c_head.as_ptr());
        let arity = args.len() + 1;
        let functor = (api.new_functor_sz)(head_atom, arity);

        let a0 = (api.new_term_refs)(arity as std::ffi::c_int);
        for (i, arg) in args.iter().enumerate() {
            let t = atom_to_term(arg, api, &mut var_map)?;
            (api.put_term)(a0 + i, t);
        }
        let result_term = a0 + args.len();
        (api.put_variable)(result_term);

        let goal_term = (api.new_term_ref)();
        if (api.cons_functor_v)(goal_term, functor, a0) == 0 {
            (api.discard_foreign_frame)(frame);
            (api.set_engine)(old_engine, std::ptr::null_mut());
            return Err(PrologError::Conversion("failed to build function goal".into()));
        }

        let p_call = (api.predicate)(
            b"call\0".as_ptr() as *const _,
            1,
            b"system\0".as_ptr() as *const _,
        );
        let query = Query::open(api, 0, p_call, goal_term)?;

        match query.step(api) {
            StepStatus::Last => {
                let mut rev_map = HashMap::new();
                let ans = term_to_atom(result_term, api, &mut rev_map)?;
                query.close(api);
                (api.discard_foreign_frame)(frame);
                (api.set_engine)(old_engine, std::ptr::null_mut());
                Ok(FunctionOutcome::Single(ans))
            }
            StepStatus::True => {
                let mut rev_map = HashMap::new();
                let ans = term_to_atom(result_term, api, &mut rev_map)?;
                let stream_id = *next_stream_id;
                *next_stream_id += 1;
                active_streams.insert(
                    stream_id,
                    ActiveStream {
                        pooled_engine: pooled,
                        query,
                        frame,
                        result_term,
                    },
                );
                (api.set_engine)(old_engine, std::ptr::null_mut());
                Ok(FunctionOutcome::Stream {
                    stream_id,
                    first_answer: Some(ans),
                })
            }
            StepStatus::False => {
                query.close(api);
                (api.discard_foreign_frame)(frame);
                (api.set_engine)(old_engine, std::ptr::null_mut());
                Ok(FunctionOutcome::Empty)
            }
            StepStatus::Exception(msg) => {
                query.close(api);
                (api.discard_foreign_frame)(frame);
                (api.set_engine)(old_engine, std::ptr::null_mut());
                Ok(FunctionOutcome::Exception(msg))
            }
        }
    }
}

/// Step an active stream to retrieve the next solution.
pub fn step_stream(api: &Api, stream: &mut ActiveStream) -> StepStreamResult {
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(stream.pooled_engine.engine(), &mut old_engine);

        let res = match stream.query.step(api) {
            StepStatus::True => {
                let mut rev_map = HashMap::new();
                match term_to_atom(stream.result_term, api, &mut rev_map) {
                    Ok(atom) => StepStreamResult::Answer(atom, false),
                    Err(e) => StepStreamResult::Error(e),
                }
            }
            StepStatus::Last => {
                let mut rev_map = HashMap::new();
                match term_to_atom(stream.result_term, api, &mut rev_map) {
                    Ok(atom) => StepStreamResult::Answer(atom, true),
                    Err(e) => StepStreamResult::Error(e),
                }
            }
            StepStatus::False => StepStreamResult::Done,
            StepStatus::Exception(msg) => StepStreamResult::Error(PrologError::Exception(msg)),
        };

        (api.set_engine)(old_engine, std::ptr::null_mut());
        res
    }
}

/// Close and release an active stream.
pub fn close_stream(api: &Api, stream: ActiveStream) {
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(stream.pooled_engine.engine(), &mut old_engine);
        stream.query.close(api);
        (api.discard_foreign_frame)(stream.frame);
        (api.set_engine)(old_engine, std::ptr::null_mut());
    }
}

/// Execute asserta on the main engine.
pub fn asserta(
    api: &Api,
    manager: &Arc<EngineManager>,
    clause: &Atom,
) -> Result<(), PrologError> {
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(manager.main_engine(), &mut old_engine);

        let frame = (api.open_foreign_frame)();
        let mut var_map = VarMap::new();
        let clause_term = goal_to_term(clause, api, &mut var_map)?;

        let p_assert = (api.predicate)(
            b"asserta\0".as_ptr() as *const _,
            1,
            b"system\0".as_ptr() as *const _,
        );
        let q = (api.open_query)(0, PL_Q_NORMAL, p_assert, clause_term);
        if q != 0 {
            (api.next_solution)(q);
            (api.close_query)(q);
        }
        (api.discard_foreign_frame)(frame);
        (api.set_engine)(old_engine, std::ptr::null_mut());
        Ok(())
    }
}

/// Execute assertz on the main engine.
pub fn assertz(
    api: &Api,
    manager: &Arc<EngineManager>,
    clause: &Atom,
) -> Result<(), PrologError> {
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(manager.main_engine(), &mut old_engine);

        let frame = (api.open_foreign_frame)();
        let mut var_map = VarMap::new();
        let clause_term = goal_to_term(clause, api, &mut var_map)?;

        let p_assert = (api.predicate)(
            b"assertz\0".as_ptr() as *const _,
            1,
            b"system\0".as_ptr() as *const _,
        );
        let q = (api.open_query)(0, PL_Q_NORMAL, p_assert, clause_term);
        if q != 0 {
            (api.next_solution)(q);
            (api.close_query)(q);
        }
        (api.discard_foreign_frame)(frame);
        (api.set_engine)(old_engine, std::ptr::null_mut());
        Ok(())
    }
}

/// Execute retract on the main engine. Returns true if retracted.
pub fn retract(
    api: &Api,
    manager: &Arc<EngineManager>,
    clause: &Atom,
) -> Result<bool, PrologError> {
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(manager.main_engine(), &mut old_engine);

        let frame = (api.open_foreign_frame)();
        let mut var_map = VarMap::new();
        let clause_term = goal_to_term(clause, api, &mut var_map)?;

        let p_retract = (api.predicate)(
            b"retract\0".as_ptr() as *const _,
            1,
            b"system\0".as_ptr() as *const _,
        );
        let q = (api.open_query)(0, PL_Q_NORMAL, p_retract, clause_term);
        let removed = if q != 0 {
            let rc = (api.next_solution)(q);
            (api.close_query)(q);
            rc != 0
        } else {
            false
        };
        (api.discard_foreign_frame)(frame);
        (api.set_engine)(old_engine, std::ptr::null_mut());
        Ok(removed)
    }
}

/// Consult a Prolog file on the main engine.
pub fn consult(
    api: &Api,
    manager: &Arc<EngineManager>,
    path: &str,
) -> Result<(), PrologError> {
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(manager.main_engine(), &mut old_engine);

        let frame = (api.open_foreign_frame)();
        let c_path = CString::new(path).map_err(|e| PrologError::Conversion(e.to_string()))?;
        let path_term = (api.new_term_ref)();
        (api.put_atom_chars)(path_term, c_path.as_ptr());

        let p_consult = (api.predicate)(
            b"consult\0".as_ptr() as *const _,
            1,
            b"system\0".as_ptr() as *const _,
        );
        let q = (api.open_query)(0, PL_Q_CATCH_EXCEPTION | PL_Q_NORMAL, p_consult, path_term);
        if q == 0 {
            (api.discard_foreign_frame)(frame);
            (api.set_engine)(old_engine, std::ptr::null_mut());
            return Err(PrologError::Internal(format!("failed to open consult query for {path}")));
        }
        let rc = (api.next_solution)(q);
        if rc == PL_S_EXCEPTION {
            let ex = (api.exception)(q);
            let mut ptr: *const std::ffi::c_char = std::ptr::null();
            let msg = if (api.get_chars)(ex, &mut ptr, crate::eval::prolog::ffi::CVT_WRITE) != 0 && !ptr.is_null() {
                std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
            } else {
                "consult exception".to_string()
            };
            (api.clear_exception)();
            (api.close_query)(q);
            (api.discard_foreign_frame)(frame);
            (api.set_engine)(old_engine, std::ptr::null_mut());
            return Err(PrologError::Exception(msg));
        }
        (api.close_query)(q);
        (api.discard_foreign_frame)(frame);
        (api.set_engine)(old_engine, std::ptr::null_mut());
        if rc == 0 {
            return Err(PrologError::Internal(format!("consult failed for {path}")));
        }
        Ok(())
    }
}

/// Find all defined arities for a predicate name in user/system.
pub fn current_arities(
    api: &Api,
    manager: &Arc<EngineManager>,
    name: &str,
) -> Result<Vec<usize>, PrologError> {
    let pooled = manager.checkout()?;
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(pooled.engine(), &mut old_engine);

        let frame = (api.open_foreign_frame)();
        let c_name = CString::new(name).map_err(|e| PrologError::Conversion(e.to_string()))?;
        let name_atom = (api.new_atom)(c_name.as_ptr());

        // Construct current_predicate(Name/Arity)
        let slash_atom = (api.new_atom)(b"/\0".as_ptr() as *const _);
        let slash_functor = (api.new_functor)(slash_atom, 2);

        let slash_args = (api.new_term_refs)(2);
        (api.put_atom)(slash_args, name_atom);
        let arity_term = slash_args + 1;
        (api.put_variable)(arity_term);

        let spec_term = (api.new_term_ref)();
        (api.cons_functor_v)(spec_term, slash_functor, slash_args);

        let p_curr = (api.predicate)(
            b"current_predicate\0".as_ptr() as *const _,
            1,
            b"system\0".as_ptr() as *const _,
        );
        let user_atom = (api.new_atom)(b"user\0".as_ptr() as *const _);
        let user_mod = (api.new_module)(user_atom);
        let q = (api.open_query)(user_mod, PL_Q_NORMAL, p_curr, spec_term);

        let mut arities = BTreeSet::new();
        if q != 0 {
            while (api.next_solution)(q) != 0 {
                let mut a: i64 = 0;
                if (api.get_int64)(arity_term, &mut a) != 0 && a >= 0 {
                    arities.insert(a as usize);
                }
            }
            (api.close_query)(q);
        }
        (api.discard_foreign_frame)(frame);
        (api.set_engine)(old_engine, std::ptr::null_mut());
        Ok(arities.into_iter().collect())
    }
}

/// Execute translatePredicate with input bindings and extract output bindings.
pub fn translate(
    api: &Api,
    manager: &Arc<EngineManager>,
    goal: &Atom,
    in_bindings: &[(String, Atom)],
) -> Result<Vec<Vec<(String, Atom)>>, PrologError> {
    let pooled = manager.checkout()?;
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(pooled.engine(), &mut old_engine);

        let frame = (api.open_foreign_frame)();
        let mut var_map = VarMap::new();

        // 1. Populate input bindings
        for (vname, vval) in in_bindings {
            let val_term = atom_to_term(vval, api, &mut var_map)?;
            var_map.insert(vname.clone(), val_term);
        }

        // 2. Convert goal (unbound variables are created and added to var_map)
        let goal_term = match goal_to_term(goal, api, &mut var_map) {
            Ok(t) => t,
            Err(e) => {
                (api.discard_foreign_frame)(frame);
                (api.set_engine)(old_engine, std::ptr::null_mut());
                return Err(e);
            }
        };

        // 3. Open query call(Goal)
        let p_call = (api.predicate)(
            b"call\0".as_ptr() as *const _,
            1,
            b"system\0".as_ptr() as *const _,
        );
        let query = Query::open(api, 0, p_call, goal_term)?;

        let mut all_solutions = Vec::new();
        loop {
            match query.step(api) {
                StepStatus::True => {
                    let mut sol_bindings = Vec::new();
                    let mut rev_map = HashMap::new();
                    for (vname, &vterm) in &var_map {
                        if let Ok(atom) = term_to_atom(vterm, api, &mut rev_map) {
                            sol_bindings.push((vname.clone(), atom));
                        }
                    }
                    all_solutions.push(sol_bindings);
                }
                StepStatus::Last => {
                    let mut sol_bindings = Vec::new();
                    let mut rev_map = HashMap::new();
                    for (vname, &vterm) in &var_map {
                        if let Ok(atom) = term_to_atom(vterm, api, &mut rev_map) {
                            sol_bindings.push((vname.clone(), atom));
                        }
                    }
                    all_solutions.push(sol_bindings);
                    break;
                }
                StepStatus::False => {
                    break;
                }
                StepStatus::Exception(msg) => {
                    query.close(api);
                    (api.discard_foreign_frame)(frame);
                    (api.set_engine)(old_engine, std::ptr::null_mut());
                    return Err(PrologError::Exception(msg));
                }
            }
        }

        query.close(api);
        (api.discard_foreign_frame)(frame);
        (api.set_engine)(old_engine, std::ptr::null_mut());
        Ok(all_solutions)
    }
}

/// Sync space atoms into Prolog dynamic clauses for `'&self'`.
pub fn sync_space(
    api: &Api,
    manager: &Arc<EngineManager>,
    atoms: &[Atom],
) -> Result<(), PrologError> {
    unsafe {
        let mut old_engine = 0;
        (api.set_engine)(manager.main_engine(), &mut old_engine);

        let frame = (api.open_foreign_frame)();

        // 1. Retract all arities of '&self' dynamically
        let c_code = CString::new(
            "forall(current_predicate('&self'/A), (functor(H, '&self', A), retractall(H)))."
        ).unwrap();
        let term = (api.new_term_ref)();
        (api.chars_to_term)(c_code.as_ptr(), term);
        let user_atom = (api.new_atom)(b"user\0".as_ptr() as *const _);
        let user_mod = (api.new_module)(user_atom);
        (api.call)(term, user_mod);

        // 2. Assert each atom as '&self'(args...)
        for atom in atoms {
            if let Atom::Expr(items) = atom {
                if !items.is_empty() {
                    let mut var_map = VarMap::new();
                    let a0 = (api.new_term_refs)(items.len() as std::ffi::c_int);
                    let mut ok = true;
                    for (i, it) in items.iter().enumerate() {
                        if let Ok(t) = atom_to_term(it, api, &mut var_map) {
                            (api.put_term)(a0 + i, t);
                        } else {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        let self_atom = (api.new_atom)(b"&self\0".as_ptr() as *const _);
                        let self_functor = (api.new_functor_sz)(self_atom, items.len());
                        let clause_term = (api.new_term_ref)();
                        if (api.cons_functor_v)(clause_term, self_functor, a0) != 0 {
                            let p_assertz = (api.predicate)(
                                b"assertz\0".as_ptr() as *const _,
                                1,
                                b"system\0".as_ptr() as *const _,
                            );
                            let q = (api.open_query)(0, PL_Q_NORMAL, p_assertz, clause_term);
                            if q != 0 {
                                (api.next_solution)(q);
                                (api.close_query)(q);
                            }
                        }
                    }
                }
            }
        }

        (api.discard_foreign_frame)(frame);
        (api.set_engine)(old_engine, std::ptr::null_mut());
        Ok(())
    }
}
