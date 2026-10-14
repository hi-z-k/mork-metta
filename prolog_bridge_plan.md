# Prolog Bridge Implementation Guide for mork-metta

This document serves as the complete, verified architectural specification and step-by-step implementation guide for adding the SWI-Prolog bridge (`prolog-bridge` feature) to `mork-metta`.

---

## 1. Codebase Verification Report (Items V1–V14)

Every item from the verification checklist has been inspected directly against the codebase and tested against a live SWI-Prolog environment (SWI-Prolog version 10.1.18).

| ID | Subject | Verified Findings in `mork-metta` & SWI |
|---|---|---|
| **V1** | `Cargo.toml` | `libloading = { version = "0.8", optional = true }` is **already** declared in `[dependencies]`. Edition is `2024`. Feature definition to add: `prolog-bridge = ["dep:libloading"]`. Required build flags: `RUSTC_BOOTSTRAP=1 RUSTFLAGS="-C target-cpu=native"`. |
| **V2** | `src/atom.rs` | `Grounded` trait: `pub trait Grounded: fmt::Debug + Send + Sync { fn display_metta(&self) -> String; fn eq_gnd(&self, other: &dyn Grounded) -> bool; fn hash_gnd(&self) -> u64; fn as_any(&self) -> &dyn Any; }`. Constructors: `Atom::sym(s)` (canonicalizes `"True"`/`"False"` to `"true"`/`"false"`), `Atom::str_val(s)`, `Atom::num(i128)`, `Atom::decimal(s)`, `Atom::expr(items)`, `Atom::Gnd(Arc<dyn Grounded>)`. Accessors: `as_sym()`, `as_str_val()`, `as_num()`, `as_numeric()`, `as_expr()`, `is_truthy()`. |
| **V3** | `src/func.rs` | `insert_native<F>(&self, name: &str, arity: u8, func: F)` registers functions with default `Effect::SpaceMutate`. `NDet::single(Atom)` and `NDet::stream(impl Iterator<Item=Atom> + Send + 'static)`. Cache invalidation: `memo_stamp.fetch_add(1, Ordering::SeqCst)`, `memo_cache.write().unwrap().clear()`, `lazy_mask_cache.write().unwrap().clear()`. |
| **V4** | `src/eval/python.rs` | Python bridge uses dedicated VM opcodes (`Opcode::PyCall`, `Opcode::PyEval`) and compiler branches, rather than `insert_native`. For Prolog: builtins (`Predicate`, `callPredicate`, `assertaPredicate`, `assertzPredicate`, `retractPredicate`, `import_prolog_function`) are registered in `FnTable`. `translatePredicate` requires variable binding sharing with the VM environment and therefore uses dedicated opcode `Opcode::TranslatePredicate`. |
| **V5** | `compiler.rs` & `run.rs` | Python imports compile via `Opcode::PythonImport { path }` and runtime `eval_py_import_library`. Prolog `.pl` import mirrors this via `Opcode::PrologImport { path }` and dynamic import in `Opcode::ImportDynamic` for paths ending in `.pl`. |
| **V6** | VM Environment & Branching | `Env` is an immutable linked list (`Env(Arc<EnvNode>)`) with `get(name)` and `extend(name, value)`. `ResultSet` is `Vec<(Atom, Env)>`. Branching is achieved by returning multiple `(Atom, Env)` tuples. Free variables in expressions are indexed via `state.free_vars_map` and assigned fresh symbols (`$__fresh_<name>_<id>`) in `state.free_vars_bindings`. In `Opcode::Progn`, threading the environment between sequential bodies enables variable sharing across successive `translatePredicate` calls. |
| **V7** | Error Atoms | MeTTa standard error format: `(Error <target/context> <detail/message>)`. Represented as `Atom::expr([Atom::sym("Error"), context.clone(), Atom::sym(msg)])`. Errors in MeTTa are normal data expressions unless inspected by `if-error` or `catch`. |
| **V8** | `lib_zar.metta` & `lib_import.metta` | `lib/lib_zar.metta` uses `(callPredicate (Predicate (quote (consult $prologfile))))`. In mork-metta, `(quote X)` evaluates to `(quote X)`. Therefore, `Predicate` and `atom_to_term` must unwrap a top-level `quote` expression. In `lib/lib_import.metta`, PeTTa's zero-argument `(consult)` inside `let*` relies on PeTTa's macro expansion; for `mork-metta`, `lib_import.metta` should use `(consult_file $prologfile)` or `consult($prologfile)`. |
| **V9** | SWI FFI Verification | Verified via live C test execution: (1) `PL_initialise(3, ["swipl", "-q", "--nosignals"])` succeeds cleanly. Setting `SWI_HOME_DIR` from `swipl --dump-runtime-variables` (`PLBASE`) guarantees clean discovery regardless of location. (2) `PL_Q_EXT_STATUS` return codes: `PL_S_LAST` (2) indicates success with no choicepoint (last solution!), `PL_S_TRUE` (1) indicates success with open choicepoint, `PL_S_FALSE` (0) indicates failure, `PL_S_EXCEPTION` (-1) indicates uncaught exception. Deterministic calls return `PL_S_LAST` immediately. (3) `PL_create_engine(NULL)`, `PL_set_engine()`, and `PL_destroy_engine()` work seamlessly on a single thread. (4) `set_prolog_flag(table_shared, true)` works on main and pooled engines. (5) `PL_get_chars(ex, &s, CVT_WRITE)` formats compound exception terms into strings. |
| **V10** | `run_examples.sh` & Tests | `run_examples.sh` runs `examples/*.metta` and checks for `❌`. Supports `--skip <pattern>`. Rust tests in `tests/` use `Runtime::new()` and `rt.eval_str()`. Tests check `swipl_available()` and skip gracefully if `libswipl` is not installed. |
| **V11** | GitHub Workflows | Create `.github/workflows/ci.yml` that installs `swi-prolog-nox` on `ubuntu-latest`, runs `cargo check` and `cargo test --features prolog-bridge`, and runs `./run_examples.sh`. |
| **V12** | `src/main.rs` | CLI spawns `eval-worker` with 8MB stack and runs `rt.load_file`. The dedicated Prolog thread runs in the background and shuts down cleanly on drop or process termination. |
| **V13** | `dashu` Integers & Floats | `Numeric::Int(IBig)`: values fitting `i64` use `PL_put_int64` / `PL_get_int64`; large integers convert via decimal strings using `PL_put_integer_chars` and `PL_get_chars(..., CVT_INTEGER)`. Floats use `PL_put_float` / `PL_get_float`. |
| **V14** | Code Standards | Conventional commits (`feat(prolog):`, `test(prolog):`, etc.). `cargo fmt` and `cargo clippy`. Target branch: `unstable`. |

