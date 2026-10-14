//! Integration tests for the SWI-Prolog bridge (`prolog-bridge` feature).
//!
//! Every test begins with a `swipl_available()` guard so the suite passes
//! cleanly on machines without SWI-Prolog installed.

#[cfg(feature = "prolog-bridge")]
mod tests {
    use mork_metta::prolog_available;

    /// Returns `true` when the SWI-Prolog runtime can be located and loaded.
    /// Delegates to the library's own discovery logic.
    pub fn swipl_available() -> bool {
        prolog_available()
    }

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn prolog_library_loads() {
        let _guard = TEST_LOCK.lock().unwrap();
        if !swipl_available() {
            eprintln!("skipping: SWI-Prolog not available");
            return;
        }
    }

    #[test]
    fn prolog_worker_thread_ping() {
        let _guard = TEST_LOCK.lock().unwrap();
        if !swipl_available() {
            eprintln!("skipping: SWI-Prolog not available");
            return;
        }
        use mork_metta::eval::prolog::thread::client;
        let client = client().expect("Prolog client should be available");
        client.ping().expect("Ping should succeed");
    }

    #[test]
    fn prolog_engine_manager() {
        let _guard = TEST_LOCK.lock().unwrap();
        if !swipl_available() {
            eprintln!("skipping: SWI-Prolog not available");
            return;
        }
        use mork_metta::eval::prolog::engine::EngineManager;
        let manager = EngineManager::get().expect("Engine manager should be available");

        let init_checked = manager.checked_out_count();
        let init_idle = manager.idle_count();

        // Check out an engine
        let engine1 = manager.checkout().expect("Should be able to checkout engine");
        assert_eq!(manager.checked_out_count(), init_checked + 1);

        // Return it early
        engine1.return_early();
        assert_eq!(manager.checked_out_count(), init_checked);
        assert!(manager.idle_count() >= init_idle);

        // Check out again - should get the pooled engine
        let engine2 = manager.checkout().expect("Should be able to checkout engine again");
        assert_eq!(manager.checked_out_count(), init_checked + 1);
        engine2.return_early();
        assert_eq!(manager.checked_out_count(), init_checked);

        // Check out multiple engines
        let e1 = manager.checkout().expect("checkout 1");
        let e2 = manager.checkout().expect("checkout 2");
        assert_eq!(manager.checked_out_count(), init_checked + 2);
        drop(e1);
        assert_eq!(manager.checked_out_count(), init_checked + 1);
        drop(e2);
        assert_eq!(manager.checked_out_count(), init_checked);
        assert!(manager.idle_count() >= init_idle);
    }

    #[test]
    fn prolog_foreign_handle() {
        let _guard = TEST_LOCK.lock().unwrap();
        if !swipl_available() {
            eprintln!("skipping: SWI-Prolog not available");
            return;
        }
        use mork_metta::eval::prolog::handle::PrologForeign;
        use mork_metta::atom::Grounded;
        use std::any::Any;
        
        // Create a foreign handle
        let handle = PrologForeign::new(42);
        
        // Test display_metta
        assert!(handle.display_metta().contains("prolog-foreign"));
        
        // Test eq_gnd
        let handle2 = PrologForeign::new(42);
        let handle3 = PrologForeign::new(43);
        assert!(handle.eq_gnd(&handle2));
        assert!(!handle.eq_gnd(&handle3));
        
        // Test hash_gnd
        assert_eq!(handle.hash_gnd(), handle2.hash_gnd());
        assert_ne!(handle.hash_gnd(), handle3.hash_gnd());
        
        // Test as_any
        let any: &dyn Any = handle.as_any();
        assert!(any.downcast_ref::<PrologForeign>().is_some());
        
        // Test PartialEq
        assert_eq!(handle, handle2);
        assert_ne!(handle, handle3);
        
        // Test Hash
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher1 = DefaultHasher::new();
        handle.hash(&mut hasher1);
        let mut hasher2 = DefaultHasher::new();
        handle2.hash(&mut hasher2);
        assert_eq!(hasher1.finish(), hasher2.finish());
    }

