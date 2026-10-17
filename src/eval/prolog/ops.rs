//! Internal Prolog operations.
//!
//! One-shot queries and imports use a pooled engine; mutations
//! (assert/retract/consult/space sync) run on the mutex-guarded main engine.

use std::collections::{BTreeSet, HashMap};
use std::ffi::{CStr, CString, c_char, c_int};

use crate::atom::Atom;
use crate::eval::prolog::convert::{VarMap, atom_to_term, goal_to_term, term_to_atom};
use crate::eval::prolog::engine::{self, with_engine};
use crate::eval::prolog::error::PrologError;
use crate::eval::prolog::ffi::{
    Api, CVT_WRITE, PL_Q_CATCH_EXCEPTION, PL_Q_EXT_STATUS, PL_Q_NODEBUG, PL_Q_NORMAL,
    PL_S_EXCEPTION, PL_S_LAST, PL_S_TRUE, qid_t, term_t,
};

pub enum FunctionOutcome {
    Single(Atom),
    Stream(std::vec::IntoIter<Atom>),
    Empty,
    Exception(String),
}

enum StepStatus {
    True,
    Last,
    False,
    Exception(String),
}

fn next_solution(api: &Api, qid: qid_t) -> StepStatus {
    match unsafe { api.next_solution(qid) } {
        PL_S_LAST => StepStatus::Last,
        PL_S_TRUE => StepStatus::True,
        PL_S_EXCEPTION => StepStatus::Exception(read_exception(api, qid)),
        _ => StepStatus::False,
    }
}

fn read_exception(api: &Api, qid: qid_t) -> String {
    unsafe {
        let ex = api.exception(qid);
        let mut ptr: *const c_char = std::ptr::null();
        let msg = if ex != 0 && api.get_chars(ex, &mut ptr, CVT_WRITE) != 0 && !ptr.is_null() {
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        } else {
            "unknown Prolog exception".to_string()
        };
        api.clear_exception();
        msg
    }
}

fn open_call(api: &Api, goal: term_t) -> Result<qid_t, PrologError> {
    unsafe {
        let p = api.predicate(
            b"call\0".as_ptr() as *const _,
            1,
            b"system\0".as_ptr() as *const _,
        );
        let flags = PL_Q_CATCH_EXCEPTION | PL_Q_EXT_STATUS | PL_Q_NODEBUG;
        let qid = api.open_query(0, flags, p, goal);
        if qid == 0 {
            return Err(PrologError::Internal("PL_open_query returned NULL".into()));
        }
        Ok(qid)
    }
}

/// Run `pred(args)` once on the currently attached engine; returns the raw
/// `PL_S_*` status, mapping thrown exceptions to `PrologError::Exception`.
fn run_once(
    api: &Api,
    name: &[u8],
    arity: c_int,
    args: term_t,
    flags: c_int,
) -> Result<c_int, PrologError> {
    unsafe {
        let p = api.predicate(
            name.as_ptr() as *const _,
            arity,
            b"system\0".as_ptr() as *const _,
        );
        let q = api.open_query(0, flags, p, args);
        if q == 0 {
            return Err(PrologError::Internal("PL_open_query returned NULL".into()));
        }
        let rc = api.next_solution(q);
        if rc == PL_S_EXCEPTION {
            let msg = read_exception(api, q);
            api.close_query(q);
            return Err(PrologError::Exception(msg));
        }
        api.close_query(q);
        Ok(rc)
    }
}

/// Load Prolog support clauses and space declarations on the main engine.
pub fn init_support(manager: &std::sync::Arc<engine::EngineManager>) -> Result<(), PrologError> {
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
    load_prolog_code(manager, code)
}

