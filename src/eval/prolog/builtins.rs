//! MeTTa builtin functions for interacting with Prolog.

use crate::atom::Atom;
use crate::builtins::arithmetic::expect_n_args;
use crate::eval::prolog::ops;
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
        let solutions = ops::call_predicate_goal(&args[0]).map_err(|e| e.to_string())?;
        match solutions.len() {
            0 => Ok(NDet::empty()),
            1 => Ok(NDet::single(Atom::sym("True"))),
            n => Ok(NDet::stream((0..n).map(|_| Atom::sym("True")))),
        }
    });

    // (succeedsPredicate G) -> True if at least one solution, else False.
    funcs.insert_native("succeedsPredicate", 1, |args, _| {
        expect_n_args(args, 1, "succeedsPredicate")?;
        let solutions = ops::call_predicate_goal(&args[0]).map_err(|e| e.to_string())?;
        let answer = if solutions.is_empty() {
            "False"
        } else {
            "True"
        };
        Ok(NDet::single(Atom::sym(answer)))
    });

    // (assertzPredicate G) -> True
    funcs.insert_native("assertzPredicate", 1, |args, _| {
        expect_n_args(args, 1, "assertzPredicate")?;
        ops::assertz_clause(&args[0]).map_err(|e| e.to_string())?;
        Ok(NDet::single(Atom::sym("True")))
    });

    // (retractPredicate G) -> True if retracted, False otherwise
    funcs.insert_native("retractPredicate", 1, |args, _| {
        expect_n_args(args, 1, "retractPredicate")?;
        let removed = ops::retract_clause(&args[0]).map_err(|e| e.to_string())?;
        Ok(NDet::single(Atom::sym(if removed {
            "True"
        } else {
            "False"
        })))
    });

    // (import_prolog_function name) -> True
    funcs.insert_native("import_prolog_function", 1, |args, funcs_ref| {
        expect_n_args(args, 1, "import_prolog_function")?;
        let name_str = match &args[0] {
            Atom::Sym(s) => s.as_str().to_string(),
            Atom::Str(s) => s.as_ref().to_string(),
            other => {
                return Err(format!(
                    "import_prolog_function expects symbol or string, got {other:?}"
                ));
            }
        };

        // Remember this name for call-time dynamic resolution of future arities.
        funcs_ref
            .imported_prolog_funcs
            .write()
            .unwrap()
            .insert(name_str.clone());

        if let Ok(arities) = ops::current_arities(&name_str) {
            for arity in arities {
                if arity >= 1 {
                    register_imported_prolog_function(funcs_ref, &name_str, (arity - 1) as u8);
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
            other => {
                return Err(format!(
                    "add-translator-rule! expects symbol or string, got {other:?}"
                ));
            }
        };
        funcs_ref
            .translator_rules
            .write()
            .unwrap()
            .insert(rule_name);
        funcs_ref.invalidate_caches();
        Ok(NDet::single(Atom::sym("True")))
    });
}

/// Register a native binding for an imported Prolog function `name/arity`.
pub fn register_imported_prolog_function(funcs: &FnTable, functor: &str, arity: u8) {
    let functor_name = functor.to_string();
    funcs.insert_native(
        functor,
        arity,
        move |call_args, _| match ops::call_function(&functor_name, call_args)
            .map_err(|e| e.to_string())?
        {
            ops::FunctionOutcome::Single(atom) => Ok(NDet::single(atom)),
            ops::FunctionOutcome::Stream(stream) => Ok(NDet::stream(stream)),
            ops::FunctionOutcome::Empty => Ok(NDet::empty()),
            ops::FunctionOutcome::Exception(msg) => Err(msg),
        },
    );
    funcs.invalidate_caches();
}

/// Dynamically resolve an imported Prolog function at call time if not
/// registered yet.
pub fn try_resolve_imported_prolog_function(funcs: &FnTable, name: &str, arity: u8) -> bool {
    let is_imported = funcs.imported_prolog_funcs.read().unwrap().contains(name);
    if !is_imported {
        return false;
    }

    if let Ok(arities) = ops::current_arities(name) {
        if arities.contains(&(arity as usize + 1)) {
            register_imported_prolog_function(funcs, name, arity);
            return true;
        }
    }
    false
}
