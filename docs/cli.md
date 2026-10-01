# plcc command line

Every input format and output the `plcc` binary handles. For what the language supports, see [language.md](language.md).

## Examples

```bash
# Parse and check (the same type check `compile` runs first; warnings do not
# stop a build, errors do — `compile --no-typecheck` skips it)
plcc parse program.st --dump-ast
plcc check program.st
plcc check main.st motor.st utils.st

# Compile to LLVM IR
plcc compile program.st -o program.ll

# Compile to native object
plcc compile program.st -o program.o --target thumbv7em-unknown-none-eabi

# Optimized (LLVM default<O2> pipeline; -O0 .. -O3, default -O0)
plcc compile -O2 program.st -o program.o

# Multi-file compilation
plcc compile main.st motor.st utils.st -o system.o

# Ladder / FBD / ST from a PLCopen XML project (mixes with .st files)
plcc compile plant.xml utils.st -o plant.o

# A Beckhoff TwinCAT 3 PLC project (.plcproj, or its directory)
plcc compile MyPlc/MyPlc.plcproj -o plc.o

# A Rockwell Studio 5000 project exported as L5X, I/O bound to %I/%Q
plcc compile plant.L5X --io-map plant_io.toml -o plant.o --target thumbv7em-none-eabi

# For a device from the catalog (target, CPU/FPU flags, process-image sizes)
plcc compile plant.st -o plant.o --device arduino-opta --emit-header plant.h
plcc device list
plcc device check my-board.toml

# A program image for the device's program slot (the runtime is flashed once;
# see docs/program-image.md), and the loader's checks on an image
plcc image plant.o --device arduino-opta -o plant.img --map
plcc image --info plant.img --device arduino-opta

# Any input printed as canonical Structured Text (ladder rungs as the ST they
# lower to, one `(* rung N *)` group per rung)
plcc convert plant.L5X --to st -o plant.st --prelude
```

Ladder Diagram and FBD come in as PLCopen XML (IEC 61131-10) and lower to the
same AST as ST; see [docs/ladder.md](ladder.md). TwinCAT 3 projects
(`.plcproj`, `.TcPOU`, `.TcDUT`, `.TcGVL`, `.TcIO`, `.TcTTO`) compile with their
ST bodies, tasks and CODESYS extensions; see [docs/twincat.md](twincat.md).
Rockwell Logix 5000 projects (`.L5X`: ladder and Logix ST routines, UDTs,
Add-On Instructions, tasks) compile with Logix semantics; see
[docs/l5x.md](l5x.md).