/// Load a string of Prolog code into the `user` module on the main engine.
pub fn load_prolog_code(
    manager: &std::sync::Arc<engine::EngineManager>,
    code: &str,
) -> Result<(), PrologError> {
    manager.with_main(|api| -> Result<(), PrologError> {
        unsafe {
            let frame = api.open_foreign_frame();
            let sargs = api.new_term_refs(2);
            let c_code = CString::new(code)?;
            api.put_string_chars(sargs, c_code.as_ptr());
            let p_open = api.predicate(
                b"open_string\0".as_ptr() as *const _,
                2,
                b"system\0".as_ptr() as *const _,
            );
            let q = api.open_query(0, PL_Q_NORMAL, p_open, sargs);
            if q == 0 || api.next_solution(q) == 0 {
                if q != 0 {
                    api.close_query(q);
                }
                api.discard_foreign_frame(frame);
                return Err(PrologError::Internal("failed to open_string".into()));
            }
            api.cut_query(q);

            let lf_args = api.new_term_refs(2);
            api.put_atom_chars(lf_args, b"user\0".as_ptr() as *const _);
            let opt = api.new_term_ref();
            let stream_atom = api.new_atom(b"stream\0".as_ptr() as *const _);
            api.cons_functor_v(opt, api.new_functor(stream_atom, 1), sargs + 1);
            let opts = api.new_term_ref();
            api.put_nil(opts);
            api.cons_list(opts, opt, opts);
            api.put_term(lf_args + 1, opts);

            let p_load = api.predicate(
                b"load_files\0".as_ptr() as *const _,
                2,
                b"system\0".as_ptr() as *const _,
            );
            let q_load = api.open_query(0, PL_Q_NORMAL, p_load, lf_args);
            if q_load != 0 {
                api.next_solution(q_load);
                api.close_query(q_load);
            }
            api.discard_foreign_frame(frame);
            Ok(())
        }
    })?
}

/// Check out a pooled engine, open `call(Goal)`, and collect one value per
/// solution produced by `on_solution`.
fn run_goal<T>(
    goal: &Atom,
    in_bindings: &[(String, Atom)],
    mut on_solution: impl FnMut(&Api, &VarMap) -> T,
) -> Result<Vec<T>, PrologError> {
    let manager = engine::manager()?;
    let pooled = manager.checkout()?;
    with_engine(
        pooled.engine(),
        |api| -> Result<Vec<T>, PrologError> {
            unsafe {
                let frame = api.open_foreign_frame();
                let result = (|| {
                    let mut var_map = VarMap::new();
                    for (name, value) in in_bindings {
                        let val_term = atom_to_term(value, api, &mut var_map)?;
                        var_map.insert(name.clone(), val_term);
                    }
                    let goal_term = goal_to_term(goal, api, &mut var_map)?;
                    let qid = open_call(api, goal_term)?;
                    let mut solutions = Vec::new();
                    loop {
                        match next_solution(api, qid) {
                            StepStatus::True => solutions.push(on_solution(api, &var_map)),
                            StepStatus::Last => {
                                solutions.push(on_solution(api, &var_map));
                                break;
                            }
                            StepStatus::False => break,
                            StepStatus::Exception(msg) => {
                                api.close_query(qid);
                                return Err(PrologError::Exception(msg));
                            }
                        }
                    }
                    api.close_query(qid);
                    Ok(solutions)
                })();
                api.discard_foreign_frame(frame);
                result
            }
        },
    )?
}

/// Execute `call(Goal)` and return `true` per solution.
pub fn call_predicate_goal(goal: &Atom) -> Result<Vec<Atom>, PrologError> {
    run_goal(goal, &[], |_, _| Atom::sym("true"))
}

