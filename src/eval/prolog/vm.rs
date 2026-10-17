use crate::atom::Atom;
use crate::env::{Env, EnvNode};
use crate::eval::machine::budget::ResultSet;
use crate::eval::prolog::ops;
use crate::eval::shared::env::lookup;

pub fn consult_file(path: &str) -> Result<(), String> {
    ops::consult_file(path).map_err(|e| e.to_string())
}

pub fn sync_space(atoms: &[Atom]) -> Result<(), String> {
    ops::sync_space(atoms).map_err(|e| e.to_string())
}

pub fn eval_translate_predicate_atom(
    goal: &Atom,
    base_env: &Env,
    free_vars_map: &[String],
    free_vars_bindings: &[Atom],
) -> Result<ResultSet, String> {
    let in_bindings = gather_bindings(base_env, free_vars_map, free_vars_bindings);
    let solutions = ops::translate_goal(goal, &in_bindings).map_err(|e| e.to_string())?;
    let mut envs = Vec::with_capacity(solutions.len());
    for solution in solutions {
        let mut env = base_env.clone();
        for (vname, vval) in solution {
            if matches!(&vval, Atom::Sym(s) if s.starts_with("$_V")) {
                continue;
            }
            env = env.extend(&vname, vval.clone());
            for (orig_name, fresh) in free_vars_map.iter().zip(free_vars_bindings) {
                let Atom::Sym(fresh_s) = fresh else { continue };
                if orig_name == &vname {
                    env = env.extend(fresh_s, vval.clone());
                } else if fresh_s.as_str() == vname {
                    env = env.extend(orig_name, vval.clone());
                }
            }
        }
        envs.push((Atom::sym("true"), env));
    }
    Ok(envs)
}

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
            EnvNode::Empty => match todo.pop() {
                Some(next) => curr = next,
                None => break,
            },
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
        let Some(Atom::Sym(fresh_s)) = free_vars_bindings.get(idx) else { continue };
        let Some(bound_val) = lookup(base_env, fresh_s) else { continue };
        if seen.insert(name.clone()) {
            bindings.push((name.clone(), bound_val.clone()));
        }
        if seen.insert(fresh_s.to_string()) {
            bindings.push((fresh_s.to_string(), bound_val));
        }
    }
    bindings
}
