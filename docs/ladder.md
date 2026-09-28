<!-- SPDX-License-Identifier: MPL-2.0 -->

# Ladder Diagram (and FBD) via PLCopen XML

IEC 61131-3's graphical languages have no text syntax: they are exchanged as
**PLCopen XML** (TC6 XML v2.01, standardized as IEC 61131-10). Editors such as
Beremiz / OpenPLC Editor and CODESYS export it. plcc reads it with the
`plcc-plcopen` crate and lowers every body into the same AST the Structured
Text parser produces. Type checking, codegen, `AT` / the process image,
CONFIGURATION / tasks, the C header and every runtime then work unchanged.

```bash
plcc parse   plant.xml --dump-ast          # the lowered AST
plcc check   plant.xml
plcc compile plant.xml -o plant.o --target thumbv7em-none-eabi --emit-header plant.h
plcc compile plant.xml helpers.st -o plc.o # XML and ST mix freely
plcc sim     plant.xml
```

An input counts as PLCopen when its extension is `.xml` or its root element is
`<project>`. Every other input is parsed as ST. Diagnostics are rendered against
the XML file and point at the element (the `localId`) they are about:

```
  × connection refers to unknown localId 99
    ╭─[ld_bad_wiring.xml:31:34]
 31 │               <connectionPointIn><connection refLocalId="99"/></connectionPointIn>
    ·                                  ──────────────┬──────────────
    ·                                                ╰── here
```

## What is read

| XML | Becomes |
|---|---|
| `<dataTypes><dataType>`: `struct`, `enum` (optional `baseType`, explicit values), `array` (`dimension`s), `subrangeSigned`/`subrangeUnsigned`, `derived` (alias), `string`/`wstring` with `length`, `pointer`, all elementary types | `TYPE ... END_TYPE` |
| `<pou pouType="program\|functionBlock\|function">` | PROGRAM / FUNCTION_BLOCK / FUNCTION |
| `<interface>`: `inputVars`, `outputVars`, `inOutVars`, `localVars`, `tempVars`, `externalVars`, `globalVars`, `accessVars`, `returnType` | VAR blocks; `constant`, `retain` (and `persistent`), `nonretain` |
| `<variable address="%IX0.0">` | `AT %IX0.0` |
| `<initialValue>`: `simpleValue`, `arrayValue` (with `repetitionValue`) | initializer (`[2(10), 20]`) |
| `<body>`: `<ST>` | parsed by the ordinary ST parser, spans remapped onto the XML (CDATA, entities and XHTML wrappers are handled) |
| `<body>`: `<LD>`, `<FBD>` | lowered to ST statements, described below |
| `<configuration>` / `<resource>` / `<task interval priority single>` / `<pouInstance>` / `<globalVars>` | CONFIGURATION / RESOURCE / TASK / program instances. `interval` may be IEC (`T#20ms`) or xsd:duration (`PT0.02S`) |

`<addData>`, `<documentation>`, `<fileHeader>`, `<contentHeader>` and graphical
`<comment>`s are ignored.

## LD and FBD elements

| Element | Supported |
|---|---|
| `leftPowerRail`, `rightPowerRail` | yes |
| `contact`: normal, `negated`, `edge="rising"`, `edge="falling"` | yes |
| `coil`: normal, `negated`, `storage="set"`, `storage="reset"`, `edge="rising"`, `edge="falling"` | yes |
| `block` with `instanceName` (FB call), including `EN`/`ENO`, `inOutVariables`, negated and edge-sensing input pins, negated outputs | yes |
| `block` without `instanceName` (function), including `EN`/`ENO` | yes. Only the result output can be read (see Limitations) |
| `inVariable`, `outVariable`, `inOutVariable` (with `negated`) | yes. The text is any ST expression (`%IX0.0`, `arr[i]`, `fb.Q`, `T#5s`) |
| `connector` / `continuation` | yes |
| `jump`, `label`, `return` | yes, including backward jumps |
| `comment` | ignored |
| `connectionPointIn` holding an `<expression>` instead of connections | yes |
| SFC (`step`, `transition`, `actionBlock`, ...), `vendorElement` | rejected with a diagnostic |

## Lowering rules

**Networks (rungs).** Elements joined by connections form one network, and so
do a connector and every continuation that shares its name. Networks run in
`executionOrderId` order when every network carries one (`0` means unset).
Otherwise they run top to bottom and then left to right, by the `<position>` of
their topmost element.