/// Call an imported Prolog function `functor(args..., Result)`.
pub fn call_function(functor_name: &str, args: &[Atom]) -> Result<FunctionOutcome, PrologError> {
    let manager = engine::manager()?;
    let pooled = manager.checkout()?;
    let api = Api::get()?;
    let mut old: usize = 0;
    unsafe { api.set_engine(pooled.engine(), &mut old) };

    let outcome = unsafe {
        let frame = api.open_foreign_frame();
        let mut var_map = VarMap::new();
        let c_head = CString::new(functor_name)?;
        let arity = args.len() + 1;
        let a0 = api.new_term_refs(arity as c_int);
        for (i, arg) in args.iter().enumerate() {
            let t = atom_to_term(arg, api, &mut var_map)?;
            api.put_term(a0 + i, t);
        }
        let result_term = a0 + args.len();
        api.put_variable(result_term);
        let head_atom = api.new_atom(c_head.as_ptr());
        let goal_term = api.new_term_ref();
        if api.cons_functor_v(goal_term, api.new_functor_sz(head_atom, arity), a0) == 0 {
            api.discard_foreign_frame(frame);
            return Err(PrologError::Conversion(
                "failed to build function goal".into(),
            ));
        }

        let qid = open_call(api, goal_term)?;
        match next_solution(api, qid) {
            StepStatus::Last => {
                let atom = term_to_atom(result_term, api, &mut HashMap::new())?;
                api.close_query(qid);
                api.discard_foreign_frame(frame);
                FunctionOutcome::Single(atom)
            }
            StepStatus::True => {
                let first = term_to_atom(result_term, api, &mut HashMap::new())?;
                let atoms = collect_rest(api, qid, result_term, first);
                api.close_query(qid);
                api.discard_foreign_frame(frame);
                FunctionOutcome::Stream(atoms.into_iter())
            }
            StepStatus::False => {
                api.close_query(qid);
                api.discard_foreign_frame(frame);
                FunctionOutcome::Empty
            }
            StepStatus::Exception(msg) => {
                api.close_query(qid);
                api.discard_foreign_frame(frame);
                FunctionOutcome::Exception(msg)
            }
        }
    };

    unsafe { api.set_engine(old, std::ptr::null_mut()) };
    Ok(outcome)
}

/// Eagerly drain the remaining solutions of a query that already produced
/// `first`, mapping mid-stream failures to `(Error prolog_stream "...")` atoms.
fn collect_rest(api: &Api, qid: qid_t, result: term_t, first: Atom) -> Vec<Atom> {
    let mut out = vec![first];
    loop {
        match next_solution(api, qid) {
            StepStatus::True => match term_to_atom(result, api, &mut HashMap::new()) {
                Ok(atom) => out.push(atom),
                Err(e) => {
                    out.push(e.to_error_atom(&Atom::sym("prolog_stream")));
                    break;
                }
            },
            StepStatus::Last => {
                if let Ok(atom) = term_to_atom(result, api, &mut HashMap::new()) {
                    out.push(atom);
                }
                break;
            }
            StepStatus::False => break,
            StepStatus::Exception(msg) => {
                out.push(PrologError::Exception(msg).to_error_atom(&Atom::sym("prolog_stream")));
                break;
            }
        }
    }
    out
}

/// Assert a clause at the back (`assertz/1`) on the main engine.
pub fn assertz_clause(clause: &Atom) -> Result<(), PrologError> {
    let manager = engine::manager()?;
    manager.with_main(|api| {
        let frame = unsafe { api.open_foreign_frame() };
        let result = (|| {
            let clause_term = goal_to_term(clause, api, &mut VarMap::new())?;
            run_once(api, b"assertz\0", 1, clause_term, PL_Q_NORMAL).map(|_| ())
        })();
        unsafe { api.discard_foreign_frame(frame) };
        result
    })?
}

/// Remove the first matching clause (`retract/1`); returns whether it matched.
pub fn retract_clause(clause: &Atom) -> Result<bool, PrologError> {
    let manager = engine::manager()?;
    manager.with_main(|api| {
        let frame = unsafe { api.open_foreign_frame() };
        let result = (|| {
            let clause_term = goal_to_term(clause, api, &mut VarMap::new())?;
            Ok(run_once(api, b"retract\0", 1, clause_term, PL_Q_NORMAL)? != 0)
        })();
        unsafe { api.discard_foreign_frame(frame) };
        result
    })?
}

/// Consult a Prolog file on the main engine.
pub fn consult_file(path: &str) -> Result<(), PrologError> {
    let manager = engine::manager()?;
    manager.with_main(|api| {
        let frame = unsafe { api.open_foreign_frame() };
        let result = (|| {
            let c_path = CString::new(path)?;
            let path_term = unsafe { api.new_term_ref() };
            unsafe { api.put_atom_chars(path_term, c_path.as_ptr()) };
            let rc = run_once(
                api,
                b"consult\0",
                1,
                path_term,
                PL_Q_CATCH_EXCEPTION | PL_Q_NORMAL,
            )?;
            if rc == 0 {
                return Err(PrologError::Internal(format!("consult failed for {path}")));
            }
            Ok(())
        })();
        unsafe { api.discard_foreign_frame(frame) };
        result
    })?
}

