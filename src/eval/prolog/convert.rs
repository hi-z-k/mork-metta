//! Conversion between MeTTa atoms and Prolog terms.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_uint};
use std::sync::Arc;

use crate::atom::{Atom, Numeric};
use crate::eval::prolog::error::PrologError;
use crate::eval::prolog::ffi::{
    Api, CVT_ATOM, CVT_INTEGER, CVT_STRING, PL_ATOM, PL_FLOAT, PL_INTEGER, PL_LIST_PAIR, PL_NIL,
    PL_STRING, PL_TERM, PL_VARIABLE, atom_t, term_t,
};
use crate::eval::prolog::handle::PrologForeign;

/// Maps variable names (e.g. `"$x"`) to Prolog term references.
pub type VarMap = HashMap<String, term_t>;

/// Strip outer `(quote ...)` from an atom.
pub fn strip_quote_atom(atom: &Atom) -> &Atom {
    if let Atom::Expr(e) = atom {
        if e.len() == 2 && is_sym(&e[0], "quote") {
            return &e[1];
        }
    }
    atom
}

fn is_sym(a: &Atom, name: &str) -> bool {
    matches!(a.as_sym(), Ok(s) if s == name)
}

fn conv<T>(r: Result<T, impl ToString>) -> Result<T, PrologError> {
    r.map_err(|e| PrologError::Conversion(e.to_string()))
}

fn get_str(api: &Api, term: term_t, kind: c_uint) -> Result<String, PrologError> {
    let mut ptr: *const c_char = std::ptr::null();
    if unsafe { api.get_chars(term, &mut ptr, kind) } == 0 || ptr.is_null() {
        return Err(PrologError::Conversion("failed to read Prolog string".into()));
    }
    conv(unsafe { CStr::from_ptr(ptr) }.to_str()).map(str::to_string)
}

/// Convert a MeTTa `Atom` to a Prolog `term_t`.
pub fn atom_to_term(atom: &Atom, api: &Api, var_map: &mut VarMap) -> Result<term_t, PrologError> {
    let atom = strip_quote_atom(atom);
    let term = unsafe { api.new_term_ref() };
    match atom {
        Atom::Sym(s) => {
            let name = s.as_str();
            if name.starts_with('$') {
                return Ok(match var_map.get(name) {
                    Some(&t) => t,
                    None => {
                        unsafe { api.put_variable(term) };
                        var_map.insert(name.to_string(), term);
                        term
                    }
                });
            }
            let c_name = CString::new(name)?;
            unsafe { api.put_atom(term, api.new_atom(c_name.as_ptr())) };
        }
        Atom::Str(s) => {
            let c_s = CString::new(s.as_ref())?;
            unsafe { api.put_string_chars(term, c_s.as_ptr()) };
        }
        Atom::Num(Numeric::Int(i)) => {
            if let Ok(v) = i64::try_from(i) {
                unsafe { api.put_int64(term, v) };
            } else {
                let c_s = CString::new(i.to_string())?;
                if unsafe { api.chars_to_term(c_s.as_ptr(), term) } == 0 {
                    return Err(PrologError::Conversion("failed to parse big int".into()));
                }
            }
        }
        Atom::Num(Numeric::Dec(d)) => {
            unsafe { api.put_float(term, d.to_f64().value()) };
        }
        Atom::Expr(e) => {
            if let Some((head, args)) = extract_predicate_compound(e) {
                return compound(api, head, &args, var_map);
            }
            if e.len() == 2 && is_sym(&e[0], "library") {
                return compound(api, "library", &[&e[1]], var_map);
            }
            if let Some(Atom::Sym(head)) = e.first() {
                if is_prolog_operator(head.as_str()) {
                    let args: Vec<&Atom> = e[1..].iter().collect();
                    return compound(api, head.as_str(), &args, var_map);
                }
            }
            return list_to_term(api, e, var_map);
        }
        Atom::Gnd(g) => {
            let Some(pf) = g.as_any().downcast_ref::<PrologForeign>() else {
                return Err(PrologError::Conversion("unsupported grounded type".into()));
            };
            if unsafe { api.recorded(pf.record, term) } == 0 {
                return Err(PrologError::Conversion("failed to replay recorded term".into()));
            }
        }
        _ => return Err(PrologError::Conversion("unsupported atom type".into())),
    }
    Ok(term)
}