    #[test]
    fn prolog_atom_to_term() {
        let _guard = TEST_LOCK.lock().unwrap();
        if !swipl_available() {
            eprintln!("skipping: SWI-Prolog not available");
            return;
        }
        use mork_metta::eval::prolog::convert::{atom_to_term, VarMap};
        use mork_metta::atom::Atom;
        use mork_metta::eval::prolog::ffi::Api;
        use mork_metta::eval::prolog::engine::EngineManager;
        use std::collections::HashMap;
        
        let api = Api::get().expect("API should be available");
        let manager = EngineManager::get().expect("EngineManager should be available");
        let pooled = manager.checkout().expect("checkout");
        let mut old_engine = 0;
        unsafe { (api.set_engine)(pooled.engine(), &mut old_engine); }
        let frame = unsafe { (api.open_foreign_frame)() };

        let mut var_map: VarMap = HashMap::new();
        
        // Test symbol
        let sym = Atom::sym("hello");
        let term = atom_to_term(&sym, &api, &mut var_map).expect("symbol conversion failed");
        assert!(unsafe { (api.term_type)(term) } == 2); // PL_ATOM
        
        // Test boolean
        let true_atom = Atom::sym("true");
        let term = atom_to_term(&true_atom, &api, &mut var_map).expect("true conversion failed");
        assert!(unsafe { (api.term_type)(term) } == 2);
        
        // Test variable
        let var = Atom::sym("$x");
        let term = atom_to_term(&var, &api, &mut var_map).expect("variable conversion failed");
        assert!(unsafe { (api.term_type)(term) } == 1); // PL_VARIABLE
        
        // Test same variable gets same term
        let term2 = atom_to_term(&var, &api, &mut var_map).expect("variable conversion failed");
        assert_eq!(term, term2);
        
        // Test integer
        let int = Atom::num(42);
        let term = atom_to_term(&int, &api, &mut var_map).expect("integer conversion failed");
        assert!(unsafe { (api.term_type)(term) } == 3); // PL_INTEGER
        
        // Test string
        let str_atom = Atom::str_val("hello world");
        let term = atom_to_term(&str_atom, &api, &mut var_map).expect("string conversion failed");
        assert!(unsafe { (api.term_type)(term) } == 6); // PL_STRING
        
        // Test list
        let list = Atom::expr(vec![
            Atom::num(1),
            Atom::num(2),
            Atom::num(3),
        ]);
        let term = atom_to_term(&list, &api, &mut var_map).expect("list conversion failed");
        let t = unsafe { (api.term_type)(term) };
        assert!(t == 7 || t == 8 || t == 10); // PL_TERM, PL_NIL or PL_LIST_PAIR

        unsafe {
            (api.discard_foreign_frame)(frame);
            (api.set_engine)(old_engine, std::ptr::null_mut());
        }
    }