---

## 2. Core Architecture & Function Strategy

```
MeTTa Code: (Predicate ...), (callPredicate ...), (import_prolog_function ...), (translatePredicate ...)
   │
   ▼
VM / Compiler Layer (compiler.rs, run.rs, op.rs, builtins.rs)
   │  - Evaluates arguments & substitutes bound variables from Env
   │  - Dispatches commands across crossbeam/std channel to dedicated thread
   ▼
Dedicated Prolog Thread (src/eval/prolog/thread.rs)
   │  - Single thread owns all SWI-Prolog C API calls
   │  - Manages Main Engine + Engine Pool (max 8 idle engines)
   ▼
Query Execution & Streaming (query.rs, stream.rs, ops.rs)
   │  - Opens query with PL_Q_CATCH_EXCEPTION | PL_Q_EXT_STATUS | PL_Q_NODEBUG
   │  - Single solution / last solution -> closes immediately, returns Outcome::One
   │  - Multiple solutions -> returns Outcome::Open with PrologStream (lazy NDet)
   ▼
Term Marshalling (convert.rs, handle.rs)
   - MeTTa Atom <---> Prolog term_t
   - Opaque values (blobs, cyclic terms, dicts) stored as PrologForeign(RecordId)
```

### Module Layout (`src/eval/prolog/`)

