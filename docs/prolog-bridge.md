# Prolog Bridge Architecture

The `prolog-bridge` feature embeds an SWI-Prolog runtime into mork-metta through
the SWI C API (`libswipl`), allowing MeTTa programs to call Prolog goals, consult
`.pl` files, and translate MeTTa s-expressions into compiled Prolog goals.

Build with:

```sh
RUSTC_BOOTSTRAP=1 RUSTFLAGS="-C target-cpu=native" cargo build --features prolog-bridge
```

Prerequisites: SWI-Prolog 9/10 (`swi-prolog-nox` on Debian/Ubuntu, or any install
providing `libswipl` and its headers). The runtime is loaded dynamically at
startup, so mork-metta never links against SWI at build time.

## Threading model

SWI-Prolog engines are attached to at most one OS thread at a time, so the
bridge keeps a single engine manager (`engine.rs`) that owns a main engine and
a pool of reusable worker engines. The startup result (engine manager or error)
is cached in a `OnceLock`.

```
MeTTa eval ──▶ EngineManager ──▶ PL_ENGINE_MAIN (mutex, setup/mutations)
                    │
                    └──▶ engine pool (one engine per open query/stream)
```

- **Main engine** (`PL_ENGINE_MAIN`): used for setup, `assert*`, `retract*`,
  consulting files and space sync. Access is serialized behind a mutex
  (`EngineManager::with_main`); mutations made here are visible to all engines.
- **Engine pool**: up to 8 idle `PL_create_engine` engines are kept alive and
  checked out for query execution (imported Prolog functions, `callPredicate`,
  `translatePredicate`). A checked-out engine is attached to the calling thread
  for the duration of an operation and returned to the pool when the operation
  finishes (results are collected eagerly).
- **Shared tabling**: every engine has `table_shared` enabled so tabled
  predicates observe one another's tables.

## Term marshalling

`convert.rs` maps between `mork_metta::atom::Atom` and SWI `term_t`s.

| MeTTa                         | Prolog                        |
| ----------------------------- | ----------------------------- |
| symbol (`foo`, `foo bar` sym) | atom                          |
| string                        | string                        |
| integer / decimal             | `PL_put_int64` / `PL_put_float` (big ints via `dashu`) |
| list expression               | list (`PL_cons_list`)         |
| `(Predicate (f a b))`         | compound `f(a,b)`             |
| `$var`                        | variable (shared through `VarMap`) |
| unrepresentable term          | `PrologForeign` opaque handle |

Outer `(quote ...)` is stripped before conversion. Terms that cannot be
represented as MeTTa atoms (blobs, cyclic terms, dicts) are recorded with
`PL_record` and surfaced as an opaque `PrologForeign` grounded value, replayed
into Prolog terms with `PL_recorded`.

## Queries and streaming

`ops.rs` checks out a pooled engine and opens `call(Goal)` with:

```
PL_Q_NORMAL | PL_Q_NODEBUG | PL_Q_CATCH_EXCEPTION | PL_Q_EXT_STATUS
```

`PL_next_solution` with extended status distinguishes:

- `PL_S_LAST`   — success, no choicepoint remains → `FunctionOutcome::Single`
- `PL_S_TRUE`   — success, more solutions exist   → `FunctionOutcome::Stream`
- `PL_S_FALSE`  — failure                          → `FunctionOutcome::Empty`
- `PL_S_EXCEPTION` — error                         → `FunctionOutcome::Exception`

All solutions are collected eagerly: `FunctionOutcome::Stream` is a fully
drained `IntoIter<Atom>` produced by `collect_rest`, and mid-stream failures are
rendered with `PL_get_chars(..., CVT_WRITE)` (via `PL_exception(qid)`) into
`(Error prolog_stream "<message>")` atoms. This keeps the pooled engine attached
only for the duration of one function call.

## `translatePredicate`

`(translatePredicate (goal ...))` compiles a MeTTa s-expression into a Prolog
goal and runs it against the current engine:

1. Arguments are evaluated and bound variables from the `Env` are substituted.
2. The head symbol and arguments are marshalled into a goal term.
3. The goal is executed; solutions are converted back to atoms.
4. Bound variables are copied out into updated `Env` frames, so
   `(progn (translatePredicate (is $x 2)) (translatePredicate (+ $x 40 $z)) $z)`
   yields `42`.

Because `translatePredicate` must read and update the VM environment, it is
implemented as a dedicated opcode rather than an ordinary native builtin.

## Translator rules

Translator rules are compile-time syntactic macros (PeTTa-style). A rule is
registered with `(add-translator-rule! name)` and backed by a `(= (name ...) ...)`
definition whose body is `(quote <template>)`. During compilation, any
expression headed by a registered rule name is expanded by matching the call
arguments against the definition patterns and substituting them into the
template; the expanded AST is then compiled normally. This is what allows
`(succeedsPredicate pattern)` to expand into a `translatePredicate`/`case`
form.

## Prolog prelude

A small embedded prelude (`ops::init_support`) supplies MeTTa-specific Prolog
predicates that are not part of core SWI-Prolog, such as the three-argument
arithmetic predicates (`'+'(A,B,R) :- R is A + B.`), `&self/N` dynamic
declarations and `file_search_path` entries for `./lib`. It is loaded once at
engine-manager initialization.

## Modules

| File           | Responsibility                                        |
| -------------- | ----------------------------------------------------- |
| `ffi.rs`       | `dlopen2` wrapper + safe SWI C API bindings           |
| `error.rs`     | `PrologError` and exception → `(Error ...)` mapping   |
| `handle.rs`    | `PrologForeign` opaque grounded handle                 |
| `convert.rs`   | `atom_to_term` / `term_to_atom`                        |
| `engine.rs`    | `EngineManager` (main engine + pool + tabling)        |
| `ops.rs`       | query lifecycle, imports, consult/sync, prelude        |
| `builtins.rs`  | native builtin registration                            |
| `vm.rs`        | MeTTa-side hooks: consult, space sync, `translatePredicate` marshalling |