/// Find all defined arities for a predicate name in the `user` module.
pub fn current_arities(name: &str) -> Result<Vec<usize>, PrologError> {
    let manager = engine::manager()?;
    let pooled = manager.checkout()?;
    with_engine(pooled.engine(), |api| -> Result<Vec<usize>, PrologError> {
        unsafe {
            let frame = api.open_foreign_frame();
            let c_name = CString::new(name)?;
            let slash_args = api.new_term_refs(2);
            api.put_atom(slash_args, api.new_atom(c_name.as_ptr()));
            let arity_term = slash_args + 1;
            api.put_variable(arity_term);
            let spec_term = api.new_term_ref();
            let slash_atom = api.new_atom(b"/\0".as_ptr() as *const _);
            api.cons_functor_v(spec_term, api.new_functor(slash_atom, 2), slash_args);

            let p = api.predicate(
                b"current_predicate\0".as_ptr() as *const _,
                1,
                b"system\0".as_ptr() as *const _,
            );
            let user_mod = api.new_module(api.new_atom(b"user\0".as_ptr() as *const _));
            let q = api.open_query(user_mod, PL_Q_NORMAL, p, spec_term);
            let mut arities = BTreeSet::new();
            if q != 0 {
                while api.next_solution(q) != 0 {
                    let mut a: i64 = 0;
                    if api.get_int64(arity_term, &mut a) != 0 && a >= 0 {
                        arities.insert(a as usize);
                    }
                }
                api.close_query(q);
            }
            api.discard_foreign_frame(frame);
            Ok(arities.into_iter().collect())
        }
    })?
}

/// Execute `translatePredicate` with input bindings, returning one binding set
/// per Prolog solution.
pub fn translate_goal(
    goal: &Atom,
    in_bindings: &[(String, Atom)],
) -> Result<Vec<Vec<(String, Atom)>>, PrologError> {
    run_goal(goal, in_bindings, collect_bindings)
}

/// Convert every variable in `var_map` back into a MeTTa atom.
fn collect_bindings(api: &Api, var_map: &VarMap) -> Vec<(String, Atom)> {
    let mut rev_map = HashMap::new();
    var_map
        .iter()
        .filter_map(|(name, &term)| {
            term_to_atom(term, api, &mut rev_map)
                .ok()
                .map(|atom| (name.clone(), atom))
        })
        .collect()
}

/// Sync space atoms into Prolog dynamic clauses for `'&self'` on the main engine.
pub fn sync_space(atoms: &[Atom]) -> Result<(), PrologError> {
    let manager = engine::manager()?;
    manager.with_main(|api| -> Result<(), PrologError> {
        unsafe {
            let frame = api.open_foreign_frame();
            let c_code = CString::new(
                "forall(current_predicate('&self'/A), (functor(H, '&self', A), retractall(H))).",
            )
            .unwrap();
            let term = api.new_term_ref();
            api.chars_to_term(c_code.as_ptr(), term);
            let user_mod = api.new_module(api.new_atom(b"user\0".as_ptr() as *const _));
            api.call(term, user_mod);

            for atom in atoms {
                let Atom::Expr(items) = atom else { continue };
                if items.is_empty() {
                    continue;
                }
                let mut var_map = VarMap::new();
                let a0 = api.new_term_refs(items.len() as c_int);
                let mut ok = true;
                for (i, it) in items.iter().enumerate() {
                    if let Ok(t) = atom_to_term(it, api, &mut var_map) {
                        api.put_term(a0 + i, t);
                    } else {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    continue;
                }
                let clause_term = api.new_term_ref();
                let self_atom = api.new_atom(b"&self\0".as_ptr() as *const _);
                if api.cons_functor_v(clause_term, api.new_functor_sz(self_atom, items.len()), a0)
                    == 0
                {
                    continue;
                }
                let _ = run_once(api, b"assertz\0", 1, clause_term, PL_Q_NORMAL);
            }
            api.discard_foreign_frame(frame);
            Ok(())
        }
    })?
}