**Inside a network.** The elements that do something (coils, output and in-out
variables, FB calls, jumps, returns) run in `executionOrderId` order, then by
position. Each one *pulls* the values it needs. A needed value that must be
produced first, such as an FB call whose output it reads or an edge detector,
is emitted just before it. The result is always a valid topological order, and
it follows the IEC rule that an element's inputs are evaluated before the
element.

**Power flow.**

- The left rail is TRUE.
- Elements in series are ANDed.
- Several connections into one input (parallel branches) are ORed.
- A contact is `power AND var`, or `power AND NOT var` when negated.
- A coil writes `var := power` and passes `power` on to anything wired after it.
- A negated coil writes `NOT power`.
- A set coil is `IF power THEN var := TRUE`, and a reset coil does the same with
  FALSE.

The seal-in rung

```
      Start          Stop      Motor
  |----| |----+----|/|-------( )----|
  |    Motor  |
  |----| |----+
```

lowers to one statement, whose span is the `<coil>` element:

```iec
Motor := (Start OR Motor) AND NOT Stop;
```

**Fan-out.** A contact or coil output that feeds more than one element is
latched once into a hidden BOOL. Every branch then sees the value the rung had
at that point, even if an earlier coil in the same rung overwrites the variable:

```iec
_ld_p12 := E;        (* contact E feeds coil X and contact F *)
X := _ld_p12;
Z := _ld_p12;        (* coil Z is in series after coil X *)
Y := _ld_p12 AND F;
```

**Edges.** Edge contacts and edge coils use hidden `R_TRIG` / `F_TRIG` instances
from the bundled standard library, one per element:

```iec
_ld_rt40(CLK := In);
RisePulse := _ld_rt40.Q;        (* In --|P|-- ( RisePulse ) *)
```

A FUNCTION has no state, so an edge element inside one is an error.

**Blocks.**

- An FB block becomes `inst(IN := ..., PT := ...)`. Outputs are read as
  `inst.Q`, after the call.
- With `EN` connected, the call becomes `IF en THEN inst(...) END_IF`, and `ENO`
  reads the same (latched) EN value.
- An instance the interface does not declare is declared automatically.
- Reading an FB output while evaluating that FB's own inputs (FBD feedback) gives
  the previous scan's value, as IEC 61131-3 specifies for feedback paths.
- A function block is inlined as an expression. `ADD`, `MUL`, `AND`, `OR` and
  `XOR` (extensible), `SUB`, `DIV`, `MOD`, `EXPT`, `GT`, `GE`, `EQ`, `LE`, `LT`
  and `NE` (chained: `GT(a,b,c)` is `a>b AND b>c`), `NOT` and `MOVE` become ST
  operators. Anything else becomes a call: positional when every input is
  connected, named otherwise.
- A function with `EN` that feeds an output variable directly lowers to
  `IF en THEN out := F(...) END_IF`, so the output keeps its value while EN is
  FALSE.
- A function whose result nobody reads is called as a statement, for its side
  effects.

**Jumps.** If a body contains a jump, a hidden `_ld_jmp : DINT` is reset at the
top of the body. A taken jump sets it to the number of its label. Every network
is guarded by `IF _ld_jmp = 0`, and each label clears `_ld_jmp` when it is the
target. A backward jump wraps the body in `WHILE TRUE DO ... IF _ld_jmp = 0 THEN
EXIT; END_IF; END_WHILE`, so control re-enters at the label. A jump takes effect
after the network it sits in has run. `return` becomes
`IF power THEN RETURN; END_IF`.

**Hidden variables.** All hidden variables are added to the POU as one extra
`VAR` block. Their names start with `_ld_`: `_ld_p<id>` (fan-out latch),
`_ld_en<id>` (latched EN), `_ld_rt<id>` / `_ld_ft<id>` (edge detectors) and
`_ld_jmp`. They appear in `--emit-header` / `--emit-symbols` like any other
variable.

**Spans.** Every generated statement and expression carries the span of the XML
element it came from: a coil's assignment points at the `<coil>`, and an
identifier points into the `<variable>` text. `plcc parse --dump-ast` shows
them as byte offsets into the `.xml` file.

## Limitations

