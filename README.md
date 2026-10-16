# Mork MeTTa (`mork-metta`)

A high-performance Meta Type Talk (MeTTa) execution engine written in Rust. It combines a bytecode compiler and virtual machine evaluation with fast space indexing via MORK / PathMap tries, and native foreign language interoperability (SWI-Prolog and Python).

---

## Features

- **Bytecode Compiler & Register VM**: Compiles MeTTa definitions and expressions into efficient bytecode opcodes (`Opcode::TranslatePredicate`, `Opcode::PrologImport`, etc.) for high-speed evaluation.
- **Space Indexing**: Integrated with MORK / PathMap tries for fast atom indexing, pattern querying, and space synchronization.
- **SWI-Prolog Interop (`prolog-bridge`)**:
  - Full bidirectional type marshaling between MeTTa Atoms and Prolog terms (integers, bignums, floats, symbols, strings, lists, compound terms, and arithmetic/logical operators).
  - Multi-engine checkout pool with thread-safe engine reuse and recycling.
  - Dedicated worker thread for single-threaded file compilation (`consult/1`).
  - Non-deterministic query evaluation returning lazy solution streams.
  - Dynamic clause assertion (`asserta`/`assertz`) and retraction (`retract`).
  - Transparent function importation into MeTTa syntax with dynamic arity resolution.
  - Boolean branching support with `succeedsPredicate` for MeTTa `if` conditions.
- **Python Interop (`python-bridge`)**: Embedded PyO3 bridge for calling Python routines directly from MeTTa.

---

## Prerequisites

- **Rust**: Nightly or with bootstrap flag (`RUSTC_BOOTSTRAP=1`).
- **SWI-Prolog** (only required when using `--features prolog-bridge`):
  - **Ubuntu / Debian**:
    ```bash
    sudo apt-get install swi-prolog-nox
    ```
  - **Fedora / RHEL**:
    ```bash
    sudo dnf install pl
    ```
  - **macOS**:
    ```bash
    brew install swi-prolog
    ```
  - **Manual / Custom Install**:
    If `libswipl.so` (or `libswipl.dylib`) is in a custom path (e.g. `~/.local/lib`), ensure it is discoverable via `LD_LIBRARY_PATH`:
    ```bash
    export LD_LIBRARY_PATH="$HOME/.local/lib:$LD_LIBRARY_PATH"
    ```

---

## Building

```bash
# Standard build without foreign bridges
RUSTC_BOOTSTRAP=1 cargo build --release

# With SWI-Prolog bridge enabled
RUSTC_BOOTSTRAP=1 cargo build --release --features prolog-bridge

# With both SWI-Prolog and Python bridges enabled
RUSTC_BOOTSTRAP=1 cargo build --release --features prolog-bridge,python-bridge
```

---

## Running

Run any `.metta` file directly with `cargo run`:

```bash
# Run a standard MeTTa script
cargo run -- <file.metta>

# Run with Prolog bridge feature enabled
cargo run --features prolog-bridge -- examples/prolog_lib_import.metta
```

---

## SWI-Prolog Interop Quickstart

When compiled with `--features prolog-bridge`, MeTTa code can interact with SWI-Prolog natively using built-in operations.

### 1. Asserting and Retracting Clauses

```metta
; Assert facts into Prolog knowledge base
!(assertzPredicate (parent Pam Bob))
!(assertzPredicate (parent Tom Bob))
!(assertzPredicate (parent Tom Liz))
!(assertzPredicate (parent Bob Ann))

; Retract a fact
!(retractPredicate (parent Tom Liz))
```

### 2. Querying Predicates

```metta
; Query all children of Bob (returns a stream of solutions)
!(callPredicate (parent Bob $child))
; Output: [Ann]

; Query with multiple variables
!(callPredicate (parent $p $c))
; Output: [Pam, Bob], [Tom, Bob], [Bob, Ann]
```

### 3. Boolean Branching with `succeedsPredicate`

`succeedsPredicate` evaluates a predicate and returns `True` or `False`, making it directly usable inside MeTTa `if` expressions:

```metta
!(if (succeedsPredicate (parent Pam Bob))
     "Pam is Bob's parent"
     "Not related")
```

### 4. Importing Prolog Predicates as MeTTa Functions

Import predicates from Prolog libraries or external files directly into the MeTTa function table:

```metta
; Import Prolog standard library predicate member/2
!(import_prolog_function member 2)

; Call it directly in MeTTa syntax
!(member $x (1 2 3))
; Solutions: 1, 2, 3
```

Import from external `.pl` files:

```metta
; Import a custom rule from a Prolog file
!(import_prolog_functions_from_file_pred "my_rules.pl" ancestor 2)
!(ancestor Tom $descendant)
```

For detailed architectural design, memory lifecycle, C-FFI details, and type mapping rules, see [`docs/prolog-bridge.md`](docs/prolog-bridge.md).

---

## Testing

```bash
# Run the Prolog bridge unit and integration tests
RUSTC_BOOTSTRAP=1 RUSTFLAGS="-C target-cpu=native" cargo test \
  --test prolog_bridge_tests \
  --test prolog_interop_tests \
  --features prolog-bridge

# Run the full example test suite
./run_examples.sh
```

---

## License

This project is licensed under the Apache License, Version 2.0 or the MIT License, at your option.