/// Convert a MeTTa goal to a Prolog goal term. `(head args...)` becomes
/// `head(args...)`; control operators convert their arguments as goals.
pub fn goal_to_term(goal: &Atom, api: &Api, var_map: &mut VarMap) -> Result<term_t, PrologError> {
    let goal = strip_quote_atom(goal);
    if let Atom::Expr(items) = goal {
        if items.len() == 2 && is_sym(&items[0], "Predicate") {
            return atom_to_term(goal, api, var_map);
        }
        if let Some(Atom::Sym(head)) = items.first() {
            if is_control(head.as_str()) {
                let terms: Result<Vec<term_t>, PrologError> = items[1..]
                    .iter()
                    .map(|arg| goal_to_term(arg, api, var_map))
                    .collect();
                return build_compound(api, head.as_str(), &terms?);
            }
            let args: Vec<&Atom> = items[1..].iter().collect();
            return compound(api, head.as_str(), &args, var_map);
        }
    }
    atom_to_term(goal, api, var_map)
}

fn is_control(s: &str) -> bool {
    matches!(s, ":-" | "?-" | "," | ";" | "->" | "*->" | "\\+" | "not")
}

/// Returns true if symbol `s` is a standard Prolog operator/functor.
pub fn is_prolog_operator(s: &str) -> bool {
    is_control(s)
        || matches!(
            s,
            "+" | "-" | "*" | "/" | "mod" | "rem" | "abs" | "min" | "max" | "sqrt" | "**" | "^"
                | "div" | "rdiv" | "=" | "\\=" | "<" | ">" | "=<" | ">=" | "=:=" | "=\\="
        )
}

/// Extract a `(Predicate (f args...))` compound from an expression.
fn extract_predicate_compound<'a>(items: &'a [Atom]) -> Option<(&'a str, Vec<&'a Atom>)> {
    if items.len() != 2 || !is_sym(&items[0], "Predicate") {
        return None;
    }
    match &items[1] {
        Atom::Expr(inner) => match inner.first() {
            Some(Atom::Sym(head)) => Some((head.as_str(), inner[1..].iter().collect())),
            _ => None,
        },
        Atom::Sym(head) => Some((head.as_str(), Vec::new())),
        _ => None,
    }
}

/// Build a compound `head(arg_terms...)`, or the bare atom when arity is 0.
fn build_compound(api: &Api, head: &str, arg_terms: &[term_t]) -> Result<term_t, PrologError> {
    let c_head = CString::new(head)?;
    let head_atom = unsafe { api.new_atom(c_head.as_ptr()) };
    let term = unsafe { api.new_term_ref() };
    if arg_terms.is_empty() {
        unsafe { api.put_atom(term, head_atom) };
        return Ok(term);
    }
    let a0 = unsafe { api.new_term_refs(arg_terms.len() as c_int) };
    for (i, &t) in arg_terms.iter().enumerate() {
        unsafe { api.put_term(a0 + i, t) };
    }
    if unsafe { api.cons_functor_v(term, api.new_functor_sz(head_atom, arg_terms.len()), a0) } == 0 {
        return Err(PrologError::Conversion("failed to build compound term".into()));
    }
    Ok(term)
}

fn compound(api: &Api, head: &str, args: &[&Atom], var_map: &mut VarMap) -> Result<term_t, PrologError> {
    let terms: Result<Vec<term_t>, PrologError> =
        args.iter().map(|a| atom_to_term(a, api, var_map)).collect();
    build_compound(api, head, &terms?)
}

