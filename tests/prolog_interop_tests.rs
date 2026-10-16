//! Comprehensive interop, edge-case, and boundary tests for the SWI-Prolog bridge.

#[cfg(feature = "prolog-bridge")]
mod interop_tests {
    use std::collections::HashMap;
    use mork_metta::atom::{Atom, Numeric};
    use mork_metta::eval::prolog::convert::{atom_to_term, term_to_atom, VarMap};
    use mork_metta::eval::prolog::engine::EngineManager;
    use mork_metta::eval::prolog::ffi::Api;
    use mork_metta::prolog_available;
    use mork_metta::Runtime;

    static INTEROP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn ensure_prolog() -> bool {
        if !prolog_available() {
            eprintln!("Skipping: SWI-Prolog is not available.");
            false
        } else {
            true
        }
    }

    // -------------------------------------------------------------
    // Test 1: Number boundary conditions (i64 max, min, bignum, floats)
    // -------------------------------------------------------------
    #[test]
    fn interop_numeric_conversions() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let api = Api::get().unwrap();
        let manager = EngineManager::get().unwrap();
        let pooled = manager.checkout().unwrap();
        let mut old = 0;
        unsafe { (api.set_engine)(pooled.engine(), &mut old); }
        let frame = unsafe { (api.open_foreign_frame)() };

        // Test i64::MAX
        let max_i64 = Atom::num(i64::MAX as i128);
        let mut var_map = VarMap::new();
        let term = atom_to_term(&max_i64, api, &mut var_map).unwrap();
        let mut rev_map = HashMap::new();
        let back = term_to_atom(term, api, &mut rev_map).unwrap();
        assert_eq!(back, max_i64, "i64::MAX roundtrip failed");

        // Test i64::MIN
        let min_i64 = Atom::num(i64::MIN as i128);
        var_map.clear();
        let term = atom_to_term(&min_i64, api, &mut var_map).unwrap();
        rev_map.clear();
        let back = term_to_atom(term, api, &mut rev_map).unwrap();
        assert_eq!(back, min_i64, "i64::MIN roundtrip failed");

        // Test Bignum (> i64::MAX)
        let bignum = Atom::num(i128::MAX);
        var_map.clear();
        let term = atom_to_term(&bignum, api, &mut var_map).unwrap();
        rev_map.clear();
        let back = term_to_atom(term, api, &mut rev_map).unwrap();
        assert_eq!(back, bignum, "i128::MAX bignum roundtrip failed");

        // Test floats / decimals
        let dec = Atom::decimal("3.1415926535").unwrap();
        var_map.clear();
        let term = atom_to_term(&dec, api, &mut var_map).unwrap();
        rev_map.clear();
        let back = term_to_atom(term, api, &mut rev_map).unwrap();
        if let (Atom::Num(Numeric::Dec(d1)), Atom::Num(Numeric::Dec(d2))) = (&dec, &back) {
            let diff = (d1.to_f64().value() - d2.to_f64().value()).abs();
            assert!(diff < 1e-6, "float roundtrip error too large: {diff}");
        } else {
            panic!("expected decimal, got {:?}", back);
        }

