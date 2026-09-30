<!-- SPDX-License-Identifier: MPL-2.0 -->

# The ladder model and ladder translation

plcc reads ladder in two notations: IEC 61131-3 LD exchanged as PLCopen XML
(`docs/ladder.md`) and Rockwell Logix RLL in L5X exports (`docs/l5x.md`). Both
meet in one dialect-neutral **ladder model** (crate `plcc-ladder`), which an
editor can load and save as JSON, and from which plcc writes PLCopen XML, L5X,
rung text and Structured Text.

```bash
plcc convert plant.xml  --to ladder-json -o plant.json   # PLCopen LD → model
plcc convert plant.L5X  --to ladder-json -o plant.json   # L5X RLL → model
plcc convert plant.json --to plcopen -o plant.xml        # model → PLCopen XML (laid out)
plcc convert plant.json --to l5x -o plant.L5X            # model → L5X
plcc convert plant.json --to st                          # model → Structured Text
```

## The model

```
Project { dialect: iec | logix, name, globals: [Variable], pous: [Pou] }
Pou     { id, name, kind: program | function_block | function, return_type?,
          variables: [Variable], routines: [Routine] }
Variable{ name, data_type, section: local | input | output | in_out | external
          | temp | global, initial?, address?, comment?, constant, retain }
Routine { id, name, rungs: [Rung] }
Rung    { id, comment?, label?, elements: [Element] }      -- a series, left to right
Element = contact { id, operand, kind: no | nc | rising | falling }
        | coil    { id, operand, kind: normal | negated | set | reset | rising | falling }
        | branch  { id, legs: [[Element]] }                -- parallel legs, top to bottom
        | block   { id, name, instance?, pins: [Pin], power_in?, power_out? }
        | jump    { id, label } | return { id }
        | st      { id, code }                             -- Structured Text statements
Pin     { name, dir: input | output | in_out, value?, negated, rung? }
```

Every element, rung, routine and POU carries an `id` unique in the project, so
an editor can refer to an element. Elements may carry `notes` (translation
warnings, why a network became an ST box). Operands, pin values and ST-box code
are text: ST expressions in the IEC dialect, Logix operands (tag paths,
immediates, CPT expressions) in the Logix dialect.

A seal-in rung and a TON rung (IEC dialect):

```json
{ "id": 1, "comment": "Seal-in: Motor runs from Start until Stop.", "elements": [
    { "type": "branch", "id": 8, "legs": [
        [ { "type": "contact", "id": 2, "operand": "Start", "kind": "no" } ],
        [ { "type": "contact", "id": 3, "operand": "Motor", "kind": "no" } ] ] },
    { "type": "contact", "id": 4, "operand": "Stop", "kind": "nc" },
    { "type": "coil", "id": 5, "operand": "Motor", "kind": "normal" } ] }

{ "id": 11, "elements": [
    { "type": "contact", "id": 12, "operand": "Enable", "kind": "no" },
    { "type": "block", "id": 14, "name": "TON", "instance": "T1",
      "pins": [ { "name": "IN", "dir": "input" },
                { "name": "PT", "dir": "input", "value": "T#100ms" },
                { "name": "Q", "dir": "output" },
                { "name": "ET", "dir": "output", "value": "Elapsed" } ],
      "power_in": "IN", "power_out": "Q" },
    { "type": "coil", "id": 15, "operand": "Done", "kind": "normal" } ] }
```

The same TON rung in the Logix dialect is
`{ "type": "block", "name": "TON", "pins": [ {"name": "Timer", "value": "T1"},
{"name": "Preset", "value": "100"}, {"name": "Accum", "value": "0"} ] }`: Logix
instructions have positional operands (named after 1756-RM003), no instance,
and the rung condition enters and leaves every instruction, so `power_in` /
`power_out` are not used.

