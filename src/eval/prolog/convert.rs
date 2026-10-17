//! Conversion between MeTTa atoms and Prolog terms.

use std::collections::HashMap;
use std::ffi::CString;

use crate::atom::{Atom, Numeric};
use crate::eval::prolog::error::PrologError;
use crate::eval::prolog::ffi::{Api, term_t};
use crate::eval::prolog::handle::PrologForeign;

/// Map for tracking variable identity during conversion.
/// Maps variable names (e.g., "$x") to Prolog term references.
pub type VarMap = HashMap<String, term_t>;

/// Strip outer `(quote ...)` from an atom.
pub fn strip_quote_atom(atom: &Atom) -> &Atom {
    if let Atom::Expr(expr_data) = atom {
        if let Some(Atom::Sym(s)) = expr_data.first() {
            if s.as_str() == "quote" && expr_data.len() == 2 {
                return &expr_data[1];
            }
        }
    }
    atom
}

/// Convert a MeTTa `Atom` to a Prolog `term_t`.
pub fn atom_to_term(
    atom: &Atom,
    api: &Api,
    var_map: &mut VarMap,
) -> Result<term_t, PrologError> {
    let atom = strip_quote_atom(atom);
    let term = unsafe { api.new_term_ref() };

    match atom {
        Atom::Sym(s) => {
            let name = s.as_str();
            if name.starts_with('$') {
                if let Some(&existing) = var_map.get(name) {
                    return Ok(existing);
                } else {
                    unsafe { api.put_variable(term) };
                    var_map.insert(name.to_string(), term);
                    return Ok(term);
                }
            } else {
                let c_name = CString::new(name).map_err(|e| PrologError::Conversion(e.to_string()))?;
                let atom_handle = unsafe { api.new_atom(c_name.as_ptr()) };
                unsafe { api.put_atom(term, atom_handle) };
            }
        }
        Atom::Str(s) => {
            let c_s = CString::new(s.as_ref()).map_err(|e| PrologError::Conversion(e.to_string()))?;
            unsafe { api.put_string_chars(term, c_s.as_ptr()) };
        }
        Atom::Num(n) => match n {
            Numeric::Int(i) => {
                if let Ok(v) = i64::try_from(i) {
                    unsafe { api.put_int64(term, v) };
                } else {
                    let c_s = CString::new(i.to_string()).map_err(|e| PrologError::Conversion(e.to_string()))?;
                    let rc = unsafe { api.chars_to_term(c_s.as_ptr(), term) };
                    if rc == 0 {
                        return Err(PrologError::Conversion("failed to parse big int into Prolog".into()));
                    }
                }
            }
            Numeric::Dec(d) => {
                let approx = d.to_f64();
                let f = approx.value();
                unsafe { api.put_float(term, f) };
            }
        },
        Atom::Expr(expr_data) => {
            if let Some((head, args)) = extract_predicate_compound(expr_data.as_ref()) {
                return compound_to_term(api, head, args, var_map);
            }
            if expr_data.len() == 2 && expr_data[0].as_sym().map(|s| s == "library").unwrap_or(false) {
                return compound_to_term(api, "library", vec![&expr_data[1]], var_map);
            }
            if let Some(Atom::Sym(head)) = expr_data.first() {
                if is_prolog_operator(head.as_str()) {
                    let args: Vec<&Atom> = expr_data[1..].iter().collect();
                    return compound_to_term(api, head.as_str(), args, var_map);
                }
            }
            return list_to_term(api, expr_data.as_ref(), var_map);
        }
        Atom::Gnd(g) => {
            if let Some(pf) = g.as_any().downcast_ref::<PrologForeign>() {
                let rc = unsafe { api.recorded(pf.record, term) };
                if rc == 0 {
                    return Err(PrologError::Conversion("failed to replay recorded term".into()));
                }
            } else {
                return Err(PrologError::Conversion(
                    "unsupported grounded type for Prolog conversion".into(),
                ));
            }
        }
        _ => {
            return Err(PrologError::Conversion("unsupported atom type for Prolog conversion".into()));
        }
    }

    Ok(term)
}

/// Convert a MeTTa goal to a Prolog goal term.
/// If `goal` is `(head args...)`, it is treated as a compound term `head(args...)`.
pub fn goal_to_term(
    goal: &Atom,
    api: &Api,
    var_map: &mut VarMap,
) -> Result<term_t, PrologError> {
    let goal = strip_quote_atom(goal);
    if let Atom::Expr(items) = goal {
        if items.len() == 2 && items[0].as_sym().map(|s| s == "Predicate").unwrap_or(false) {
            return atom_to_term(goal, api, var_map);
        }
        if let Some(Atom::Sym(head)) = items.first() {
            let head_str = head.as_str();
            // If it's a clause operator (:- , ; -> etc.), its arguments are themselves goals/clauses!
            if matches!(head_str, ":-" | "?-" | "," | ";" | "->" | "*->" | "\\+" | "not") {
                let args_terms: Result<Vec<term_t>, PrologError> = items[1..]
                    .iter()
                    .map(|arg| goal_to_term(arg, api, var_map))
                    .collect();
                let args_terms = args_terms?;
                return compound_from_terms(api, head_str, &args_terms);
            }
            let args: Vec<&Atom> = items[1..].iter().collect();
            return compound_to_term(api, head_str, args, var_map);
        }
    }
    atom_to_term(goal, api, var_map)
}

