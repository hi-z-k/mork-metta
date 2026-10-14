(: consult_file (-> Atom Atom))
(= (consult_file $prologfile)
   (callPredicate (Predicate (quote (consult $prologfile)))))

(: use_module_file (-> Atom Atom))
(= (use_module_file $prologfile)
   (callPredicate (Predicate (quote (use_module $prologfile)))))

(= (import_prolog_functions_from_file $prologfile $importfunctions)
   (let* (($ok (consult_file $prologfile))
          ($function (superpose $importfunctions)))
         (import_prolog_function $function)))

(= (import_prolog_functions_from_module $prologfile $importfunctions)
   (let* (($ok (use_module_file $prologfile))
          ($function (superpose $importfunctions)))
         (import_prolog_function $function)))

!(import_prolog_functions_from_file (library lib_import.pl) (static-import! git-import! use-module!))