- **SFC** is not supported yet: `<SFC>` bodies, SFC elements, and POU
  `<actions>` / `<transitions>` are all reported as unsupported. IL bodies are
  rejected (IL is deprecated by IEC 61131-3:2013).
- `<structValue>` initial values are rejected, because the AST has no struct
  aggregate initializer. Use field defaults on the STRUCT type instead.
- Only the result of a function block can be read. Reading a function's other
  `VAR_OUTPUT`s (`OUT2` and so on) is reported as not supported yet.
- When a function with `EN` feeds anything other than an output variable, its
  result is computed whether or not EN is TRUE. ENO still follows EN.
- Existing plcc restrictions still apply to lowered code. Two you may hit:
  - `AT` is only accepted on `VAR` / `VAR_GLOBAL`, so put located variables in
    `localVars` or `globalVars`, not `inputVars`.
  - Codegen does not resolve bare enumerator names yet (`mode := Running`).
- Graphical comments are dropped, and so is vendor `<addData>`.
- plcc validates only what it needs, not the whole TC6 schema. A structural
  error in a body (a dangling `refLocalId`, a duplicate `localId`, a malformed
  element) stops the lowering of that body, so later errors in the same body are
  not reported until it is fixed.

## Running a ladder program on the Arduino Opta

The generic Opta runtime (`~/opta_plcc/runtime/runtime.ino`) runs any module
through the runtime contract. Its process-image map:

| Address | Opta |
|---|---|
| `%IX0.0` .. `%IX0.7` | inputs I1..I8 (digital) |
| `%IX1.0` | USER button |
| `%IW1` .. `%IW8` | I1..I8 analog, raw 0..4095 |
| `%QX0.0` .. `%QX0.3` | relays 1..4 |
| `%QX0.4` | blue USER LED |
| `%MWn` | Modbus holding register n |

Bind LD variables to these addresses with `address=` in the interface, as in
`tests/fixtures/plcopen/ld_seal_in.xml`. There, Start is I1, Stop is I2 and
Motor is relay 1. A `<configuration>` sets the task interval. Without one, plcc
adds an implicit task (`--task-interval`, default `T#20ms`).

`build.sh` only forwards its first argument to `plcc compile`, so the XML flow
needs no changes to it:

```bash
cd ~/opta_plcc
./build.sh /path/to/ld_seal_in.xml             # compile, link the runtime, flash
./build.sh /path/to/ld_seal_in.xml --no-upload
```

The only requirement is that the `plcc` binary `build.sh` points at
(`/home/bherbruck/github/plcc/target/debug/plcc`) was built from a tree that
includes `plcc-plcopen`. This flow was verified by running build.sh's two steps
by hand against a copy of the runtime:

```bash
plcc compile tests/fixtures/plcopen/ld_seal_in.xml -o runtime/plc.o \
    --target thumbv7em-none-eabi --emit-header runtime/plc.h \
    --image-size I=18 --image-size Q=1 --image-size M=64
arduino-cli compile -b arduino:mbed_opta:opta \
    --build-property "compiler.c.elf.extra_flags=$PWD/runtime/plc.o" runtime
# Sketch uses 146464 bytes (7%) of program storage space.
```

The ladder fixtures that use TON/TOF/CTU and FBD also compile to
`thumbv7em-none-eabi` objects. Standard FBs, including the hidden edge
detectors, come from the bundled ST standard library, so there is nothing extra
to link.

## Tests

- Fixtures, all written for plcc: `tests/fixtures/plcopen/`.
  - A seal-in rung with AT I/O and a task.
  - TON/TOF/CTU with EN/ENO.
  - Set/reset, negated and edge coils; edge contacts.
  - Parallel branches, fan-out, connectors.
  - Forward and backward jumps, and return.
  - FBD arithmetic with user FUNCTION and FUNCTION_BLOCK bodies.
  - An ST project with data types and a configuration.
  - Broken files under `errors/`.
- `crates/plcc-plcopen/tests/` lowers each fixture, compiles it with the
  bundled stdlib, JIT-runs it through `plcc_get_app()` over many scans (timers
  on a fake clock), and checks outputs by name via the runtime contract's symbol
  table.
- `crates/plcc-cli/tests/plcopen_inputs.rs` runs the binary: `--dump-ast`,
  `check`, mixed `.xml` + `.st` inputs, a Cortex-M object with `--emit-header`,
  and error rendering.