- **`ffi.rs`**: Dynamic loading of `libswipl` via `libloading::Library`.
  - Discovers paths: (1) `MORK_METTA_LIBSWIPL` env var, (2) `swipl --dump-runtime-variables` (`PLLIBDIR`/`PLLIBSWIPL`/`PLBASE`), (3) standard shared library names (`libswipl.so.10`, `libswipl.so.9`, `libswipl.so.8`, `libswipl.so`, `libswipl.dylib`).
  - Sets `SWI_HOME_DIR` before initialization.
  - Safe function wrappers for all SWI C API functions.
- **`error.rs`**: `PrologError` enum: `LibraryNotFound`, `SymbolMissing`, `InitFailed`, `Exception(String)`, `ThreadGone`, `Conversion(String)`, `Internal(String)`. Implements `to_error_atom(&self, context: &Atom) -> Atom`.
- **`handle.rs`**: `PrologForeign { record: usize }` implementing `Grounded`. Manages `PL_record`, `PL_recorded`, `PL_erase`. `Drop` notifies Prolog thread to erase the recorded term.
- **`convert.rs`**:
  - `atom_to_term`: Translates symbols, strings, numbers, lists, expressions, `(Predicate ...)` compounds, and variables (`VarMap`). Automatically strips outer `(quote ...)`.
  - `term_to_atom`: Translates atoms to symbols, compounds to `(Predicate (head args...))`, lists to expressions, integers to exact `Numeric::Int`, floats to `Numeric::Dec`, variables to `$_V<n>`, and unrepresentable terms to `PrologForeign`.
- **`engine.rs`**: `EngineManager` managing the main engine (`PL_ENGINE_MAIN`) and pooled engines (`PL_create_engine`). Sets `set_prolog_flag(table_shared, true)` on all engines. Reuses engines up to `max_idle = 8`.
- **`query.rs`**: Query lifecycle: `open_call`, `step` (checking `PL_S_LAST`, `PL_S_TRUE`, `PL_S_FALSE`, `PL_S_EXCEPTION`), `close`.
- **`stream.rs`**: `PrologStream` implementing `Iterator<Item = Atom>` for lazy query iteration into `NDet::Stream`.
- **`thread.rs`**: Spawns `mork-prolog` worker thread. Exposes `PrologClient` with `request` and `notify`. Cached via `static CLIENT: OnceLock<Result<PrologClient, PrologError>>`.
- **`ops.rs`**: Internal command executors running on the Prolog thread: `call_goal`, `next_answer`, `close_stream`, `direct`, `call_function`, `current_arities`, `translate`.
- **`builtins.rs`**: Registration of `Predicate`, `callPredicate`, `assertaPredicate`, `assertzPredicate`, `retractPredicate`, `import_prolog_function`.
- **`translate.rs`**: Support logic for `Opcode::TranslatePredicate`. Gathers variables, marshals arguments with bindings, maps solutions to updated `Env` frames.

---

## 3. TDD Strategy & CI Foundation

To guarantee test-driven development and prevent any regressions, the CI workflow and test harness are introduced at the start.

### CI Configuration (`.github/workflows/ci.yml`)
- Triggers on push and pull requests to `unstable`.
- Runs on `ubuntu-latest`.
- Steps:
  1. Check out repository.
  2. Install Rust toolchain with `RUSTC_BOOTSTRAP=1`.
  3. Install SWI-Prolog: `sudo apt-get update && sudo apt-get install -y swi-prolog-nox`.
  4. Run `cargo check --features prolog-bridge`.
  5. Run `cargo test --features prolog-bridge`.
  6. Run `./run_examples.sh`.

