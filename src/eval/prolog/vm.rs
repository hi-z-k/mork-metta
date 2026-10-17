//! Prolog-specific logic called by the shared VM.
//!
//! The VM keeps a thin dispatch stub (see `run.rs`); everything that knows about
//! Prolog — consulting files, syncing `&self`, marshalling `translatePredicate`
//! environments across the bridge — lives here.

use crate::atom::Atom;
use crate::env::{Env, EnvNode};
use crate::eval::machine::budget::ResultSet;
use crate::eval::prolog::ops;
use crate::eval::shared::env::lookup;
use crate::parser::Expr;

/// Consult a `.pl` file through the bridge.
pub fn consult_file(path: &str) -> Result<(), String> {
    ops::consult_file(path).map_err(|e| e.to_string())
}

/// Push the current space atoms into Prolog's `'&self'` dynamic clauses.
pub fn sync_space(atoms: &[Atom]) -> Result<(), String> {
    ops::sync_space(atoms).map_err(|e| e.to_string())
}

/// Execute a `translatePredicate` goal and return one branch environment per
/// Prolog solution, mirroring the frozen semantics:
///
/// * sub-terms are substituted, not eagerly reduced, and even a bare symbol
///   goal is passed through `translate`;
/// * every unbound variable is forwarded in and any solution binding is mapped
///   back onto both the original name and its fresh counterpart.
pub fn eval_translate_predicate(
    goal: &Expr,
    base_env: &Env,
    free_vars_map: &[String],
    free_vars_bindings: &[Atom],
) -> Result<ResultSet, String> {
    let evaluated_goal = build_goal(goal, base_env, free_vars_map, free_vars_bindings);
    let in_bindings = gather_bindings(base_env, free_vars_map, free_vars_bindings);

    let solutions =
        ops::translate_goal(&evaluated_goal, &in_bindings).map_err(|e| e.to_string())?;
    if solutions.is_empty() {
        return Ok(Vec::new());
    }

    let mut envs = Vec::with_capacity(solutions.len());
    for solution in solutions {
        let mut env = base_env.clone();
        for (vname, vval) in solution {
            let is_unbound_dummy = matches!(&vval, Atom::Sym(s) if s.starts_with("$_V"));
            if is_unbound_dummy {
                continue;
            }
            env = env.extend(&vname, vval.clone());
            for (idx, orig_name) in free_vars_map.iter().enumerate() {
                if orig_name == &vname {
                    if let Some(Atom::Sym(fresh_s)) = free_vars_bindings.get(idx) {
                        env = env.extend(fresh_s, vval.clone());
                    }
                }
                if let Some(Atom::Sym(fresh_s)) = free_vars_bindings.get(idx) {
                    if fresh_s.as_str() == vname.as_str() {
                        env = env.extend(orig_name, vval.clone());
                    }
                }
            }
        }
        envs.push((Atom::sym("true"), env));
    }
    Ok(envs)
}

/// Substitute bound variables into the goal expression without evaluating.
fn build_goal(
    goal: &Expr,
    base_env: &Env,
    free_vars_map: &[String],
    free_vars_bindings: &[Atom],
) -> Atom {
    fn rec(e: &Expr, env: &Env, fv_map: &[String], fv_bind: &[Atom]) -> Atom {
        match e {
            Expr::Symbol(s) if s.starts_with('$') => {
                if let Some(val) = lookup(env, s) {
                    val
                } else if let Some(pos) = fv_map.iter().position(|x| x == s) {
                    match fv_bind.get(pos) {
                        Some(Atom::Sym(fresh_s)) => {
                            lookup(env, fresh_s).unwrap_or_else(|| Atom::sym(fresh_s))
                        }
                        Some(other) => other.clone(),
                        None => Atom::sym(s),
                    }
                } else {
                    Atom::sym(s)
                }
            }
            Expr::Symbol(s) => Atom::sym(s),
            Expr::Str(s) => Atom::str_val(s),
            Expr::Number(n) => Atom::Num(n.clone()),
            Expr::List(items) => {
                let collected: Vec<Atom> = items
                    .iter()
                    .map(|it| rec(it, env, fv_map, fv_bind))
                    .collect();
                Atom::expr(collected)
            }
        }
    }

    match goal {
        Expr::List(items) if !items.is_empty() => {
            let head = match &items[0] {
                Expr::Symbol(s) => s.clone(),
                Expr::Str(s) => s.clone(),
                other => other.to_string(),
            };
            let args: Vec<Atom> = items[1..]
                .iter()
                .map(|arg| rec(arg, base_env, free_vars_map, free_vars_bindings))
                .collect();
            if args.is_empty() {
                Atom::sym(&head)
            } else {
                let mut list = vec![Atom::sym(&head)];
                list.extend(args);
                Atom::expr(list)
            }
        }
        // A bare (non-list) goal is textualised, matching the original bridge.
        _ => Atom::sym(&rec(goal, base_env, free_vars_map, free_vars_bindings).to_sexpr_string()),
    }
}

/// Flatten the MeTTa environment plus fresh free-variable bindings into the
/// `(name, value)` pairs forwarded to Prolog.
fn gather_bindings(
    base_env: &Env,
    free_vars_map: &[String],
    free_vars_bindings: &[Atom],
) -> Vec<(String, Atom)> {
    let mut bindings = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut curr = base_env.clone();
    let mut todo = Vec::new();
    loop {
        match curr.inner() {
            EnvNode::Empty => {
                if let Some(next) = todo.pop() {
                    curr = next;
                    continue;
                }
                break;
            }
            EnvNode::Cons { name, value, next } => {
                if seen.insert(name.to_string()) {
                    bindings.push((name.to_string(), (**value).clone()));
                }
                curr = next.clone();
            }
            EnvNode::Link { prefix, base } => {
                todo.push(base.clone());
                curr = prefix.clone();
            }
        }
    }

    for (idx, name) in free_vars_map.iter().enumerate() {
        if let Some(Atom::Sym(fresh_s)) = free_vars_bindings.get(idx) {
            if let Some(bound_val) = lookup(base_env, fresh_s) {
                if seen.insert(name.clone()) {
                    bindings.push((name.clone(), bound_val.clone()));
                }
                if seen.insert(fresh_s.to_string()) {
                    bindings.push((fresh_s.to_string(), bound_val));
                }
            }
        }
    }

    bindings
}
