//! Internal Prolog operations.
//!
//! Each operation attaches an engine to the calling thread for its duration:
//! one-shot queries and imports check out a pooled engine, while mutations
//! (assert/retract/consult/space sync) run on the mutex-guarded main engine.

use std::collections::{BTreeSet, HashMap};
use std::ffi::{CStr, CString, c_char, c_int};

use crate::atom::Atom;
use crate::eval::prolog::convert::{VarMap, atom_to_term, goal_to_term, term_to_atom};
use crate::eval::prolog::engine::{self, with_engine};
use crate::eval::prolog::error::PrologError;
use crate::eval::prolog::ffi::{
    Api, CVT_WRITE, PL_Q_CATCH_EXCEPTION, PL_Q_EXT_STATUS, PL_Q_NODEBUG, PL_Q_NORMAL,
    PL_S_EXCEPTION, PL_S_FALSE, PL_S_LAST, PL_S_TRUE, qid_t, term_t,
};
use crate::eval::prolog::stream::PrologStream;

/// Outcome of invoking an imported Prolog function.
pub enum FunctionOutcome {
    /// Deterministic solution (last answer); engine already returned to pool.
    Single(Atom),
    /// Open choicepoint; the returned stream owns the engine and query.
    Stream(PrologStream),
    /// Goal failed.
    Empty,
    /// Uncaught Prolog exception.
    Exception(String),
}

/// Result of stepping an open stream.
pub enum StepStreamResult {
    Answer(Atom, bool /* is_last */),
    Done,
    Error(PrologError),
}

/// Result of a single `PL_next_solution` step.
enum StepStatus {
    True,
    Last,
    False,
    Exception(String),
}

/// Step a query once, converting Prolog exceptions into a readable message.
fn next_solution(api: &Api, qid: qid_t) -> StepStatus {
    let rc = unsafe { api.next_solution(qid) };
    match rc {
        PL_S_LAST => StepStatus::Last,
        PL_S_TRUE => StepStatus::True,
        PL_S_FALSE => StepStatus::False,
        PL_S_EXCEPTION => StepStatus::Exception(read_exception(api, qid)),
        _ => StepStatus::False,
    }
}

/// Read and clear the exception attached to an open query.
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