### Test Skipping Helper (`swipl_available()`)
In `tests/prolog_tests.rs`:
```rust
pub fn swipl_available() -> bool {
    #[cfg(feature = "prolog-bridge")]
    {
        crate::eval::prolog::ffi::Api::get().is_ok()
    }
    #[cfg(not(feature = "prolog-bridge"))]
    {
        false
    }
}
```
All tests invoke `if !swipl_available() { return; }` to skip cleanly on machines without SWI installed.

---

## 4. Step-by-Step Commit Plan (24 Commits)

### Commit 1: `ci(prolog): add CI workflow and test harness`
- **Files**: `.github/workflows/ci.yml`, `tests/prolog_bridge_tests.rs`
- **What**: Create GitHub Actions workflow installing `swi-prolog-nox`. Add skeleton `tests/prolog_bridge_tests.rs` with `swipl_available()` check.
- **Proof**: CI workflow file validates; `cargo test` passes.

### Commit 2: `build(prolog): add prolog-bridge feature and empty module`
- **Files**: `Cargo.toml`, `src/eval/mod.rs`, `src/eval/prolog/mod.rs`
- **What**: Declare `prolog-bridge = ["dep:libloading"]` in `Cargo.toml`. Add conditional module `pub mod prolog;` in `src/eval/mod.rs`.
- **Proof**: `RUSTC_BOOTSTRAP=1 RUSTFLAGS="-C target-cpu=native" cargo check` passes with and without `--features prolog-bridge`.

### Commit 3: `docs(prolog): document bridge architecture and design decisions`
- **Files**: `docs/prolog-bridge.md`
- **What**: Write detailed reference documentation for the bridge architecture, threading model, term mapping, and engine pooling.
- **Proof**: Markdown documentation reviewed.

### Commit 4: `feat(prolog): load libswipl at runtime with path discovery`
- **Files**: `src/eval/prolog/ffi.rs`, `src/eval/prolog/error.rs`
- **What**: Implement dynamic loading of `libswipl` via `libloading::Library`. Candidate paths include env `MORK_METTA_LIBSWIPL`, `swipl --dump-runtime-variables` discovery, and shared library names (`libswipl.so.10`, `libswipl.so.9`, `libswipl.so`, `libswipl.dylib`). Set `SWI_HOME_DIR` before initialization.
- **Proof**: Unit test in `tests/prolog_bridge_tests.rs` asserts `Api::get()` loads successfully when SWI is installed and provides informative error when missing.

### Commit 5: `feat(prolog): add dedicated prolog worker thread and command channel`
- **Files**: `src/eval/prolog/thread.rs`
- **What**: Spawn dedicated thread `mork-prolog`. Provide `PrologClient` with channels for sending commands and receiving replies. Startup result is cached in `OnceLock`.
- **Proof**: Round-trip `Ping` command test passes.

### Commit 6: `feat(prolog): manage main and pooled engines with shared tabling`
- **Files**: `src/eval/prolog/engine.rs`
- **What**: Implement `EngineManager`. Support checkout and return of pooled engines (up to 8 idle engines kept alive). Apply `set_prolog_flag(table_shared, true)` on engine initialization.
- **Proof**: Test checking out two engines, verifying independent query contexts and shared tabling flag.

### Commit 7: `feat(prolog): add opaque handle for foreign prolog terms`
- **Files**: `src/eval/prolog/handle.rs`
- **What**: Implement `PrologForeign` holding a recorded term handle (`RecordId`). Implements `Grounded` trait (`display_metta`, `eq_gnd`, `hash_gnd`, `as_any`). `Drop` sends `EraseRecord` to the Prolog thread.
- **Proof**: Test records a term, accesses it, drops it, and verifies `EraseRecord` is received.

### Commit 8: `feat(prolog): convert MeTTa atoms to Prolog terms`
- **Files**: `src/eval/prolog/convert.rs`
- **What**: Implement `atom_to_term`. Handles symbols (`true`, `false`, atoms), strings, 64-bit and big integers (`dashu`), decimals/floats, lists, `(Predicate (f ...))` compounds, and unwraps outer `(quote ...)`.
- **Proof**: Unit tests verifying each atom type converts to expected Prolog term structure.