    #[test]
    fn prolog_term_to_atom() {
        let _guard = TEST_LOCK.lock().unwrap();
        if !swipl_available() {
            eprintln!("skipping: SWI-Prolog not available");
            return;
        }
        use mork_metta::eval::prolog::convert::{atom_to_term, term_to_atom, VarMap};
        use mork_metta::atom::Atom;
        use mork_metta::eval::prolog::ffi::Api;
        use mork_metta::eval::prolog::engine::EngineManager;
        use std::collections::HashMap;

        let api = Api::get().expect("API should be available");
        let manager = EngineManager::get().expect("EngineManager should be available");
        let pooled = manager.checkout().expect("checkout");
        let mut old_engine = 0;
        unsafe { (api.set_engine)(pooled.engine(), &mut old_engine); }
        let frame = unsafe { (api.open_foreign_frame)() };

        let mut var_map: VarMap = HashMap::new();
        let mut rev_map: HashMap<usize, String> = HashMap::new();

        // Roundtrip symbol
        let sym = Atom::sym("hello");
        let term = atom_to_term(&sym, &api, &mut var_map).unwrap();
        let back = term_to_atom(term, &api, &mut rev_map).unwrap();
        assert_eq!(back, sym);

        // Roundtrip integer
        let int = Atom::num(42);
        let term = atom_to_term(&int, &api, &mut var_map).unwrap();
        let back = term_to_atom(term, &api, &mut rev_map).unwrap();
        assert_eq!(back, int);

        // Roundtrip string
        let s = Atom::str_val("world");
        let term = atom_to_term(&s, &api, &mut var_map).unwrap();
        let back = term_to_atom(term, &api, &mut rev_map).unwrap();
        assert_eq!(back, s);

        // Roundtrip list
        let list = Atom::expr(vec![Atom::num(1), Atom::num(2), Atom::num(3)]);
        let term = atom_to_term(&list, &api, &mut var_map).unwrap();
        let back = term_to_atom(term, &api, &mut rev_map).unwrap();
        assert_eq!(back, list);

        // Roundtrip compound
        let compound = Atom::expr(vec![
            Atom::sym("Predicate"),
            Atom::expr(vec![Atom::sym("foo"), Atom::num(1), Atom::num(2)]),
        ]);
        let term = atom_to_term(&compound, &api, &mut var_map).unwrap();
        let back = term_to_atom(term, &api, &mut rev_map).unwrap();
        assert_eq!(back, compound);

        unsafe {
            (api.discard_foreign_frame)(frame);
            (api.set_engine)(old_engine, std::ptr::null_mut());
        }
    }

    #[test]
    fn prolog_call_predicate() {
        let _guard = TEST_LOCK.lock().unwrap();
        if !swipl_available() {
            eprintln!("skipping: SWI-Prolog not available");
            return;
        }
        use mork_metta::Runtime;
        let mut rt = Runtime::new();

        let res = rt.eval_str("!(callPredicate (Predicate (true)))").unwrap();
        assert_eq!(res.map(|a| a.to_sexpr_string()), Some("true".to_string()));

        let res = rt.eval_str("!(callPredicate (Predicate (fail)))").unwrap();
        assert_eq!(res, None);

        let res = rt.eval_str("!(if (succeedsPredicate (Predicate (true))) Yes No)").unwrap();
        assert_eq!(res.map(|a| a.to_sexpr_string()), Some("Yes".to_string()));

        let res = rt.eval_str("!(if (succeedsPredicate (Predicate (fail))) Yes No)").unwrap();
        assert_eq!(res.map(|a| a.to_sexpr_string()), Some("No".to_string()));
    }

    #[test]
    fn prolog_assert_retract() {
        let _guard = TEST_LOCK.lock().unwrap();
        if !swipl_available() {
            eprintln!("skipping: SWI-Prolog not available");
            return;
        }
        use mork_metta::Runtime;
        let mut rt = Runtime::new();

        rt.eval_str("!(assertzPredicate (Predicate (foo_test 123)))").unwrap();
        let res = rt.eval_str("!(callPredicate (Predicate (foo_test 123)))").unwrap();
        assert_eq!(res.map(|a| a.to_sexpr_string()), Some("true".to_string()));

        let res = rt.eval_str("!(callPredicate (Predicate (foo_test 999)))").unwrap();
        assert_eq!(res, None);

        let res = rt.eval_str("!(retractPredicate (Predicate (foo_test 123)))").unwrap();
        assert_eq!(res.map(|a| a.to_sexpr_string()), Some("true".to_string()));

        let res = rt.eval_str("!(callPredicate (Predicate (foo_test 123)))").unwrap();
        assert_eq!(res, None);
    }

    #[test]
    fn prolog_import_function() {
        let _guard = TEST_LOCK.lock().unwrap();
        if !swipl_available() {
            eprintln!("skipping: SWI-Prolog not available");
            return;
        }
        use mork_metta::Runtime;
        let mut rt = Runtime::new();

        rt.eval_str("!(import_prolog_function +)").unwrap();
        let res = rt.eval_str("!(+ 2 3)").unwrap();
        assert_eq!(res.map(|a| a.to_sexpr_string()), Some("5".to_string()));
    }
}