/// Convert a list expression to a Prolog list.
fn list_to_term(api: &Api, items: &[Atom], var_map: &mut VarMap) -> Result<term_t, PrologError> {
    let list_term = unsafe { api.new_term_ref() };
    unsafe { api.put_nil(list_term) };
    let mut current = list_term;
    for item in items.iter().rev() {
        let head_term = atom_to_term(item, api, var_map)?;
        let new_cell = unsafe { api.new_term_ref() };
        if unsafe { api.cons_list(new_cell, head_term, current) } == 0 {
            return Err(PrologError::Conversion("failed to build list".into()));
        }
        current = new_cell;
    }
    Ok(current)
}

/// Convert a Prolog `term_t` to a MeTTa `Atom`.
pub fn term_to_atom(
    term: term_t,
    api: &Api,
    reverse_vars: &mut HashMap<usize, String>,
) -> Result<Atom, PrologError> {
    match unsafe { api.term_type(term) } {
        PL_VARIABLE => {
            if let Some(name) = reverse_vars.get(&term) {
                return Ok(Atom::sym(name));
            }
            let name = format!("$_V{term}");
            reverse_vars.insert(term, name.clone());
            Ok(Atom::sym(&name))
        }
        PL_ATOM => {
            let s = get_str(api, term, CVT_ATOM)?;
            Ok(if s == "[]" { Atom::expr(vec![]) } else { Atom::sym(&s) })
        }
        PL_STRING => Ok(Atom::str_val(&get_str(api, term, CVT_STRING)?)),
        PL_INTEGER => {
            let mut val: i64 = 0;
            if unsafe { api.get_int64(term, &mut val) } != 0 {
                return Ok(Atom::num(val as i128));
            }
            if let Ok(s) = get_str(api, term, CVT_INTEGER) {
                if let Ok(ibig) = dashu::integer::IBig::from_str_radix(&s, 10) {
                    return Ok(Atom::Num(Numeric::Int(ibig)));
                }
            }
            Err(PrologError::Conversion("failed to read Prolog integer".into()))
        }
        PL_FLOAT => {
            let mut val: f64 = 0.0;
            if unsafe { api.get_float(term, &mut val) } == 0 {
                return Err(PrologError::Conversion("failed to get float".into()));
            }
            Atom::decimal(&format!("{val}")).map_err(PrologError::Conversion)
        }
        PL_NIL => Ok(Atom::expr(vec![])),
        PL_LIST_PAIR => {
            let mut elements = Vec::new();
            let head = unsafe { api.new_term_ref() };
            let tail = unsafe { api.new_term_ref() };
            let mut current = term;
            while unsafe { api.term_type(current) } == PL_LIST_PAIR {
                if unsafe { api.get_list(current, head, tail) } == 0 {
                    break;
                }
                elements.push(term_to_atom(head, api, reverse_vars)?);
                current = tail;
            }
            if unsafe { api.term_type(current) } != PL_NIL {
                elements.push(term_to_atom(current, api, reverse_vars)?);
            }
            Ok(Atom::expr(elements))
        }
        PL_TERM => {
            let mut name_atom: atom_t = 0;
            let mut arity: c_int = 0;
            if unsafe { api.get_name_arity(term, &mut name_atom, &mut arity) } == 0 {
                return Err(PrologError::Conversion("failed to get functor name and arity".into()));
            }
            let name_ptr = unsafe { api.atom_chars(name_atom) };
            if name_ptr.is_null() {
                return Err(PrologError::Conversion("failed to get functor name chars".into()));
            }
            let name = conv(unsafe { CStr::from_ptr(name_ptr) }.to_str())?;
            let mut args = Vec::with_capacity(arity as usize + 1);
            args.push(Atom::sym(name));
            for i in 1..=arity {
                let arg = unsafe { api.new_term_ref() };
                if unsafe { api.get_arg(i, term, arg) } == 0 {
                    return Err(PrologError::Conversion(format!("failed to get arg {i}")));
                }
                args.push(term_to_atom(arg, api, reverse_vars)?);
            }
            Ok(Atom::expr(vec![Atom::sym("Predicate"), Atom::expr(args)]))
        }
        _ => {
            let record = unsafe { api.record(term) };
            if record == 0 {
                return Err(PrologError::Conversion("failed to record foreign term".into()));
            }
            Ok(Atom::Gnd(Arc::new(PrologForeign::new(record))))
        }
    }
}