/// Returns true if symbol `s` is a standard Prolog operator/functor.
pub fn is_prolog_operator(s: &str) -> bool {
    matches!(
        s,
        "+" | "-" | "*" | "/" | "mod" | "rem" | "abs" | "min" | "max"
            | "sqrt" | "**" | "^" | "div" | "rdiv"
            | "=" | "\\=" | "<" | ">" | "=<" | ">=" | "=:=" | "=\\="
            | ":-" | "?-" | "," | ";" | "->" | "*->" | "\\+" | "not"
    )
}

/// Extract a `(Predicate (f args...))` compound from an expression.
fn extract_predicate_compound<'a>(items: &'a [Atom]) -> Option<(&'a str, Vec<&'a Atom>)> {
    if items.len() == 2 {
        if let Some(Atom::Sym(s)) = items.first() {
            if s.as_str() == "Predicate" {
                match &items[1] {
                    Atom::Expr(inner) => {
                        if let Some(Atom::Sym(head)) = inner.first() {
                            return Some((head.as_str(), inner[1..].iter().collect()));
                        }
                    }
                    Atom::Sym(head) => {
                        return Some((head.as_str(), Vec::new()));
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

/// Convert a compound `head(args...)` to a Prolog term from existing term handles.
fn compound_from_terms(
    api: &Api,
    head: &str,
    arg_terms: &[term_t],
) -> Result<term_t, PrologError> {
    let c_head = CString::new(head).map_err(|e| PrologError::Conversion(e.to_string()))?;
    let head_atom = unsafe { api.new_atom(c_head.as_ptr()) };
    let term = unsafe { api.new_term_ref() };

    if arg_terms.is_empty() {
        unsafe { api.put_atom(term, head_atom) };
        return Ok(term);
    }

    let arity = arg_terms.len();
    let functor = unsafe { api.new_functor_sz(head_atom, arity) };

    let a0 = unsafe { api.new_term_refs(arity as std::ffi::c_int) };
    for (i, &arg_term) in arg_terms.iter().enumerate() {
        unsafe { api.put_term(a0 + i, arg_term) };
    }

    let rc = unsafe { api.cons_functor_v(term, functor, a0) };
    if rc == 0 {
        return Err(PrologError::Conversion("failed to build compound term".into()));
    }

    Ok(term)
}

/// Convert a compound `head(args...)` to a Prolog term.
fn compound_to_term(
    api: &Api,
    head: &str,
    args: Vec<&Atom>,
    var_map: &mut VarMap,
) -> Result<term_t, PrologError> {
    let c_head = CString::new(head).map_err(|e| PrologError::Conversion(e.to_string()))?;
    let head_atom = unsafe { api.new_atom(c_head.as_ptr()) };
    let term = unsafe { api.new_term_ref() };

    if args.is_empty() {
        unsafe { api.put_atom(term, head_atom) };
        return Ok(term);
    }

    let arity = args.len();
    let functor = unsafe { api.new_functor_sz(head_atom, arity) };

    let a0 = unsafe { api.new_term_refs(arity as std::ffi::c_int) };
    for (i, arg) in args.iter().enumerate() {
        let arg_term = atom_to_term(arg, api, var_map)?;
        unsafe { api.put_term(a0 + i, arg_term) };
    }

    let rc = unsafe { api.cons_functor_v(term, functor, a0) };
    if rc == 0 {
        return Err(PrologError::Conversion("failed to build compound term".into()));
    }

    Ok(term)
}

/// Convert a list expression to a Prolog list.
fn list_to_term(
    api: &Api,
    items: &[Atom],
    var_map: &mut VarMap,
) -> Result<term_t, PrologError> {
    let list_term = unsafe { api.new_term_ref() };
    unsafe { api.put_nil(list_term) };

    if items.is_empty() {
        return Ok(list_term);
    }

    let mut current = list_term;
    for item in items.iter().rev() {
        let head_term = atom_to_term(item, api, var_map)?;
        let new_cell = unsafe { api.new_term_ref() };
        let rc = unsafe { api.cons_list(new_cell, head_term, current) };
        if rc == 0 {
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
    let ttype = unsafe { api.term_type(term) };
    match ttype {
        crate::eval::prolog::ffi::PL_VARIABLE => {
            if let Some(name) = reverse_vars.get(&term) {
                Ok(Atom::sym(name))
            } else {
                let name = format!("$_V{term}");
                reverse_vars.insert(term, name.clone());
                Ok(Atom::sym(&name))
            }
        }
        crate::eval::prolog::ffi::PL_ATOM => {
            let mut ptr: *const std::ffi::c_char = std::ptr::null();
            let rc = unsafe { api.get_chars(term, &mut ptr, crate::eval::prolog::ffi::CVT_ATOM) };
            if rc == 0 || ptr.is_null() {
                return Err(PrologError::Conversion("failed to get atom chars".into()));
            }
            let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_str()
                .map_err(|e| PrologError::Conversion(e.to_string()))?;
            if s == "[]" {
                Ok(Atom::expr(vec![]))
            } else {
                Ok(Atom::sym(s))
            }
        }
        crate::eval::prolog::ffi::PL_STRING => {
            let mut ptr: *const std::ffi::c_char = std::ptr::null();
            let rc = unsafe { api.get_chars(term, &mut ptr, crate::eval::prolog::ffi::CVT_STRING) };
            if rc == 0 || ptr.is_null() {
                return Err(PrologError::Conversion("failed to get string chars".into()));
            }
            let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_str()
                .map_err(|e| PrologError::Conversion(e.to_string()))?;
            Ok(Atom::str_val(s))
        }
        crate::eval::prolog::ffi::PL_INTEGER => {
            let mut val: i64 = 0;
            if unsafe { api.get_int64(term, &mut val) } != 0 {
                Ok(Atom::num(val as i128))
            } else {
                let mut ptr: *const std::ffi::c_char = std::ptr::null();
                let rc = unsafe { api.get_chars(term, &mut ptr, crate::eval::prolog::ffi::CVT_INTEGER) };
                if rc != 0 && !ptr.is_null() {
                    let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_str()
                        .map_err(|e| PrologError::Conversion(e.to_string()))?;
                    if let Ok(ibig) = dashu::integer::IBig::from_str_radix(s, 10) {
                        return Ok(Atom::Num(Numeric::Int(ibig)));
                    }
                }
                Err(PrologError::Conversion("failed to read Prolog integer".into()))
            }
        }
        crate::eval::prolog::ffi::PL_FLOAT => {
            let mut val: f64 = 0.0;
            let rc = unsafe { api.get_float(term, &mut val) };
            if rc == 0 {
                return Err(PrologError::Conversion("failed to get float".into()));
            }
            Ok(Atom::decimal(&format!("{val}")).map_err(PrologError::Conversion)?)
        }
        crate::eval::prolog::ffi::PL_NIL => {
            Ok(Atom::expr(vec![]))
        }
        crate::eval::prolog::ffi::PL_LIST_PAIR => {
            let mut elements = Vec::new();
            let head = unsafe { api.new_term_ref() };
            let tail = unsafe { api.new_term_ref() };
            let mut current = term;
            while unsafe { api.term_type(current) } == crate::eval::prolog::ffi::PL_LIST_PAIR {
                if unsafe { api.get_list(current, head, tail) } == 0 {
                    break;
                }
                elements.push(term_to_atom(head, api, reverse_vars)?);
                current = tail;
            }
            if unsafe { api.term_type(current) } != crate::eval::prolog::ffi::PL_NIL {
                // Improper list
                elements.push(term_to_atom(current, api, reverse_vars)?);
            }
            Ok(Atom::expr(elements))
        }
        crate::eval::prolog::ffi::PL_TERM => {
            let mut name_atom: crate::eval::prolog::ffi::atom_t = 0;
            let mut arity: std::ffi::c_int = 0;
            if unsafe { api.get_name_arity(term, &mut name_atom, &mut arity) } == 0 {
                return Err(PrologError::Conversion("failed to get functor name and arity".into()));
            }
            let name_ptr = unsafe { api.atom_chars(name_atom) };
            if name_ptr.is_null() {
                return Err(PrologError::Conversion("failed to get functor name chars".into()));
            }
            let name_str = unsafe { std::ffi::CStr::from_ptr(name_ptr) }.to_str()
                .map_err(|e| PrologError::Conversion(e.to_string()))?;

            let mut args = Vec::with_capacity(arity as usize + 1);
            args.push(Atom::sym(name_str));
            for i in 1..=arity {
                let arg_term = unsafe { api.new_term_ref() };
                if unsafe { api.get_arg(i, term, arg_term) } == 0 {
                    return Err(PrologError::Conversion(format!("failed to get arg {i}")));
                }
                args.push(term_to_atom(arg_term, api, reverse_vars)?);
            }
            Ok(Atom::expr(vec![
                Atom::sym("Predicate"),
                Atom::expr(args),
            ]))
        }
        _ => {
            let record = unsafe { api.record(term) };
            if record == 0 {
                return Err(PrologError::Conversion("failed to record foreign term".into()));
            }
            Ok(Atom::Gnd(std::sync::Arc::new(PrologForeign::new(record))))
        }
    }
}