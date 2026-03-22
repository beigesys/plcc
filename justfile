# plcc — IEC 61131-3 Structured Text Compiler
# Run `just` to see all commands, `just test` for full suite

default:
    @just --list

# Build everything
build:
    cargo build

# Build optimized
build-release:
    cargo build --release

# Run all tests (fast — excludes OSCAT corpus)
test:
    cargo test -p plcc-st --lib --test parse_fixtures --test parse_comprehensive
    cargo test -p plcc-hir
    cargo test -p plcc-runtime
    cargo test -p plcc-codegen -- --test-threads=1
    cargo test -p plcc-hal

# Run only unit tests (fastest)
test-unit:
    cargo test -p plcc-st --lib
    cargo test -p plcc-hir --lib
    cargo test -p plcc-runtime --lib

# Run e2e execution tests (JIT verified)
test-e2e:
    cargo test -p plcc-codegen --test execution --test kitchen_sink --test advanced_execution --test coverage_completion --test fb_execution --test oop_execution --test real_world --test complex_types --test stdlib_execution --test stdlib_complete --test string_datetime --test globals_and_strings -- --test-threads=1

# Run IEC 61131-3 conformance tests
test-conformance:
    cargo test -p plcc-codegen --test iec_conformance -- --test-threads=1
    cargo test -p plcc-hir --test iec_type_conformance

# Run IR safety verification
test-safety:
    cargo test -p plcc-codegen --test ir_safety -- --test-threads=1

# Run cross-compilation tests (6 targets)
test-cross:
    cargo test -p plcc-codegen --test cross_compile -- --test-threads=1

# Run OSCAT corpus test (559 real-world FBs, takes ~3s)
test-oscat:
    cargo test -p plcc-st --test parse_oscat -- --nocapture

# Run HAL simulator tests
test-hal:
    cargo test -p plcc-hal

# Run any ST program with JIT simulation
# Examples:
#   just sim tests/fixtures/programs/blink.st
#   just sim tests/fixtures/programs/pid_simple.st --scans 50
#   just sim tests/fixtures/programs/batch_process.st --scans 70
sim +args:
    cargo run -- sim {{args}}

# Compile an ST file to LLVM IR
compile-ir file:
    cargo run -- compile {{file}} -o /tmp/plcc_output.ll
    @echo "IR written to /tmp/plcc_output.ll"
    @cat /tmp/plcc_output.ll

# Compile an ST file to native object
compile file target="x86_64-unknown-linux-gnu":
    cargo run -- compile {{file}} -o /tmp/plcc_output.o --target {{target}}

# Parse an ST file and dump AST as JSON
parse file:
    cargo run -- parse {{file}} --dump-ast

# Type-check an ST file
check file:
    cargo run -- check {{file}}

# Compile all fixture programs and verify they produce valid IR
test-fixtures-compile:
    #!/usr/bin/env bash
    set -e
    ok=0; fail=0
    for f in tests/fixtures/programs/*.st tests/fixtures/codegen/*.st tests/fixtures/parse/oop.st; do
        if cargo run --quiet -- compile "$f" -o /tmp/plcc_fixture.ll 2>/dev/null; then
            ok=$((ok + 1))
        else
            echo "FAIL: $f"
            fail=$((fail + 1))
        fi
    done
    echo "$ok passed, $fail failed"
    [ $fail -eq 0 ]

# Fetch external test corpora
fetch-external:
    mkdir -p tests/external
    [ -d tests/external/oscat ] || git clone --depth 1 https://github.com/simsum/oscat.git tests/external/oscat
    [ -d tests/external/rusty ] || git clone --depth 1 https://github.com/PLC-lang/rusty.git tests/external/rusty
    [ -d tests/external/iec-checker ] || git clone --depth 1 https://github.com/jubnzv/iec-checker.git tests/external/iec-checker

# Run full test suite including external corpora (everything)
test-all: fetch-external
    just test
    just test-oscat
    just test-fixtures-compile
    just test-conformance
    just test-safety

# Count total tests
test-count:
    #!/usr/bin/env bash
    total=$(cargo test --workspace -- --test-threads=1 --list 2>/dev/null | grep ": test$" | wc -l)
    echo "$total tests found"

# Show test coverage summary
test-summary:
    #!/usr/bin/env bash
    echo "=== Parser ==="
    cargo test -p plcc-st --lib --test parse_fixtures --test parse_comprehensive 2>&1 | grep "test result:"
    echo "=== Type Checker ==="
    cargo test -p plcc-hir 2>&1 | grep "test result:"
    echo "=== Runtime ==="
    cargo test -p plcc-runtime 2>&1 | grep "test result:"
    echo "=== Codegen ==="
    cargo test -p plcc-codegen -- --test-threads=1 2>&1 | grep "test result:"
    echo "=== HAL ==="
    cargo test -p plcc-hal 2>&1 | grep "test result:"

# Run QEMU ARM Cortex-M3 test (requires qemu-system-arm + arm-none-eabi-gcc)
test-qemu:
    bash tests/qemu/run_qemu_test.sh

# Run Renode STM32F4 Discovery test (requires renode + arm-none-eabi-gcc)
test-renode:
    bash tests/qemu/run_renode_test.sh

# Run both hardware emulation tests
test-hw: test-qemu test-renode

# Run Modbus RTU demo: water treatment PLC on STM32F4 + Renode
demo-modbus:
    bash tests/qemu/run_modbus_demo.sh

# Run water treatment PLC with Modbus TCP for FUXA
# Then open http://localhost:1881 and connect to localhost:1502
demo-plc:
    cargo run -- sim tests/fixtures/programs/water_treatment.st --scans 0 --interval-ms 100 --modbus 1502

# Start FUXA SCADA dashboard (Docker)
demo-fuxa-up:
    cd demo && docker compose up -d
    @echo "FUXA SCADA running at http://localhost:1881"
    @echo "Connect to Modbus TCP at localhost:1502"
    @echo "Run 'just demo-plc' in another terminal"

demo-fuxa-down:
    cd demo && docker compose down

# Clean build artifacts
clean:
    cargo clean