**Blocks.** In the IEC dialect `power_in` names the input pin the rung's power
enters (`EN`, `IN`, `CU`; none: the block takes no power and its output is
ANDed into the rung) and `power_out` the output that continues the rung
(`ENO`, `Q`; none: the rung continues with the power that entered). A pin's
`value` is the expression wired to an input or the variable an output is
written to. A pin's `rung` is a second power-flow path: for an input, the
elements between the left rail and the pin (a counter's reset contact); for an
output, the elements it drives to the right rail. `catalog` in `plcc-ladder`
lists the pins of the IEC standard FBs and operator functions and of 90 Logix
instructions, for an editor's palette.

### Semantics (IEC dialect)

The lowering to ST (`plcc_ladder::lower`) follows `docs/ladder.md`: the left
rail is TRUE; series is AND; a contact is `power AND var` (`AND NOT var`
normally closed, an edge contact the `Q` of a hidden `R_TRIG`/`F_TRIG`); a coil
writes `var := power` (negated `NOT power`; set/reset `IF power THEN var :=
TRUE/FALSE`; edge coils through a hidden edge detector) and passes the power
on; a branch latches the power at the branch point into a hidden BOOL when two
or more legs read it and ORs the legs where they join; an FB call is
`inst(IN := power, ...)`, or `IF power THEN inst(...) END_IF` when power enters
on `EN`; a function is inlined as an expression; jumps use a hidden `_ld_jmp`;
an ST box runs its statements under `IF power THEN`. Hidden variables are named
after the element id (`_ld_p<branch>`, `_ld_en<block>`, `_ld_rt<id>`).

### Semantics (Logix dialect)

The rung condition flows left to right; every instruction runs on every scan
with it (a false rung clears an OTE, resets a TON). This is `docs/l5x.md`
"Ladder (RLL) routines"; the L5X lowering executes the model directly.

## Readers

**Rockwell rung text** (`plcc_ladder::rll::read`): `XIC`/`XIO` → contacts,
`OTE`/`OTL`/`OTU` → coils, `JMP` → jump, `RET()` → return, a leading `LBL` →
the rung's label, every other instruction → a block with its operands as pins
(named from the catalog), `[a,b]` → a branch. Operand text is kept verbatim.
**The L5X compiler lowers RLL from this model**: `plcc-l5x` reads each rung
into the model and lowers the model, so reading, converting and compiling share
one representation (all L5X tests run through it).

**L5X** (`plcc_l5x::ladder::read`): controller tags → globals, each program → a
POU (main routine first), RLL routines → rungs with their comments, ST routines
→ one ST box. Tag types are Logix names (`DINT[10]`); structure initial values
are kept (`(PRE := 150, ACC := 0)` from decorated data, `[0,50,0]` from L5K);
an alias tag has the type `ALIAS` and its target as initial value. Tasks, UDTs,
AOIs and modules are not in the model.

**PLCopen LD** (`plcc_plcopen::ladder::read`): each network (connected
elements; rails do not join networks, so one rail shared by all rungs is fine)
is reduced to series/parallel form:

1. wires are nodes (every connection into one input is one joined wire), the
   left rail is the source, the right rail and every unread output the sink;
2. elements are edges; a block is an edge from its first power-fed input to
   its first power-read output. Inputs fed by `inVariable`s / expressions and
   outputs read by `outVariable`s are data pins (`value`); a second power-fed
   input or power-read output becomes a pin path (`rung`);
3. series and parallel reductions repeat until one edge is left.

A network that does not reduce — a bridge, an output feeding two joins, a
block output wired into another block's data pin, an edge qualifier on a block
pin, an in-out read on its output side — becomes a rung with one **ST box**
holding the ST that network lowers to, with the reason in its notes; nothing is
dropped. A label next to a network becomes that rung's label; a graphical
`<comment>` becomes the comment of the rung below it. ST bodies become an ST
box, FBD bodies an ST box with the ST they lower to. Data types and
configurations are not in the model.

**The PLCopen compiler keeps its direct LD lowering** (`docs/ladder.md`),
which also serves FBD bodies and gives diagnostics exact XML spans (a model
operand is text, its position in the XML is not kept). Instead, an
equivalence test (`crates/plcc-cli/tests/ladder_model.rs`) compiles every LD
fixture both ways — direct, and XML → model → ST — JIT-runs both over 200–300
scans with randomized inputs and a randomly advancing clock, and compares
every variable after every scan (20 000 comparisons over the five fixtures).

## Writers

**Rung text** (`plcc_ladder::rll::write`) is canonical, as Studio 5000 writes
it: operands separated by `,`, every branch leg followed by a space:
`[XIC(Start) ,XIC(Motor) ]XIO(Stop)OTE(Motor);`.

**L5X** (`plcc_l5x::ladder::write`): a controller (1756-L83E) with its local
module, controller tags, one program per POU with its tags and RLL routines
(the first routine is the main routine), and one continuous task scheduling
every program. Tags the model uses but does not declare are derived from
their use: the structure operand of TON/TOF/RTO is a TIMER and of CTU/CTD a
COUNTER; a member `.DN .TT .EN .PRE .ACC` means a TIMER, `.CU .CD .OV .UN` a
COUNTER; contact, coil and one-shot operands are BOOL; other operands DINT (REAL
when the instruction has a REAL literal). Each derived tag is a warning; an
indexed tag that is not declared cannot be sized and is reported. An ST box in
a rung becomes an ST routine `<Routine>_ST<id>` called by `JSR` in its place; a
routine that is one ST box is an ST routine. The file follows the structure of
a Studio 5000 export; it is verified by reading it back with plcc and running
it, not by Studio 5000.

**PLCopen XML** (`plcc_plcopen::ladder::write`), TC6 v2.01 with a generated
layout: a band per rung (comment above it), a grid inside the band (series
left to right, branch legs one below the other, blocks two columns wide and as
tall as their pins), one left and one right power rail per rung, a
`relPosition` on every connection point and the end points of every wire with
one orthogonal bend. Data pins become `inVariable` / `outVariable` elements
beside the pin; pin paths are laid out below the rung. Element ids are kept as
`localId`s (a rung's id is its left rail's `localId`). An ST box becomes an
action `LD_ST_<id>` of the POU (ST body, notes in its documentation) called by a
box of that name through EN/ENO; further routines become actions with LD
bodies; a POU that is one ST box is written with an ST body. plcc's PLCopen
reader compiles actions (as ACTIONs of the PROGRAM / FUNCTION_BLOCK). The TC6
XSD is not freely downloadable from plcopen.org, so the output is checked
against the element order of the schema by the tests and by reading it back.

## Round trips (tested)

| Round trip | Guarantee | Test |
|---|---|---|
| PLCopen LD → model → PLCopen XML → model | the same model, element ids and rung ids included (POU and routine ids are renumbered); the written XML compiles through the direct LD lowering and runs like the original | `plcopen_round_trip_is_identity` |
| RLL text → model → RLL text | identical text for canonical input (105 of the 108 rungs in the fixtures are canonical); any input comes back canonical and stable, equal up to whitespace | `rung_text_round_trip` |
| L5X → model → L5X → model | the same model (ids renumbered in order); the written L5X compiles and runs like the original (fixtures without UDTs, AOIs or event tasks) | `l5x_round_trip` |
| PLCopen LD → model → ST | runs like the direct LD lowering | `plcopen_model_path_runs_like_the_direct_lowering` |
