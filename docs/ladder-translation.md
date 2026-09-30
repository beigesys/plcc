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

## IEC ↔ Logix translation

```bash
plcc convert plant.xml --to l5x -o plant.L5X          # IEC LD → Logix RLL
plcc convert plant.L5X --to plcopen -o plant.xml      # Logix RLL → IEC LD
plcc convert plant.L5X --to ladder-json --dialect iec # the translated model
```

`plcc_ladder::translate::translate_with` (the CLI uses
`plcc_l5x::ladder::translate`, which parses Logix operands with the L5X
compiler's expression parser) translates element by element and keeps the
rung structure. Every element whose behaviour differs gets a **warning** —
printed by `plcc convert` and kept in the element's `notes` — naming the
element (POU / routine / rung, kind, operand, id) and the difference. What has
no counterpart is **NOT TRANSLATED**: an ST box holding a comment with the
original instruction takes its place, and the warning says why; nothing is
dropped silently. Structure one dialect cannot draw in place becomes **helper
rungs** just before or after the rung (their ids are fresh).

Sources: IEC 61131-3:2013 §8.2 (LD elements), §6.6.3.5 (standard function
blocks: bistables, edge detection, counters, timers) and §6.6.2.5 (standard
functions); Rockwell 1756-RM003 *Logix 5000 Controllers General Instructions*,
chapters *Bit Instructions*, *Timer and Counter Instructions*, *Compare
Instructions*, *Compute/Math Instructions*, *Move/Logical Instructions* and
*Program Control Instructions* (each instruction's operand table and
*Execution* table, including the false-rung and prescan rows).

| IEC (model, IEC dialect) | Logix | Exact? | Difference, and what the translation does |
|---|---|---|---|
| contact `--\| \|--` / `--\|/\|--` | XIC / XIO | yes | |
| coil `--( )--` | OTE | yes, but prescan | Logix clears OTE bits at prescan (the transition to Run); IEC variables start from their initial value — equal at a cold start. One project-level warning. |
| set / reset coil `(S)` `(R)` | OTL / OTU | yes | |
| negated coil `(/)` | OTE of a helper bit + rung `XIO(neg<id>)OTE(var)` after | no | Logix has no negated output; the variable is written after the rung, not at the coil's position (matters only if the same rung reads it later). |
| rising / falling contact `\|P\|` `\|N\|` | helper rung `XIC(v)OSR(edge<id>_sb,edge<id>)` (OSF) before the rung, and XIC(edge<id>) | first scan | Logix has no edge contact. OSR's prescan keeps a variable already TRUE from firing on the first scan; IEC R_TRIG fires. |
| rising / falling coil `(P)` `(N)` | OSR / OSF(var_sb<id>, var) | first scan | as above; the edge memory is a storage bit tag |
| branch, jump, label, return | `[ , ]`, JMP, LBL, RET() | yes | a label moves to the first helper rung inserted before its rung |
| TON / TOF instance `T` (IN = power, PT, Q, ET) | TON / TOF(T, PT in ms, 0); `XIC(T.DN)` continues the rung | ms resolution | The FB instance becomes a TIMER tag: Q ↔ .DN, ET ↔ .ACC, PT ↔ .PRE, IN ↔ .EN — **TIME ↔ DINT milliseconds**. .ACC can pass .PRE by one scan where ET stops at PT. A non-constant PT becomes a `MOV(pt,T.PRE)` rung before; ET / Q copied to variables become `MOV(T.ACC,v)` / `XIC(T.DN)OTE(v)` rungs after. Power on EN (IN from a pin): the timer runs on a leg `EN AND IN`, so EN FALSE resets it (IEC: not called, it freezes) — warned. |
| RTO (plcc FB: IN, R, PT → Q, ET) | RTO(T, PT, 0) + reset | yes (R priority kept) | **RTO is not an IEC standard FB**: plcc's stdlib has one (`crates/plcc-stdlib/st/timers.st`) with the Logix RTO behaviour: ET accumulates while IN, holds while not, R clears it. R becomes `RES(T)`; in the rung itself the rung is split after the timer so the reset follows it, as R wins inside the IEC call. |
| CTU instance `C` (CU = power, R, PV, Q, CV) | CTU(C, PV, 0); `XIC(C.DN)` | no (range) | Q ↔ .DN, CV ↔ .ACC, PV ↔ .PRE. R clears .ACC/.DN/.OV/.UN (not RES, which also clears .CU and would count again while CU stays TRUE); the rung is split after the counter so the reset follows a count of the same scan (R wins in IEC). **IEC CV is an INT that stops at 32,767; Logix .ACC is a DINT that wraps past 2,147,483,647 and sets .OV.** |
| CTD instance (CD, LD, PV, Q, CV) | CTD(C, PV, 0); LD as `MOV(C.PRE,C.ACC)`; Q as `LEQ(C.ACC,0)` | no | Logix .DN is .ACC ≥ .PRE; IEC Q is CV ≤ 0. |
| R_TRIG instance on the power | ONS(inst_sb) | first scan | the storage bit holds the rung state |
| GT GE EQ LE LT NE (result = rung) | GRT GEQ EQU LEQ LES NEQ | yes when power does not enter on EN | Logix compares are input instructions: the rung continues with (rung AND result). |
| ADD SUB MUL DIV MOD EXPT AND OR XOR (EN = power, OUT → v) | ADD SUB MUL DIV MOD XPY AND OR XOR (a, b, v); CPT for 3+ inputs | no (arithmetic) | Logix computes in the widest type of the sources **and the destination**, stores integers with truncation and REAL → integer half-to-even, sets S:V, and DIV by zero gives Source A with a minor fault where IEC faults; logical instructions zero-fill SINT/INT. |
| MOVE, NOT, ABS SQRT SIN COS TAN ASIN ACOS ATAN LN LOG TRUNC | MOV, NOT, ABS SQR SIN COS TAN ASN ACS ATN LN LOG TRN | numeric conversion | as above for the math functions |
| call box of another routine (an action) | JSR(routine, 0) | yes | |
| ST box | an ST routine `<Routine>_ST<id>` called by JSR in its place | syntax | IEC ST and Logix ST differ (FB calls, TIME literals, standard functions): check the code. |
| TP, SR, RS, CTUD, LIMIT, user FBs and functions | — | NOT TRANSLATED | no Logix instruction (Add-On Instructions are not generated) |
| VAR_INPUT / VAR_OUTPUT, FUNCTION_BLOCK / FUNCTION POUs | program tags / Logix programs | — | warned: Logix programs have no directions, and nothing calls a translated FB |
| types: BOOL SINT…LREAL; BYTE WORD DWORD LWORD; TIME; TON TOF RTO; CTU CTD; STRING; `ARRAY[0..n]` | same; SINT INT DINT LINT; DINT ms; TIMER; COUNTER; STRING; `T[n+1]` | | arrays must start at 0; AT addresses are dropped (Logix I/O is module tags; see `--io-map`) |

| Logix | IEC (model) | Exact? | Difference, and what the translation does |
|---|---|---|---|
| XIC / XIO, OTE / OTL / OTU, `[ , ]`, JMP / LBL, RET() | contacts, coils, branch, jump / label, return | yes (OTE prescan as above) | |
| TON / TOF / RTO(T, pre, acc) | TON / TOF / RTO instance T, IN = power, PT = `T#<pre>ms`; the rung continues with the power (Logix timers are output instructions) | ms / ET clamp | T.DN → T.Q, T.ACC → `TIME_TO_DINT(T.ET)`, T.PRE → `TIME_TO_DINT(T.PT)`, T.EN → T.IN, T.TT → `(T.IN AND NOT T.Q)` wherever T's members are read. A `?` preset comes from the tag's initial data. A non-zero initial Accum is not carried. |
| CTU / CTD(C, pre, acc) | CTU (CU = power) / CTD (CD = power); both on one tag: one CTUD instance, each call naming only its own count input | no (range) | C.DN → C.Q (CTUD: QU), C.ACC → C.CV, C.PRE → C.PV; .OV / .UN are not translated (INT vs DINT) |
| RES(T / C) | ST box calling the instance with its reset input: `T(IN := FALSE)` (TON), `C(R := TRUE); C(R := FALSE)` (CTU, CTUD, RTO) | TOF, CTD: no | inputs not named keep their value, so no count or timing starts; an IEC TOF has no reset (IN := FALSE starts the off-delay) and an IEC CTD loads PV — both warned |
| ONS(sb) | coil `os<id>_rung` (latches the rung), then contact `sb` normally closed; rung after: `sb := os<id>_rung` | yes | the storage bit stays a tag and **starts TRUE, as after the Logix prescan**; it is updated after the rung rather than at this point |
| OSR(sb, q) / OSF(sb, q) | the rung latched as for ONS; OSR: `q := rung AND NOT sb` on a leg of its own; OSF: `q := sb AND NOT rung` in a rung after | yes (OSF: q written after the rung) | |
| EQU NEQ LES LEQ GRT GEQ (and EQ NE LT LE GT GE) | GT… box with no power input, result = rung (lowered as rung AND result) | yes | |
| LIM(lo, t, hi) | contact on `((lo <= hi) AND lo <= t AND t <= hi) OR (lo > hi AND (t >= lo OR t <= hi))` | yes | the circular range of RM003 |
| MEQ(s, m, c) | contact on `(s AND m) = (c AND m)` | zero fill | |
| CMP(expr) / CPT(dest, expr) | contact on the expression / ST box `dest := expr;` | precedence made explicit | Logix binds AND/OR/XOR tighter than comparisons; the expression is parsed with Logix precedence and printed with IEC parentheses; `&&` `\|\|` `!` → AND OR NOT; SQR ASN ACS ATN TRN → SQRT ASIN ACOS ATAN TRUNC; a read of `x.[i]` → `(SHR(x, i) AND 1) = 1` |
| ADD SUB MUL DIV MOD XPY AND OR XOR NOT MOV CLR ABS SQR SIN … TRN | ADD … EXPT, AND OR XOR NOT, MOVE, MOVE(0), ABS SQRT SIN … TRUNC boxes (EN = power, OUT → destination) | no (arithmetic) | as in the other direction |
| NEG | ST box `d := -s;` | yes | |
| AFI / NOP / TND | contact on FALSE / left out / RETURN | TND: no | TND ends the task's scan; RETURN ends only this POU |
| JSR(routine, 0) | call box of the routine (an action of the POU) | yes | JSR with parameters is NOT TRANSLATED |
| S:FS | the program variable S_FS, TRUE until a last rung `S_FS := FALSE` | yes | |
| S:V S:Z S:N, module tags `Local:1:I`, writes to `x.[i]` | — | NOT TRANSLATED | |
| MCR, UID/UIE, GSV/SSV, MSG, EVENT, FOR/BRK, SBR/RET with parameters, COP/CPS/FLL, BSL/BSR, FIFO/LIFO, BTD, MVM, SWPB, string instructions, SIZE, TOD/FRD, AOIs | — | NOT TRANSLATED | no IEC ladder counterpart |
| TIMER / COUNTER tags; `DINT[10]`; STRING; alias tags; UDTs | TON/TOF/RTO, CTU/CTD/CTUD instances (by use); `ARRAY[0..9] OF DINT`; STRING[82]; — ; the type name | | |

**Tested** (`crates/plcc-cli/tests/ladder_translate.rs`): an IEC model with
every mapped element (seal-in, set/reset, edge contact and coils, negated coil,
TON, TOF, RTO and CTU with resets, ADD, MOVE, GT) and the PLCopen fixtures
`ld_seal_in` and `ld_coils_edges` are translated to Logix, written as L5X,
compiled with Logix semantics and run against the IEC originals over 300–400
randomized scans; the L5X fixtures `seal_in`, `bits_branches` and
`timers_counters` are translated to IEC and run against the Logix originals.
Variables are compared by name after every scan from the second on (inputs
stay FALSE during the first, so prescan effects do not set the runs apart).
The `math` fixture, which exists to exercise Logix-only arithmetic, is checked
for its warnings instead.

## Structured Text → ladder

```bash
plcc convert motor.st --to plcopen -o motor.xml     # the drawable subset as LD rungs
plcc convert motor.st --to ladder-json              # the model
```

`plcc_ladder::from_st` draws what ladder can express **exactly** and puts
everything else in ST boxes, one rung per statement, in statement order:

| Statement | Rung |
|---|---|
| `x := <boolean expression>` (x and every operand BOOL) | contacts — AND in series, OR as parallel legs, NOT as a normally closed contact, NOT of AND/OR by De Morgan — and a coil on `x` |
| `IF c THEN x := TRUE; y := FALSE; END_IF` (only TRUE/FALSE into BOOLs; no ELSIF/ELSE) | contacts for `c`, set / reset coils (parallel legs when several) |
| `RETURN;`, `IF c THEN RETURN; END_IF` | contacts for `c`, a return |
| a comparison in a condition, `a > b` | a GT/GE/EQ/LE/LT/NE box run by the rung (EN/ENO), writing its result into a hidden BOOL `ld_cmp<n>` (declared on the POU), and a contact on it. Operands with a call, a division or a computed index stay in ST (the box runs only when the rung reaches it; the ST always evaluated them) |
| `inst(IN := c, PT := T#5s, Q => y)` of a standard FB (TON TOF TP RTO CTU CTD CTUD R_TRIG F_TRIG SR RS) with named arguments | contacts for the power input (IN, CU, CD, CLK, S1, S) when it is a drawable condition, the FB box with its other inputs as pin values; `v := inst.OUT` statements right after the call become output pins, and `x := inst.Q` (the FB's power output) a coil after the box |
| a call of an FB declared in the file, named arguments | the box (no power input: it is called every scan) |
| `x := a + b` (`-` `*` `/` `MOD`) | an ADD / SUB / MUL / DIV / MOD box |
| any other assignment | a MOVE box with the expression on its input |
| loops, CASE, other IFs, calls with positional arguments or of functions, EXIT, … | an ST box holding the statement |

Types come from the declarations (and those of standard FB members); an
assignment whose target or operands are not known to be BOOL is not drawn with
contacts. Declarations that are not POUs (TYPEs, CLASSes, INTERFACEs,
CONFIGURATIONs) and a POU's METHODs / PROPERTYs / ACTIONs are carried along
in the model as ST (`Project::declarations`, `Pou::members`), so ST → ladder →
ST loses nothing; PLCopen and L5X output name them in a warning, as those
formats do not hold them here.

**Guarantee: the ladder computes what the ST did.** Tested
(`crates/plcc-cli/tests/ladder_from_st.rs`) by running the input and the
model lowered back to ST side by side (JIT) over randomized inputs, comparing
every variable after every scan: all 18 ST fixtures that compile on their own
(46 of their 95 rungs are drawn, 49 are ST boxes: loops, CASE, IF with other statements, calls of functions) and a generated corpus of
60 programs built from the drawable statements with random conditions, FB
calls, arithmetic and loops (409 rungs, 330 drawn as ladder), 150–200 scans
each.

## Round trips (tested)

| Round trip | Guarantee | Test |
|---|---|---|
| PLCopen LD → model → PLCopen XML → model | the same model, element ids and rung ids included (POU and routine ids are renumbered); the written XML compiles through the direct LD lowering and runs like the original | `plcopen_round_trip_is_identity` |
| RLL text → model → RLL text | identical text for canonical input (105 of the 108 rungs in the fixtures are canonical); any input comes back canonical and stable, equal up to whitespace | `rung_text_round_trip` |
| L5X → model → L5X → model | the same model (ids renumbered in order); the written L5X compiles and runs like the original (fixtures without UDTs, AOIs or event tasks) | `l5x_round_trip` |
| PLCopen LD → model → ST | runs like the direct LD lowering | `plcopen_model_path_runs_like_the_direct_lowering` |
| LD → ST → LD | the same rungs for rungs in canonical form (below); any LD comes back running the same | `ld_to_st_to_ld_is_identity_for_canonical_rungs`, `ld_to_st_to_ld_runs_like_ld` |
| ST → LD → ST | runs like the input (drawable statements as rungs, the rest in ST boxes) | `st_fixtures_convert_exactly`, `generated_programs_convert_exactly` |
| IEC ↔ Logix | runs like the original, up to the differences each warning names | `iec_to_logix_runs_alike`, `logix_to_iec_runs_alike` |
| model → PLCopen XML | follows the TC6 v2.01 element order, unique `localId`s, every connection to an element of its body with its end points, a `relPosition` on every connection point; reads back without errors | `written_plcopen_follows_the_schema` |

**Canonical form** (for LD → ST → LD). The IEC lowering prints a canonical
rung as one statement, or one FB call and the copies of its outputs, and
`from_st` draws that back as the same rung:

- contacts and branches of contacts, then **one** coil:
  `x := a AND NOT b;`, `y := (a OR (b AND NOT c)) AND d;` (a branch of
  contacts needs no latch, so it prints as a parenthesized OR);
- contacts, then one set or reset coil: `IF c THEN x := TRUE; END_IF;`;
- contacts into the power input of a standard FB (IN, CU, CD, CLK, S1, S) whose
  other inputs are values, whose outputs go to variables, with at most one coil
  on its power output (Q):
  `T1(IN := a, PT := T#100ms); Elapsed := T1.ET; Done := T1.Q;`.

Not canonical, and what they come back as (running the same): several coils in
one rung (one rung per coil); a branch holding coils or boxes, which latches
the power in a hidden `_ld_p<id>` (a rung assigning it); a compare box
(`x := a > b` comes back as a compare box writing `ld_cmp<n>` and a contact on
it); a negated coil (`x := NOT y` comes back as a normally closed contact);
edge contacts and coils (their hidden `R_TRIG` calls); an FB with power on EN
(an `IF` around the call becomes an ST box); jumps (the `_ld_jmp` logic).

## Limits

- **PLCopen compiles through its own LD lowering**, not the model (the model
  path is checked against it); FBD networks are not in the model (an FBD body
  is an ST box).
- **Not in the model**: data types, configurations and tasks, Logix UDTs, AOIs,
  modules and tasks (L5X output schedules every program in one continuous
  task); graphical positions (the writer lays out afresh).
- **Non-series-parallel LD networks** are ST boxes (with the ST they lower to);
  editing such a rung in a ladder editor means editing ST.
- **L5X output** is checked by plcc's reader, not by Studio 5000; undeclared
  indexed tags (`a[3]`) cannot be sized and must be declared.
- **Translation** is per element; what has no counterpart is NOT TRANSLATED
  (an ST box with a comment), never dropped. Logix status flags (S:V, S:Z,
  S:N), module tags, MCR, file/array, FIFO and string instructions, AOIs and
  JSR parameters have no IEC ladder form here; neither do IEC TP, SR, RS,
  CTUD, LIMIT and user FBs in Logix.