### Commit 9: `feat(prolog): convert Prolog terms to MeTTa atoms`
- **Files**: `src/eval/prolog/convert.rs`
- **What**: Implement `term_to_atom`. Converts atoms, strings, integers, floats, lists to expressions, compounds to `(Predicate ...)`, variables to `$_V<n>`, and handles to `PrologForeign`.
- **Proof**: Round-trip conversion tests: `atom -> term -> atom` equality for all supported types.

### Commit 10: `feat(prolog): open and step queries with extended status`
- **Files**: `src/eval/prolog/query.rs`, `src/eval/prolog/ops.rs`
- **What**: Implement `open_call`, `step`, and `close`. Open query with `PL_Q_CATCH_EXCEPTION | PL_Q_EXT_STATUS | PL_Q_NODEBUG`. Recognize `PL_S_LAST` to detect when an answer is final and immediately return `Outcome::One`.
- **Proof**: Tests on deterministic goal (`=(42, 42)` -> `One`), nondeterministic goal (`between(1, 2, X)` -> `Open`), failing goal (`=(1, 2)` -> `Failed`), and exception goal (`undefined/1` -> `Exception`).

### Commit 11: `feat(prolog): stream query solutions into lazy NDet`
- **Files**: `src/eval/prolog/stream.rs`
- **What**: Implement `PrologStream` as an `Iterator<Item = Atom>` driving `NDet::Stream`. Dropping stream sends `CloseStream` to release the pool engine immediately.
- **Proof**: Test streaming 5 solutions from `between(1, 10, X)` and dropping early; engine returns to pool without leak.

### Commit 12: `feat(prolog): map Prolog exceptions to MeTTa Error atoms`
- **Files**: `src/eval/prolog/error.rs`
- **What**: Format Prolog exceptions via `PL_get_chars(..., CVT_WRITE)` and convert them to `(Error <context> "<message>")`.
- **Proof**: Calling undefined predicate produces `(Error ...)` atom matching expected diagnostic format.

### Commit 13: `feat(prolog): add Predicate and callPredicate builtins`
- **Files**: `src/eval/prolog/builtins.rs`, `src/builtins/mod.rs`
- **What**: Implement `(Predicate (f args...))` and `(callPredicate G)`. Register in `register_builtins`.
- **Proof**: Integration test running `!(callPredicate (Predicate (true)))` returning `True`.

### Commit 14: `feat(prolog): add assertaPredicate, assertzPredicate, and retractPredicate`
- **Files**: `src/eval/prolog/builtins.rs`
- **What**: Implement `assertaPredicate`, `assertzPredicate`, and `retractPredicate` running on the main engine.
- **Proof**: Integration test asserting `foo(1)`, checking `callPredicate`, retracting `foo(1)`, and verifying subsequent failure.

### Commit 15: `feat(prolog): add import_prolog_function with arity discovery`
- **Files**: `src/eval/prolog/builtins.rs`, `src/func.rs`
- **What**: Implement `(import_prolog_function name)`. Discovers existing predicate arities via `current_predicate(name/A)`, registers native functions in `FnTable` with arity `A - 1` (last argument is return value), and invalidates caches.
- **Proof**: Import `plus/3` and evaluate `!(plus 2 3)` returning `5`.

### Commit 16: `feat(vm): resolve imported Prolog functions at call time`
- **Files**: `src/func.rs`, `src/eval/machine/vm/run.rs`
- **What**: If an imported function name is invoked with an arity not seen at import time, query Prolog at call time and dynamically register the arity.
- **Proof**: Defining predicate after import call succeeds on invocation.