        unsafe {
            (api.discard_foreign_frame)(frame);
            (api.set_engine)(old, std::ptr::null_mut());
        }
    }

    // -------------------------------------------------------------
    // Test 2: Special strings and unicode in conversions
    // -------------------------------------------------------------
    #[test]
    fn interop_string_and_symbol_unicode() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let api = Api::get().unwrap();
        let manager = EngineManager::get().unwrap();
        let pooled = manager.checkout().unwrap();
        let mut old = 0;
        unsafe { (api.set_engine)(pooled.engine(), &mut old); }
        let frame = unsafe { (api.open_foreign_frame)() };

        // Test unicode string with spaces and quotes
        let test_str = Atom::str_val("Hello, 世界! 🚀 \"quotes\" and \n newlines");
        let mut var_map = VarMap::new();
        let term = atom_to_term(&test_str, api, &mut var_map).unwrap();
        let mut rev_map = HashMap::new();
        let back = term_to_atom(term, api, &mut rev_map).unwrap();
        assert_eq!(back, test_str, "Unicode string roundtrip failed");

        // Test empty string
        let empty_str = Atom::str_val("");
        var_map.clear();
        let term = atom_to_term(&empty_str, api, &mut var_map).unwrap();
        rev_map.clear();
        let back = term_to_atom(term, api, &mut rev_map).unwrap();
        assert_eq!(back, empty_str, "Empty string roundtrip failed");

        // Test symbols with dashes, colons, underscores
        let complex_sym = Atom::sym("foo-bar:baz_123");
        var_map.clear();
        let term = atom_to_term(&complex_sym, api, &mut var_map).unwrap();
        rev_map.clear();
        let back = term_to_atom(term, api, &mut rev_map).unwrap();
        assert_eq!(back, complex_sym, "Complex symbol roundtrip failed");

        unsafe {
            (api.discard_foreign_frame)(frame);
            (api.set_engine)(old, std::ptr::null_mut());
        }
    }

    // -------------------------------------------------------------
    // Test 3: Empty lists, nested empty lists, improper lists
    // -------------------------------------------------------------
    #[test]
    fn interop_list_structures() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let api = Api::get().unwrap();
        let manager = EngineManager::get().unwrap();
        let pooled = manager.checkout().unwrap();
        let mut old = 0;
        unsafe { (api.set_engine)(pooled.engine(), &mut old); }
        let frame = unsafe { (api.open_foreign_frame)() };

        // Empty list ()
        let empty_list = Atom::expr(vec![]);
        let mut var_map = VarMap::new();
        let term = atom_to_term(&empty_list, api, &mut var_map).unwrap();
        let mut rev_map = HashMap::new();
        let back = term_to_atom(term, api, &mut rev_map).unwrap();
        assert_eq!(back, empty_list, "Empty list roundtrip failed");

        // Nested lists: ((1 2) () (3 (4 5)))
        let nested = Atom::expr(vec![
            Atom::expr(vec![Atom::num(1), Atom::num(2)]),
            Atom::expr(vec![]),
            Atom::expr(vec![Atom::num(3), Atom::expr(vec![Atom::num(4), Atom::num(5)])]),
        ]);
        var_map.clear();
        let term = atom_to_term(&nested, api, &mut var_map).unwrap();
        rev_map.clear();
        let back = term_to_atom(term, api, &mut rev_map).unwrap();
        assert_eq!(back, nested, "Nested list roundtrip failed");

        unsafe {
            (api.discard_foreign_frame)(frame);
            (api.set_engine)(old, std::ptr::null_mut());
        }
    }

    // -------------------------------------------------------------
    // Test 4: translatePredicate multi-solution non-determinism with let vs progn
    // -------------------------------------------------------------
    #[test]
    fn interop_translate_predicate_multi_solutions() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let mut rt = Runtime::new();

        // 4a. With let: let should iterate all solutions
        let code_let = r#"
        !(collapse (let $ok (translatePredicate (member $x (10 20 30))) $x))
        "#;
        let res_let = rt.eval_str(code_let).unwrap();
        println!("let member solutions: {:?}", res_let);

        // 4b. With progn: progn currently takes only first solution
        let code_progn = r#"
        !(collapse (progn (translatePredicate (member $x (10 20 30))) $x))
        "#;
        let res_progn = rt.eval_str(code_progn).unwrap();
        println!("progn member solutions: {:?}", res_progn);

        // Both let and progn behaviors observed
        assert!(res_let.is_some());
    }

    // -------------------------------------------------------------
    // Test 5: translatePredicate failure handling (case with Empty)
    // -------------------------------------------------------------
    #[test]
    fn interop_translate_predicate_failure() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let mut rt = Runtime::new();

        // When goal fails, translatePredicate returns empty set.
        // In MeTTa, `case` catches `Empty`:
        let code = r#"
        !(case (translatePredicate (= 1 2))
               (($item Success)
                (Empty Failure)))
        "#;
        let res = rt.eval_str(code).unwrap();
        println!("case on failure: {:?}", res);
        assert_eq!(
            res.map(|a| a.to_sexpr_string()),
            Some("Failure".to_string()),
            "Failed goal should be handled by Empty in case"
        );
    }

    // -------------------------------------------------------------
    // Test 6: translatePredicate with bound input variables
    // -------------------------------------------------------------
    #[test]
    fn interop_translate_predicate_input_bindings() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let mut rt = Runtime::new();

        // Bound variable matching
        let code = r#"
        !(let $x 5 (progn (translatePredicate (= $x 5)) Success))
        "#;
        let res = rt.eval_str(code).unwrap();
        assert_eq!(res.map(|a| a.to_sexpr_string()), Some("Success".to_string()));

        // Bound variable mismatching (returns Empty, handled via case)
        let code_mismatch = r#"
        !(let $x 5 (case (translatePredicate (= $x 6))
                         (($item Success)
                          (Empty Failure))))
        "#;
        let res = rt.eval_str(code_mismatch).unwrap();
        assert_eq!(res.map(|a| a.to_sexpr_string()), Some("Failure".to_string()));
    }

    // -------------------------------------------------------------
    // Test 7: Exception propagation (throw, undefined predicate)
    // -------------------------------------------------------------
    #[test]
    fn interop_prolog_exceptions() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let mut rt = Runtime::new();

        // Explicit throw in Prolog
        let code = r#"
        !(translatePredicate (throw "custom_prolog_error"))
        "#;
        let res = rt.eval_str(code);
        println!("throw result: {:?}", res);
        assert!(res.is_err(), "throw in Prolog should produce an Err result in MeTTa");

        // Undefined predicate call in Prolog
        let code_undef = r#"
        !(translatePredicate (totally_non_existent_predicate_12345 $x))
        "#;
        let res_undef = rt.eval_str(code_undef);
        println!("Undefined predicate result: {:?}", res_undef);
        assert!(res_undef.is_err(), "Calling undefined predicate should produce an Err result");
    }

    // -------------------------------------------------------------
    // Test 8: Multiple concurrent engine checkouts / pool exhaustion
    // -------------------------------------------------------------
    #[test]
    fn interop_engine_pool_limit_and_recycling() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let manager = EngineManager::get().unwrap();

        let initial_checked_out = manager.checked_out_count();

        // Check out 12 engines (MAX_IDLE_ENGINES is 8)
        let mut engines = Vec::new();
        for i in 0..12 {
            let engine = manager.checkout().expect(&format!("Failed to checkout engine {i}"));
            engines.push(engine);
        }
        assert_eq!(manager.checked_out_count(), initial_checked_out + 12);

        // Drop all 12 engines
        drop(engines);

        // All should be returned; idle pool capped at MAX_IDLE_ENGINES (8)
        assert_eq!(manager.checked_out_count(), initial_checked_out);
        assert_eq!(manager.idle_count(), 8, "Idle pool should cap at 8");
    }

    // -------------------------------------------------------------
    // Test 9: Streaming query with early drop (no resource leak)
    // -------------------------------------------------------------
    #[test]
    fn interop_stream_early_drop() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let manager = EngineManager::get().unwrap();

        let temp_dir = std::env::temp_dir();
        let pl_path = temp_dir.join("stream_drop.pl");
        std::fs::write(&pl_path, "my_range(X) :- between(1, 1000, X).\n").unwrap();

        let client_ref = mork_metta::eval::prolog::thread::client().unwrap();
        client_ref.request(mork_metta::eval::prolog::thread::PrologCommand::Consult {
            path: pl_path.to_string_lossy().to_string(),
        }).unwrap();

        let mut rt = Runtime::new();
        rt.eval_str("!(import_prolog_function my_range)").unwrap();

        // Take only the first result and discard the rest
        let before_checked_out = manager.checked_out_count();
        let code = r#"
        !(car-atom (collapse (my_range)))
        "#;
        let res = rt.eval_str(code).unwrap();
        println!("car-atom result: {:?}", res);
        assert_eq!(res.map(|a| a.to_sexpr_string()), Some("1".to_string()));

        // The engine from the stream should be returned when the stream is dropped
        let after_checked_out = manager.checked_out_count();
        assert_eq!(after_checked_out, before_checked_out, "Stream engine leaked after partial consumption!");
    }

    // -------------------------------------------------------------
    // Test 10: High-volume query loop (500 queries)
    // -------------------------------------------------------------
    #[test]
    fn interop_loop_500_queries() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let mut rt = Runtime::new();

        for i in 0..500 {
            let code = format!("!(progn (translatePredicate (is $x (+ {i} 1))) $x)");
            let res = rt.eval_str(&code).unwrap();
            assert_eq!(res.map(|a| a.to_sexpr_string()), Some(format!("{}", i + 1)));
        }
    }

    // -------------------------------------------------------------
    // Test 11: Full stream consumption via collapse
    // -------------------------------------------------------------
    #[test]
    fn interop_stream_full_collapse() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let temp_dir = std::env::temp_dir();
        let pl_path = temp_dir.join("small_range.pl");
        std::fs::write(&pl_path, "small_range(X) :- between(1, 5, X).\n").unwrap();

        let client_ref = mork_metta::eval::prolog::thread::client().unwrap();
        client_ref.request(mork_metta::eval::prolog::thread::PrologCommand::Consult {
            path: pl_path.to_string_lossy().to_string(),
        }).unwrap();

        let mut rt = Runtime::new();
        rt.eval_str("!(import_prolog_function small_range)").unwrap();

        let code = r#"
        !(collapse (small_range))
        "#;
        let res = rt.eval_str(code).unwrap();
        println!("collapse result: {:?}", res);
        assert_eq!(res.map(|a| a.to_sexpr_string()), Some("(1 2 3 4 5)".to_string()));
    }

    // -------------------------------------------------------------
    // Test 12: Imported function failure handling
    // -------------------------------------------------------------
    #[test]
    fn interop_imported_function_failure() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let temp_dir = std::env::temp_dir();
        let pl_path = temp_dir.join("always_fails.pl");
        std::fs::write(&pl_path, "always_fails(_) :- fail.\n").unwrap();

        let client_ref = mork_metta::eval::prolog::thread::client().unwrap();
        client_ref.request(mork_metta::eval::prolog::thread::PrologCommand::Consult {
            path: pl_path.to_string_lossy().to_string(),
        }).unwrap();

        let arities_reply = client_ref.request(mork_metta::eval::prolog::thread::PrologCommand::CurrentArities {
            name: "always_fails".to_string(),
        }).unwrap();
        println!("always_fails arities_reply: {:?}", arities_reply);

        let mut rt = Runtime::new();
        rt.eval_str("!(import_prolog_function always_fails)").unwrap();

        let direct_res = rt.eval_str("!(always_fails)");
        println!("direct always_fails: {:?}", direct_res);

        let code = r#"
        !(case (always_fails)
               (($res Success)
                (Empty Failure)))
        "#;
        let res = rt.eval_str(code).unwrap();
        println!("case always_fails: {:?}", res);
        assert_eq!(res.map(|a| a.to_sexpr_string()), Some("Failure".to_string()));
    }

    // -------------------------------------------------------------
    // Test 13: Variable co-reference ($x appears twice in goal)
    // -------------------------------------------------------------
    #[test]
    fn interop_variable_coreference() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let mut rt = Runtime::new();

        // (= $x $x) should trivially succeed
        let code1 = r#"
        !(case (translatePredicate (= $x $x))
               (($res Success)
                (Empty Failure)))
        "#;
        let res1 = rt.eval_str(code1).unwrap();
        assert_eq!(res1.map(|a| a.to_sexpr_string()), Some("Success".to_string()));

        // Check if binding works when variable appears twice in goal
        let code2 = r#"
        !(progn (translatePredicate (= (pair $x $x) (pair 42 42))) $x)
        "#;
        let res2 = rt.eval_str(code2).unwrap();
        assert_eq!(res2.map(|a| a.to_sexpr_string()), Some("42".to_string()));

        // Conflicting binding on same variable should fail
        let code3 = r#"
        !(case (translatePredicate (= (pair $x $x) (pair 10 20)))
               (($res Success)
                (Empty Failure)))
        "#;
        let res3 = rt.eval_str(code3).unwrap();
        assert_eq!(res3.map(|a| a.to_sexpr_string()), Some("Failure".to_string()));
    }

    // -------------------------------------------------------------
    // Test 14: Multithreaded concurrent queries
    // -------------------------------------------------------------
    #[test]
    fn interop_multithreaded_concurrency() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        use std::thread;

        let mut handles = Vec::new();
        for t in 0..8 {
            let handle = thread::spawn(move || {
                let mut rt = Runtime::new();
                for i in 0..50 {
                    let val = t * 100 + i;
                    let code = format!("!(progn (translatePredicate (is $x (+ {val} 1))) $x)");
                    let res = rt.eval_str(&code).unwrap();
                    assert_eq!(res.map(|a| a.to_sexpr_string()), Some(format!("{}", val + 1)));
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().expect("Worker thread panicked!");
        }
    }

    // -------------------------------------------------------------
    // Test 15: Foreign term recording and replay (stream blob)
    // -------------------------------------------------------------
    #[test]
    fn interop_foreign_blob_term() {
        let _guard = INTEROP_LOCK.lock().unwrap();
        if !ensure_prolog() { return; }
        let _client_ref = mork_metta::eval::prolog::thread::client().unwrap();

        // Query open_string to get a stream blob
        let mut rt = Runtime::new();
        let code = r#"
        !(progn (translatePredicate (open_string "test_stream_content" $s)) $s)
        "#;
        let res = rt.eval_str(code).unwrap();
        println!("Stream blob result: {:?}", res);
        assert!(res.is_some(), "Stream blob should be returned");
        let atom = res.unwrap();
        assert!(matches!(atom, Atom::Gnd(_)), "Unrepresentable blob should become Atom::Gnd(PrologForeign)");

        // Pass the foreign atom directly via environment binding to close($s)
        let close_code = r#"
        !(let $s (progn (translatePredicate (open_string "test_stream_content" $s)) $s)
               (translatePredicate (close $s)))
        "#;
        let close_res = rt.eval_str(close_code);
        println!("Close stream result: {:?}", close_res);
        assert!(close_res.is_ok(), "Foreign blob should replay cleanly via PL_recorded");
    }
}
