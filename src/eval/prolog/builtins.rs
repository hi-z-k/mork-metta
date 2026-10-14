//! MeTTa builtin functions for interacting with Prolog.

use crate::atom::Atom;
use crate::builtins::arithmetic::expect_n_args;
use crate::eval::prolog::stream::PrologStream;
use crate::eval::prolog::thread::{client, PrologCommand, PrologReply};
use crate::func::{FnTable, NDet};

/// Register all Prolog builtins into `funcs`.
pub fn register_prolog_builtins(funcs: &FnTable) {
    // (Predicate (f args...)) -> (Predicate (f args...))
    funcs.insert_native("Predicate", 1, |args, _| {
        expect_n_args(args, 1, "Predicate")?;
        Ok(NDet::single(Atom::expr(vec![
            Atom::sym("Predicate"),
            args[0].clone(),
        ])))
    });

    // (callPredicate G) -> True for each solution, empty on failure
    funcs.insert_native("callPredicate", 1, |args, _| {
        expect_n_args(args, 1, "callPredicate")?;
        let client = client().map_err(|e| e.to_string())?;
        match client.request(PrologCommand::CallPredicate {
            goal: args[0].clone(),
        }) {
            Ok(PrologReply::Solutions(solutions)) => {
                if solutions.is_empty() {
                    Ok(NDet::empty())
                } else if solutions.len() == 1 {
                    Ok(NDet::single(Atom::sym("True")))
                } else {
                    let count = solutions.len();
                    Ok(NDet::stream((0..count).map(|_| Atom::sym("True"))))
                }
            }
            Ok(PrologReply::Error(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
            _ => Ok(NDet::empty()),
        }
    });

    // (succeedsPredicate G) -> True if at least one solution, False on failure (boolean for `if`)
    funcs.insert_native("succeedsPredicate", 1, |args, _| {
        expect_n_args(args, 1, "succeedsPredicate")?;
        let client = client().map_err(|e| e.to_string())?;
        match client.request(PrologCommand::CallPredicate {
            goal: args[0].clone(),
        }) {
            Ok(PrologReply::Solutions(solutions)) => {
                if solutions.is_empty() {
                    Ok(NDet::single(Atom::sym("False")))
                } else {
                    Ok(NDet::single(Atom::sym("True")))
                }
            }
            Ok(PrologReply::Error(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
            _ => Ok(NDet::single(Atom::sym("False"))),
        }
    });

    // (assertaPredicate G) -> True
    funcs.insert_native("assertaPredicate", 1, |args, _| {
        expect_n_args(args, 1, "assertaPredicate")?;
        let client = client().map_err(|e| e.to_string())?;
        match client.request(PrologCommand::Asserta {
            clause: args[0].clone(),
        }) {
            Ok(PrologReply::Bool(true)) => Ok(NDet::single(Atom::sym("True"))),
            Ok(PrologReply::Error(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
            _ => Ok(NDet::single(Atom::sym("False"))),
        }
    });

    // (assertzPredicate G) -> True
    funcs.insert_native("assertzPredicate", 1, |args, _| {
        expect_n_args(args, 1, "assertzPredicate")?;
        let client = client().map_err(|e| e.to_string())?;
        match client.request(PrologCommand::Assertz {
            clause: args[0].clone(),
        }) {
            Ok(PrologReply::Bool(true)) => Ok(NDet::single(Atom::sym("True"))),
            Ok(PrologReply::Error(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
            _ => Ok(NDet::single(Atom::sym("False"))),
        }
    });

    // (retractPredicate G) -> True if retracted, False if not found
    funcs.insert_native("retractPredicate", 1, |args, _| {
        expect_n_args(args, 1, "retractPredicate")?;
        let client = client().map_err(|e| e.to_string())?;
        match client.request(PrologCommand::Retract {
            clause: args[0].clone(),
        }) {
            Ok(PrologReply::Bool(true)) => Ok(NDet::single(Atom::sym("True"))),
            Ok(PrologReply::Bool(false)) => Ok(NDet::single(Atom::sym("False"))),
            Ok(PrologReply::Error(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
            _ => Ok(NDet::single(Atom::sym("False"))),
        }
    });

    // (import_prolog_function name) -> True
    funcs.insert_native("import_prolog_function", 1, |args, funcs_ref| {
        expect_n_args(args, 1, "import_prolog_function")?;
        let name_str = match &args[0] {
            Atom::Sym(s) => s.as_str().to_string(),
            Atom::Str(s) => s.as_ref().to_string(),
            other => return Err(format!("import_prolog_function expects symbol or string, got {other:?}")),
        };

        // Remember this name for call-time dynamic resolution of future arities
        funcs_ref.imported_prolog_funcs.write().unwrap().insert(name_str.clone());

        let client = client().map_err(|e| e.to_string())?;
        if let Ok(PrologReply::Arities(arities)) = client.request(PrologCommand::CurrentArities {
            name: name_str.clone(),
        }) {
            for arity in arities {
                if arity >= 1 {
                    let metta_arity = (arity - 1) as u8;
                    register_imported_prolog_function(funcs_ref, &name_str, metta_arity);
                }
            }
        }

        Ok(NDet::single(Atom::sym("True")))
    });

    // (add-translator-rule! name) -> True
    funcs.insert_native("add-translator-rule!", 1, |args, funcs_ref| {
        expect_n_args(args, 1, "add-translator-rule!")?;
        let rule_name = match &args[0] {
            Atom::Sym(s) => s.as_str().to_string(),
            Atom::Str(s) => s.as_ref().to_string(),
            other => return Err(format!("add-translator-rule! expects symbol or string, got {other:?}")),
        };
        funcs_ref.translator_rules.write().unwrap().insert(rule_name);
        funcs_ref.invalidate_caches();
        Ok(NDet::single(Atom::sym("True")))
    });
}

/// Register a native binding for an imported Prolog function `name/arity`.
pub fn register_imported_prolog_function(funcs: &FnTable, functor: &str, arity: u8) {
    let functor_name = functor.to_string();
    funcs.insert_native(functor, arity, move |call_args, _| {
        let client = client().map_err(|e| e.to_string())?;
        match client.request(PrologCommand::CallFunction {
            functor: functor_name.clone(),
            args: call_args.to_vec(),
        }) {
            Ok(PrologReply::Atom(atom)) => Ok(NDet::single(atom)),
            Ok(PrologReply::StreamOpened {
                stream_id,
                first_answer,
            }) => {
                let stream = PrologStream::new(stream_id, first_answer);
                Ok(NDet::stream(stream))
            }
            Ok(PrologReply::Solutions(sols)) if sols.is_empty() => Ok(NDet::empty()),
            Ok(PrologReply::Error(err)) => Err(err.to_string()),
            Err(e) => Err(e.to_string()),
            _ => Ok(NDet::empty()),
        }
    });
    funcs.invalidate_caches();
}

/// Dynamically resolve an imported Prolog function at call time if not registered yet.
pub fn try_resolve_imported_prolog_function(funcs: &FnTable, name: &str, arity: u8) -> bool {
    let is_imported = funcs
        .imported_prolog_funcs
        .read()
        .unwrap()
        .contains(name);
    if !is_imported {
        return false;
    }

    let client = match client() {
        Ok(c) => c,
        Err(_) => return false,
    };

    if let Ok(PrologReply::Arities(arities)) = client.request(PrologCommand::CurrentArities {
        name: name.to_string(),
    }) {
        let target_prolog_arity = arity as usize + 1;
        if arities.contains(&target_prolog_arity) {
            register_imported_prolog_function(funcs, name, arity);
            return true;
        }
    }
    false
}