### Commit 17: `feat(vm): support .pl file import via PrologImport opcode`
- **Files**: `src/eval/machine/vm/op.rs`, `src/eval/machine/vm/compiler.rs`, `src/eval/machine/vm/run.rs`
- **What**: Add `Opcode::PrologImport { path }`. Update `compiler.rs` to route `.pl` paths (and `(library file.pl)`) to `PrologImport`. Update `Opcode::ImportDynamic` for `.pl` extensions.
- **Proof**: Integration test `!(import! &self "test.pl")` successfully consults file.

### Commit 18: `test(prolog): verify lib_zar and lib_import on bridge`
- **Files**: `examples/prolog_lib_zar.metta`, `examples/prolog_lib_import.metta`
- **What**: Add tests running `lib/lib_zar.metta` and `lib/lib_import.metta`.
- **Proof**: `run_examples.sh` passes both tests.

### Commit 19: `feat(vm): compile translatePredicate to dedicated opcode`
- **Files**: `src/eval/machine/vm/op.rs`, `src/eval/machine/vm/compiler.rs`
- **What**: Introduce `Opcode::TranslatePredicate { head, nargs, vars }`. Compile arguments to evaluate sub-expressions first while preserving variable identity.
- **Proof**: Compiler test verifies AST compilation to `Opcode::TranslatePredicate`.

### Commit 20: `feat(prolog): execute translatePredicate with variable copy-in/copy-out`
- **Files**: `src/eval/prolog/translate.rs`, `src/eval/machine/vm/run.rs`
- **What**: Execute `translatePredicate`. Read variables from environment; convert to terms; execute goal; extract bound variables and return updated `(Atom::sym("true"), Env)`.
- **Proof**: Test `!(translatePredicate (is $x 42))` binds `$x` to `42`.

### Commit 21: `feat(vm): thread environment and branch across translatePredicate solutions`
- **Files**: `src/eval/machine/vm/run.rs`
- **What**: In `Opcode::Progn`, thread `Env` from previous results to subsequent expressions. Branch multiple solutions into distinct `(Atom, Env)` elements in `ResultSet`.
- **Proof**: `examples/translatepredicate.metta` passes: `(progn (translatePredicate (is $x 2)) (translatePredicate (+ $x 40 $z)) $z)` evaluates to `42`.

### Commit 22: `test(prolog): cover succeedsPredicate, tabling, and examples`
- **Files**: `examples/prolog_succeeds_predicate.metta`, `examples/prolog_tabling_fib.metta`
- **What**: Run `examples/spaces_succeedspredicate.metta` and `examples/tabling_fib.metta`.
- **Proof**: Both example files execute successfully and pass with `ok`.

### Commit 23: `test(examples): skip prolog examples when SWI is absent in run_examples.sh`
- **Files**: `run_examples.sh`
- **What**: Update `run_examples.sh` to check if `swipl` binary or `libswipl` is available; if absent, automatically add `prolog_` and Prolog-dependent examples to `SKIP`.
- **Proof**: `./run_examples.sh` passes completely on clean environments without SWI-Prolog.

### Commit 24: `docs: update README with Prolog bridge build and usage instructions`
- **Files**: `README.md`
- **What**: Document `--features prolog-bridge`, prerequisites (`swi-prolog-nox`), and usage examples for `callPredicate`, `import_prolog_function`, and `translatePredicate`.
- **Proof**: Documentation accurate and reviewed.

---

## 5. Verification Checklist for Autonomous LLM Coder

When executing each commit, the coding model must verify:
1. `RUSTC_BOOTSTRAP=1 RUSTFLAGS="-C target-cpu=native" cargo check` passes with default features.
2. `RUSTC_BOOTSTRAP=1 RUSTFLAGS="-C target-cpu=native" cargo check --features prolog-bridge` passes.
3. `RUSTC_BOOTSTRAP=1 RUSTFLAGS="-C target-cpu=native" cargo test --features prolog-bridge` passes.
4. `./run_examples.sh` completes with `0 failed`.
5. Code passes `cargo fmt --check` and `cargo clippy --features prolog-bridge -- -D warnings`.