/// Open `call(Goal)` on the currently attached engine with exception catching.
fn open_call(api: &Api, goal: term_t) -> Result<qid_t, PrologError> {
    unsafe {
        let p_call = api.predicate(
            b"call\0".as_ptr() as *const _,
            1,
            b"system\0".as_ptr() as *const _,
        );
        let flags = PL_Q_CATCH_EXCEPTION | PL_Q_EXT_STATUS | PL_Q_NODEBUG;
        let qid = api.open_query(0, flags, p_call, goal);
        if qid == 0 {
            return Err(PrologError::Internal("PL_open_query returned NULL".into()));
        }
        Ok(qid)
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
            let p_open_str = api.predicate(
                b"open_string\0".as_ptr() as *const _,
                2,
                b"system\0".as_ptr() as *const _,
            );
            let sargs = api.new_term_refs(2);
            let c_code = CString::new(code).map_err(|e| PrologError::Conversion(e.to_string()))?;
            api.put_string_chars(sargs, c_code.as_ptr());

            let q = api.open_query(0, PL_Q_NORMAL, p_open_str, sargs);
            if q == 0 || api.next_solution(q) == 0 {
                if q != 0 {
                    api.close_query(q);
                }
                api.discard_foreign_frame(frame);
                return Err(PrologError::Internal("failed to open_string".into()));
            }
            api.cut_query(q);

            let stream_term = sargs + 1;
            let lf_args = api.new_term_refs(2);
            api.put_atom_chars(lf_args, b"user\0".as_ptr() as *const _);

            let opt = api.new_term_ref();
            let stream_atom = api.new_atom(b"stream\0".as_ptr() as *const _);
            let stream_functor = api.new_functor(stream_atom, 1);
            api.cons_functor_v(opt, stream_functor, stream_term);

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

/// Execute `call(Goal)` and return `true` per solution.
pub fn call_predicate_goal(goal: &Atom) -> Result<Vec<Atom>, PrologError> {
    let manager = engine::manager()?;
    let pooled = manager.checkout()?;
    with_engine(pooled.engine(), |api| -> Result<Vec<Atom>, PrologError> {
        unsafe {
            let frame = api.open_foreign_frame();
            let mut var_map = VarMap::new();
            let goal_term = match goal_to_term(goal, api, &mut var_map) {
                Ok(t) => t,
                Err(e) => {
                    api.discard_foreign_frame(frame);
                    return Err(e);
                }
            };
            let qid = open_call(api, goal_term)?;

            let mut solutions = Vec::new();
            loop {
                match next_solution(api, qid) {
                    StepStatus::True => solutions.push(Atom::sym("true")),
                    StepStatus::Last => {
                        solutions.push(Atom::sym("true"));
                        break;
                    }
                    StepStatus::False => break,
                    StepStatus::Exception(msg) => {
                        api.close_query(qid);
                        api.discard_foreign_frame(frame);
                        return Err(PrologError::Exception(msg));
                    }
                }
            }
            api.close_query(qid);
            api.discard_foreign_frame(frame);
            Ok(solutions)
        }
    })?
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
        let head_atom = api.new_atom(c_head.as_ptr());
        let arity = args.len() + 1;
        let functor = api.new_functor_sz(head_atom, arity);

        let a0 = api.new_term_refs(arity as c_int);
        for (i, arg) in args.iter().enumerate() {
            let t = atom_to_term(arg, api, &mut var_map)?;
            api.put_term(a0 + i, t);
        }
        let result_term = a0 + args.len();
        api.put_variable(result_term);

        let goal_term = api.new_term_ref();
        if api.cons_functor_v(goal_term, functor, a0) == 0 {
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
                let atom = term_to_atom(result_term, api, &mut HashMap::new())?;
                // Leave the query open; the stream takes ownership of the
                // engine, frame, query and result term.
                api.set_engine(old, std::ptr::null_mut());
                return Ok(FunctionOutcome::Stream(PrologStream::new(
                    pooled,
                    qid,
                    frame,
                    result_term,
                    Some(atom),
                )));
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

/// Step an open query, converting the result term on success.
pub fn step_query(api: &Api, qid: qid_t, result: term_t) -> StepStreamResult {
    match next_solution(api, qid) {
        StepStatus::True => match term_to_atom(result, api, &mut HashMap::new()) {
            Ok(atom) => StepStreamResult::Answer(atom, false),
            Err(e) => StepStreamResult::Error(e),
        },
        StepStatus::Last => match term_to_atom(result, api, &mut HashMap::new()) {
            Ok(atom) => StepStreamResult::Answer(atom, true),
            Err(e) => StepStreamResult::Error(e),
        },
        StepStatus::False => StepStreamResult::Done,
        StepStatus::Exception(msg) => StepStreamResult::Error(PrologError::Exception(msg)),
    }
}

/// Assert a clause at the back (`assertz/1`) on the main engine.
pub fn assertz_clause(clause: &Atom) -> Result<(), PrologError> {
    assert_clause(clause, b"assertz\0")
}

fn assert_clause(clause: &Atom, pred_name: &[u8]) -> Result<(), PrologError> {
    let manager = engine::manager()?;
    manager.with_main(|api| -> Result<(), PrologError> {
        unsafe {
            let frame = api.open_foreign_frame();
            let mut var_map = VarMap::new();
            let clause_term = goal_to_term(clause, api, &mut var_map)?;
            let p = api.predicate(
                pred_name.as_ptr() as *const _,
                1,
                b"system\0".as_ptr() as *const _,
            );
            let q = api.open_query(0, PL_Q_NORMAL, p, clause_term);
            if q != 0 {
                api.next_solution(q);
                api.close_query(q);
            }
            api.discard_foreign_frame(frame);
            Ok(())
        }
    })?
}

/// Remove the first matching clause (`retract/1`); returns whether it matched.
pub fn retract_clause(clause: &Atom) -> Result<bool, PrologError> {
    let manager = engine::manager()?;
    manager.with_main(|api| -> Result<bool, PrologError> {
        unsafe {
            let frame = api.open_foreign_frame();
            let mut var_map = VarMap::new();
            let clause_term = goal_to_term(clause, api, &mut var_map)?;
            let p = api.predicate(
                b"retract\0".as_ptr() as *const _,
                1,
                b"system\0".as_ptr() as *const _,
            );
            let q = api.open_query(0, PL_Q_NORMAL, p, clause_term);
            let removed = if q != 0 {
                let rc = api.next_solution(q);
                api.close_query(q);
                rc != 0
            } else {
                false
            };
            api.discard_foreign_frame(frame);
            Ok(removed)
        }
    })?
}

/// Consult a Prolog file on the main engine.
pub fn consult_file(path: &str) -> Result<(), PrologError> {
    let manager = engine::manager()?;
    manager.with_main(|api| -> Result<(), PrologError> {
        unsafe {
            let frame = api.open_foreign_frame();
            let c_path = CString::new(path)?;
            let path_term = api.new_term_ref();
            api.put_atom_chars(path_term, c_path.as_ptr());

            let p = api.predicate(
                b"consult\0".as_ptr() as *const _,
                1,
                b"system\0".as_ptr() as *const _,
            );
            let q = api.open_query(0, PL_Q_CATCH_EXCEPTION | PL_Q_NORMAL, p, path_term);
            if q == 0 {
                api.discard_foreign_frame(frame);
                return Err(PrologError::Internal(format!(
                    "failed to open consult query for {path}"
                )));
            }
            let rc = api.next_solution(q);
            if rc == PL_S_EXCEPTION {
                let msg = read_exception(api, q);
                api.close_query(q);
                api.discard_foreign_frame(frame);
                return Err(PrologError::Exception(msg));
            }
            api.close_query(q);
            api.discard_foreign_frame(frame);
            if rc == 0 {
                return Err(PrologError::Internal(format!("consult failed for {path}")));
            }
            Ok(())
        }
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
            let name_atom = api.new_atom(c_name.as_ptr());

            let slash_atom = api.new_atom(b"/\0".as_ptr() as *const _);
            let slash_functor = api.new_functor(slash_atom, 2);

            let slash_args = api.new_term_refs(2);
            api.put_atom(slash_args, name_atom);
            let arity_term = slash_args + 1;
            api.put_variable(arity_term);

            let spec_term = api.new_term_ref();
            api.cons_functor_v(spec_term, slash_functor, slash_args);

            let p = api.predicate(
                b"current_predicate\0".as_ptr() as *const _,
                1,
                b"system\0".as_ptr() as *const _,
            );
            let user_atom = api.new_atom(b"user\0".as_ptr() as *const _);
            let user_mod = api.new_module(user_atom);
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
    let manager = engine::manager()?;
    let pooled = manager.checkout()?;
    with_engine(
        pooled.engine(),
        |api| -> Result<Vec<Vec<(String, Atom)>>, PrologError> {
            unsafe {
                let frame = api.open_foreign_frame();
                let mut var_map = VarMap::new();

                for (vname, vval) in in_bindings {
                    let val_term = atom_to_term(vval, api, &mut var_map)?;
                    var_map.insert(vname.clone(), val_term);
                }

                let goal_term = match goal_to_term(goal, api, &mut var_map) {
                    Ok(t) => t,
                    Err(e) => {
                        api.discard_foreign_frame(frame);
                        return Err(e);
                    }
                };

                let qid = open_call(api, goal_term)?;
                let mut all_solutions = Vec::new();
                loop {
                    match next_solution(api, qid) {
                        StepStatus::True => {
                            all_solutions.push(collect_bindings(api, &var_map));
                        }
                        StepStatus::Last => {
                            all_solutions.push(collect_bindings(api, &var_map));
                            break;
                        }
                        StepStatus::False => break,
                        StepStatus::Exception(msg) => {
                            api.close_query(qid);
                            api.discard_foreign_frame(frame);
                            return Err(PrologError::Exception(msg));
                        }
                    }
                }
                api.close_query(qid);
                api.discard_foreign_frame(frame);
                Ok(all_solutions)
            }
        },
    )?
}

/// Convert every variable in `var_map` back into a MeTTa atom.
fn collect_bindings(api: &Api, var_map: &VarMap) -> Vec<(String, Atom)> {
    let mut bindings = Vec::new();
    let mut rev_map = HashMap::new();
    for (name, &term) in var_map {
        if let Ok(atom) = term_to_atom(term, api, &mut rev_map) {
            bindings.push((name.clone(), atom));
        }
    }
    bindings
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
            let user_atom = api.new_atom(b"user\0".as_ptr() as *const _);
            let user_mod = api.new_module(user_atom);
            api.call(term, user_mod);

            for atom in atoms {
                if let Atom::Expr(items) = atom {
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
                    let self_atom = api.new_atom(b"&self\0".as_ptr() as *const _);
                    let self_functor = api.new_functor_sz(self_atom, items.len());
                    let clause_term = api.new_term_ref();
                    if api.cons_functor_v(clause_term, self_functor, a0) == 0 {
                        continue;
                    }
                    let p = api.predicate(
                        b"assertz\0".as_ptr() as *const _,
                        1,
                        b"system\0".as_ptr() as *const _,
                    );
                    let q = api.open_query(0, PL_Q_NORMAL, p, clause_term);
                    if q != 0 {
                        api.next_solution(q);
                        api.close_query(q);
                    }
                }
            }
            api.discard_foreign_frame(frame);
            Ok(())
        }
    })?
}
