<!-- SPDX-License-Identifier: MPL-2.0 -->

# Rockwell Logix 5000 projects (.L5X)

plcc compiles Studio 5000 / RSLogix 5000 projects exported as **L5X** (Logix
Designer's XML export) so they run on plcc targets, for example the Arduino Opta
generic runtime. Logix is the second dialect next to CODESYS: where Logix and IEC
61131-3 disagree, plcc does what a Logix controller does.

The `plcc-l5x` crate reads the export and lowers it to the same AST the
Structured Text parser produces, so type checking, codegen, the process image,
tasks, the C header and every runtime work unchanged. The Logix behaviour lives
in two places: the lowering (rung-condition flow, prescan, instruction
semantics) and a bundled ST prelude, `crates/plcc-l5x/st/logix.st` (TIMER,
COUNTER, CONTROL, the timer/counter/one-shot algorithms, the conversion rules),
which the CLI compiles with every L5X input.

```bash
plcc check   plant.L5X
plcc parse   plant.L5X --dump-st        # the Structured Text it lowers to
plcc compile plant.L5X -o plant.o --target thumbv7em-none-eabi \
             --emit-header plant.h --io-map plant_io.toml
plcc sim     plant.L5X
```

An input is L5X when its extension is `.L5X` (any case) or its root element is
`<RSLogix5000Content>`. L5X and `.st` inputs mix. Diagnostics point into the
`.L5X` file: the rung text, the operand, the tag or routine element.

```
  × unknown tag `Missing`
   ╭─[bad_rungs.L5X:17:52]
17 │ <Rung Number="1" Type="N"><Text><![CDATA[XIC(Missing)OTE(A);]]></Text></Rung>
   ·                                              ───┬───
   ·                                                 ╰── here
```

Sources. Behaviour follows Rockwell's public manuals; section names are cited
throughout this document and in the code:

- **RM003** — *Logix 5000 Controllers General Instructions Reference Manual*,
  1756-RM003 (current edition published as 1756-RM018A-EN-P, September 2025).
- **RM014** — *Logix 5000 Controllers Import/Export Reference Manual*,
  1756-RM084 (current edition 1756-RM014D-EN-P, September 2025): the L5X schema.
- **PM007** — *Logix 5000 Controllers Structured Text Programming Manual*,
  1756-PM007.

## What is read

| L5X | Becomes |
|---|---|
| `<DataTypes><DataType>` (user-defined) | `TYPE ... STRUCT`. BOOL members (`DataType="BIT"`, packed by Logix into hidden `SINT` hosts) become ordinary BOOL fields; the hidden hosts are dropped. |
| `<DataType Family="StringFamily">`, `STRING` | a structure `LEN : DINT; DATA : ARRAY[0..n-1] OF SINT` (`STRING` is 82 characters) |
| predefined `TIMER`, `COUNTER`, `CONTROL`, `FBD_TIMER`, `FBD_COUNTER`, `FBD_ONESHOT` | prelude structures with the Logix members (`PRE ACC EN TT DN`, `PRE ACC CU CD DN OV UN`, `LEN POS EN EU DN EM ER UL IN FD`, ...) |
| `<AddOnInstructionDefinitions>` | `FUNCTION_BLOCK`: Input → `VAR_INPUT`, Output → `VAR_OUTPUT`, InOut → `VAR_IN_OUT`, local tags → `VAR`, plus `EnableIn` (default TRUE) / `EnableOut`. Logic, Prescan and EnableInFalse routines (RLL or ST). |
| `<Modules>` connection `InputTag` / `OutputTag` / `ConfigTag` (and GuardLogix `SafetyInputTag` / `SafetyOutputTag`) | module tags `Local:<slot>:I` (local chassis) or `<Module>:I` (`:O`, `:C`, `:SI`, `:SO`), typed from their decorated data; several connections carrying the same tag are one tag |
| controller `<Tags>` | one `VAR_GLOBAL` block |
| `<Data Format="Decorated">`, `<Data Format="L5K">`, `<Data Format="String">`, AOI `<DefaultData>` | initial values (`[0,5000,0]` for a TIMER is control word, PRE, ACC; UDT L5K values follow the physical member order, hidden hosts included) |
| alias tags (`TagType="Alias" AliasFor="..."`) | resolved: every use of the alias is a use of the target path |
| `<Programs><Program>` | `FUNCTION_BLOCK lx__P_<name>` holding the program tags, one `METHOD R_<routine>` per routine, and a global instance named like the program |
| `<Routine Type="RLL">` | lowered rung by rung (below) |
| `<Routine Type="ST">` | Logix ST, handed to the ST parser (below) |
| `<Routine Type="FBD">`, `"SFC"` | **error**: not supported yet |
| `<Tasks>` | `CONFIGURATION` / `TASK` (below) |
| `<EncodedData>` (source-protected routines, AOIs) | **error**: cannot be compiled |

`MESSAGE` and `PID` are modelled as structures with their documented members
(MSG never completes; the PID instruction itself is not implemented yet).
`MODULE` (an AOI InOut that refers to an I/O module for GSV/SSV) is an opaque
placeholder, and a module's name can be passed to it. Tags of a type plcc
cannot model (motion axes and groups, `ALARM_*`, `COORDINATE_SYSTEM`,
drive-specific module types without decorated data, a UDT or string type the
export does not include, ...) are left out with a warning; using one is an
error at the use.

### Names

Logix names contain letters, digits and single underscores and never two
underscores in a row, so every name plcc adds contains `__`:

- hidden variables and helpers are `lx__...` (`lx__rc`, `lx__put_DINT_i`);
- a Logix name that is an ST keyword gets a `__` suffix: a tag called `Time`,
  `DINT`, `Word` or `Step` is `Time__` in the generated ST and the symbol table;
- `:` in module tags and module-defined types becomes `__`: `Local:1:I` is
  `Local__1__I`, `AB:1769_DI16:I:0` is `AB__1769_DI16__I__0`.

Program tags are members of the program's global instance, so
`Program:MainProgram.Motor` (also written `\MainProgram.Motor`, the form of
program-parameter connections) is `MainProgram.Motor` in ST and the symbol table;
controller tags are plain globals (`GLOBAL.Motor` in `--emit-symbols`).

## Ladder (RLL) routines

Rung text is the neutral text of RM014 "Neutral text for ladder instructions":
`XIC(a)XIO(b)[OTE(c),OTL(d)]OTU(e);`, with nested branches, empty legs, and
operands that are tag paths (`tag.member`, `arr[i,j]`, bit numbers `tag.5`,
indirect bits `tag.[n]`, read and written), immediate values in any radix (`16#FF`,
`2#1010`, `8#17`, `1.$` for infinity), `?` for an unset pseudo-operand, and CPT/CMP
expressions.

**Rung condition.** Each rung starts with rung-condition-in TRUE and it flows left
to right (RM003, every instruction's *Execution* table). Input instructions
(XIC, XIO, compares, ONS, AOIs through EnableOut) set the condition for what
follows; output instructions act on it and pass it on unchanged. **Every
instruction runs on every scan, true or false**: a false rung clears an OTE,
resets a TON, clears an ONS storage bit, clears a counter's CU. A branch starts
each leg with the condition at the branch and ORs the legs where they join.
Instructions execute in the order they appear, legs top to bottom, so a later
instruction sees what an earlier one on the same rung wrote. The generated code
is a straight sequence of statements over a rung-condition variable:

```iec
lx__rc := TRUE;                          (* [XIC(Start),XIC(Motor)]XIO(Stop)OTE(Motor); *)
lx__bs1 := lx__rc; lx__bo1 := FALSE;
lx__rc := lx__rc AND Start;       lx__bo1 := lx__bo1 OR lx__rc;
lx__rc := lx__bs1;
lx__rc := lx__rc AND Motor;       lx__bo1 := lx__bo1 OR lx__rc;
lx__rc := lx__bo1;
lx__rc := lx__rc AND NOT Stop;
Motor := lx__rc;
```

**Prescan.** On the transition to Run a Logix controller prescans every routine
before the first scan; each instruction's *Prescan* row says what that does (OTE
clears its bit, ONS sets its storage bit so an input already TRUE does not
fire, OSR sets its storage bit and clears its output, OSF clears both, TON and
TOF clear EN/TT/DN and set ACC to 0 (TON) or PRE (TOF), RTO clears EN/TT, CTU and
CTD set CU/CD so a count input already TRUE does not count). plcc collects those
actions from every routine of a program into a `lx__prescan` method that runs
once, just before the program's first scan. Pseudo-operands (the Preset and
Accum operands of TON/TOF/RTO/CTU/CTD, RM003 "Pseudo-operand initialization":
"initialized when the application is downloaded") are applied at the same point.

**S:FS** (first scan) is TRUE during each program's first scan. **S:V, S:Z, S:N**
are set by the math and move instructions from the value they store (RM003 "Math
status flags": "the math status flags are set based on the stored value"): the
instruction clears S:V, and any overflow on the way to the stored value (a result
that does not fit the destination, a divide by zero) sets it. An overflow also
sets **S:MINOR** and counts a minor fault (type 4, code 4) in
`lx__minor_faults`; plcc has no fault log and does not clear S:MINOR by itself.
Status flags are only updated by ladder instructions, as on a controller.

**Program control.**

- `JSR(Routine, n, in1..inn, ret1..retm)` calls the routine's method; input
  parameters are copied into the tags of the subroutine's `SBR`, and the
  values of the `RET(v1..vm)` that ended the subroutine into the JSR's return
  operands (RM003 JSR/SBR/RET), converted to their types. `SBR` is then a
  no-op.
- `RET()` and `TND()` end the routine ("If the TND instruction is in a
  subroutine, control returns to the calling routine").
- `JMP(L)` / `LBL(L)`: a taken jump skips the rest of its rung and every rung up
  to the one whose first instruction is `LBL(L)`, forward or backward (a
  backward jump loops within the scan, as on a controller).
- `MCR()`: "Each time the MCR instruction is executed with rung-condition-in
  false, the override behavior is toggled"; while the override is on, every rung
  starts FALSE (the MCR rungs included). The override resets at the start of each
  routine.
- `AFI()` makes the rung false; `NOP()` does nothing.
- `EVENT(Task)` triggers an event task (below).
- An Add-On Instruction `Aoi(tag, p1, ..., pn)` takes its backing tag and the
  required parameters in definition order: Inputs are passed by value, InOuts
  by reference, Outputs copied to their operand after the call; EnableIn is the
  rung condition and the rung continues with EnableOut. The AOI's Logic runs
  only when EnableIn is TRUE; its EnableInFalse routine runs when it is FALSE if
  "Execute EnableInFalse" is set; its prescan (Logic prescan actions, then the
  Prescan routine if "Execute Prescan" is set) runs from the caller's prescan.

## Instructions

| Instruction | Semantics (RM003) |
|---|---|
| XIC XIO | rung AND bit / rung AND NOT bit |
| OTE | bit := rung (cleared on a false rung and at prescan) |
| OTL OTU | set / clear the bit when the rung is true |
| ONS | true rung: out = NOT storage, storage := 1; false: storage := 0 |
| OSR OSF | output bit TRUE for one scan on the rung's rising / falling edge |
| TON TOF RTO | the *Flow Chart (True/False)* of each: time base 1 ms; `ACC = ACC + (current_time - last_time_scanned)`; ACC saturates at 2,147,483,647; **ACC is not clamped to PRE** when done (TON/RTO stop accumulating once DN is set); setting DN pauses a TON/RTO, clearing it pauses a TOF. Whole milliseconds are added and the sub-millisecond remainder carried, so timers do not drift with the scan period. |
| CTU CTD | count on the rung's false→true transition (CU/CD hold the last rung state); past 2,147,483,647 ACC wraps and OV sets (UN for CTD); an overflow while UN is set (or underflow while OV is set) clears both; DN = ACC ≥ PRE, updated only while neither OV nor UN is set |
| RES | TIMER / COUNTER: ACC := 0 and status bits cleared; CONTROL: POS := 0 and status bits cleared |
| EQU/EQ NEQ/NE LES/LT LEQ/LE GRT/GT GEQ/GE | compare in the higher-ranked type of the two operands (DINT immediate vs REAL tag compares as REAL). The v36 mnemonics (EQ, NE, ...) and the older ones are both accepted. |
| LIM | Low ≤ High: Low ≤ Test ≤ High; Low > High: the circular range Test ≥ Low OR Test ≤ High |
| MEQ | (Source AND Mask) = (Compare AND Mask), SINT/INT zero-filled |
| CMP | the expression is TRUE. CPT/CMP operator precedence follows RM003 "Determine the order of operation" — **AND/OR/XOR bind tighter than the comparisons**, so `a > b AND c` is `a > (b AND c)`; use `&&` / `||` for logical conditions |
| ADD SUB MUL DIV MOD XPY | computed in the highest-ranked type of the sources **and the destination** (RM003 "Data conversions": SINT, USINT, INT, UINT, DINT, UDINT, LINT, ULINT, REAL, LREAL), integers at 64 bits; the result is stored into the destination (below). Integer DIV truncates; DIV with a REAL destination divides in REAL. **DIV by zero: Source A** for integers, ±infinity (`1.$`) for REAL, and a minor fault; **MOD by zero: 0** and a minor fault (RM003 DIV/MOD footnotes). |
| CPT | expression evaluated, integers at 64 bits, stored into the destination |
| NEG ABS SQR/SQRT SIN COS TAN ASN/ASIN ACS/ACOS ATN/ATAN LN LOG DEG RAD TRN TOD FRD | as named; transcendental functions evaluated in LREAL and rounded to REAL; TRN drops the fraction and keeps the type; TOD/FRD convert to/from BCD (S:V outside 0..99,999,999) |
| MOV/MOVE | numeric: converted and stored (below); same-type structures: copied |
| MVM | Dest := (Source AND Mask) OR (Dest AND NOT Mask) |
| CLR | Dest := 0 (sets S:Z) |
| AND OR XOR NOT (and BAND BOR BXOR) | bitwise, SINT/INT/DINT sources **zero-filled** ("Logical instructions use zero fill. All other instructions use sign-extension") |
| BTD | Length bits from Source (from Source bit) into Dest (from Dest bit); no flags |
| COP CPS | copy Length elements of the destination's type, element by element, stopping at the end of either array. Source and destination must have the same element type. |
| FLL | fill Length destination elements (stops at the end of the array) |
| BSL BSR | shift a bit into bit 0 (BSL) or bit Length-1 (BSR) of a DINT array on the rung's rising edge; the bit shifted out goes to `.UL` (RM003 flow charts: EN, DN, ER, UL; Length 0 just unloads Source) |
| FFL FFU LFL LFU | FIFO / LIFO over an array and a CONTROL, on the rung's false→true transition (EN for loads, EU for unloads): FFL/LFL load Source at `.POS` and advance it unless full; FFU returns element 0 and shifts elements 1..LEN-1 down; LFU returns element POS-1 and stores 0 there; an empty stack returns 0. DN = `.POS >= .LEN`, EM = `.POS = 0`, both set when `.LEN <= 0` or `.POS < 0`. Prescan sets EN/EU "to prevent a false load/unload when scan begins". Numbers are converted to the element type without status flags; strings and same-type structures are copied whole. |
| SWPB | INT: swap the two bytes (sign-extended into a DINT Dest); DINT: REVERSE (ABCD→DCBA), WORD (→CDAB), HIGH/LOW (→BADC); no status flags. Also in ST. |
| FOR BRK | FOR(Routine, Index, Initial, Terminal, Step) runs the routine in a loop within the scan; BRK leaves the innermost FOR |
| SIZE | a constant: plcc arrays have fixed bounds |
| CONCAT MID DELETE INSERT FIND UPPER LOWER DTOS STOD | on Logix strings (`LEN` + `DATA`), 1-based positions, truncation to the destination with S:V; also in ST |
| UID UIE | no-ops: plcc tasks do not preempt one another (docs/process-image.md), so there is no user-task interrupt to disable |
| GSV SSV | accepted with a warning: plcc has no controller object model; GSV leaves its destination unchanged, SSV sets nothing |
| MSG | accepted with a warning: no CIP messaging; the message never starts (EN, DN, ER stay FALSE) |
| JSR SBR RET JMP LBL MCR TND AFI NOP EVENT | above |
| ST: TONR TOFR RTOR CTUD OSRI OSFI | FBD_TIMER / FBD_COUNTER / FBD_ONESHOT instructions, per their RM003 *Execution* tables (Reset handling, first-run behaviour, PresetInv) |

**Storing a result** (RM003 "Data conversions", "Elementary data types"):

- An integer that does not fit its destination keeps its low bits ("truncates the
  upper portion of the larger integer and generates an overflow"): SINT 100 +
  100 is -56 with S:V.
- REAL → integer rounds to nearest, **ties to even** ("Fractions = 0.5 round up
  or down to the nearest even number": 2.5 → 2, 3.5 → 4, -2.5 → -2), then keeps the
  low bits. plcc's own IEC conversions round ties away from zero, like CODESYS;
  Logix code goes through the prelude instead.
- SINT/INT sources are sign-extended, except in the logical instructions.
- An immediate integer is a DINT (RM003 "Immediate values"); a REAL immediate is
  a REAL.

## Structured Text routines

Logix ST reads like IEC ST — same statements, and PM007 "Determine the order of
execution" gives it IEC operator precedence (unlike CPT) — so a routine's text
goes to the plcc-st parser after a token-level pass:

- tag paths are resolved through the Logix scopes exactly like ladder operands
  (aliases, `Local:1:I.Data`, keyword-named tags, `Program:X.Tag`); unchanged
  text is copied byte for byte, so spans stay exact;
- `JSR(R)`, `JSR(R, n, in...)` call the routine (inputs copied to its SBR tags);
  `RET()` and `TND()` return; `SBR(...)` is removed; `EVENT(Task)`;
  `SIZE(array, dim, dest)` becomes a constant; `DEG`/`RAD`;
- `Aoi(tag, args...)` becomes the FB call with the required parameters
  (EnableIn TRUE, "Structured text instructions execute as if EnableIn is always
  set");
- `[:=]` (non-retentive assignment) assigns like `:=`, and its target (a BOOL
  or number) is also reset to zero by the prescan: "the tag ... is reset to
  zero each time the controller enters the Run mode" (RM003 "Specify a
  non-retentive assignment");
- `SWPB`, `COP`/`CPS`/`FLL`, the string instructions and `GSV`/`SSV`/`MSG` work
  as in ladder.

After parsing, a pass over the AST adds the Logix numeric rules IEC does not
have: integer `/` and `MOD` go through the DIV/MOD definitions above ("the DIV
instruction and the operator '/'" share one definition, so dividing by zero
yields Source A, not a trap), integer operands are promoted to at least DINT
("optimal data type DINT"), and a REAL assigned to an integer rounds half to
even. ST does not update the math status flags.

## Reading the generated ST

`plcc convert plant.L5X --to st` prints the Structured Text a project lowers
to (`--prelude` appends the Logix prelude, so the file compiles on its own:
`plcc compile plant.st -o plant.o`). The shape, so it can be read:

- **Controller tags** are one `VAR_GLOBAL` block; each **program** is a
  `FUNCTION_BLOCK lx__P_<Program>` holding its program tags, with one
  `METHOD R_<Routine>` per routine and a `lx__prescan` method (the prescan
  actions, above); a `PROGRAM lx__run_<Program>` and a `CONFIGURATION`
  schedule them like the L5X tasks.
- **Rungs** are separated by a comment with the rung number, its
  documentation and its neutral text, then the rung-condition sequence
  described under "Ladder (RLL) routines":

  ```iec
  (* rung 0: Seal-in: Start latches the motor through its own contact, Stop breaks it.
     [XIC(Start) ,XIC(Motor) ]XIO(Stop)OTE(Motor);
  *)
  lx__rc := TRUE;
  ...
  Motor := lx__rc;
  ```

- **Instructions** that are more than a boolean become calls into the prelude,
  named after the instruction: `lx__ton(T1, lx__rc)` / `lx__tof` / `lx__rto`
  run a TIMER with the rung condition (the TIMER structure, `.PRE .ACC .EN .TT
  .DN`, is the Logix one); `lx__ctu` / `lx__ctd` a COUNTER; `lx__res_timer`,
  `lx__res_counter`, `lx__res_control` are RES. Arithmetic results are stored
  through `lx__put_<TYPE>_i(v)` (integer value) or `lx__put_<TYPE>_r(v)`
  (floating value), which apply the Logix conversion and set `lx__S_V`,
  `lx__S_Z`, `lx__S_N` (S:V, S:Z, S:N); `lx__div_i`, `lx__mod_i`, `lx__round`,
  `lx__r2l` are the division, modulo and rounding rules of the table above.
  Status and first-scan flags are globals (`lx__S_FS`, `lx__S_V`, ...).
- **Hidden variables** start with `lx__`: `lx__rc` (rung condition),
  `lx__bs<n>` / `lx__bo<n>` (branch start / branch OR at nesting depth n),
  `lx__jmp` (a taken JMP's label number), `lx__mcr`, `lx__first`.

## Faults

Logix distinguishes minor faults (logged; the program keeps running) from major
faults (the controller stops unless a fault routine clears them). plcc follows
that split:

- **Minor faults** — overflow and division by zero. `DIV` / `/` by zero
  stores Source A (integers) or ±infinity (REAL), `MOD` by zero stores 0, and
  both set S:V and S:MINOR and count a minor fault 4/4 (RM003 DIV, MOD and
  "Math status flags"). The program keeps running. Every division in the
  lowered code and the prelude is guarded, so the CODESYS-style
  `PLCC_FAULT_DIV_BY_ZERO` that plain `.st` code raises never fires in Logix
  code.
- **Major fault 4/20, array subscript out of range** — "If an array subscript
  is too large (out of range), a major fault (type 4, code 20) generates"
  (RM003 "Math status flags", "Index through arrays"). In code lowered from an
  L5X (and the Logix prelude) an out-of-range subscript calls the runtime's
  `plcc_fault(PLCC_FAULT_ARRAY_BOUNDS, where)` (docs/process-image.md
  "Runtime faults"), with `where` = `plant.L5X:line:col: POU` pointing at the
  subscript in the rung or ST line. Structured Text inputs keep clamping, as
  CODESYS does. Instructions whose manual entry says they do not fault while
  false (OTL, OTU) only evaluate their operand on a true rung.
- There are no fault routines or `GSV(Program, ..., MajorFaultRecord)`: a
  major fault stops the plcc runtime; its handler decides what that means on
  the target (safe outputs, restart).

## Tasks

| Logix | plcc |
|---|---|
| `Type="PERIODIC" Rate="10" Priority="5"` | `TASK Name (INTERVAL := T#10000us, PRIORITY := 5)` |
| `Type="CONTINUOUS"` | its programs have no `WITH`: plcc's free-running `__background` task, lowest priority — the continuous task's place in Logix |
| `Type="EVENT"`, trigger *EVENT instruction only* | `TASK Name (SINGLE := lx__event_Name)`: `EVENT(Name)` sets the flag, the task clears it when it runs, so each EVENT is a new rising edge |
| other event triggers (module input change, consumed tag, axis...) | warning; the task runs only on EVENT instructions |
| `InhibitTask="true"`, `Disabled="true"` programs, unscheduled programs | not run (warning) |

Logix priorities (1 = most urgent) are used as IEC priorities unchanged (0 = most
urgent). Each scheduled program is one instance of a runner
`PROGRAM lx__run_<name>`, which copies mapped inputs, runs the program and copies
mapped outputs. Without a `<Tasks>` element (component exports), plcc's implicit
task runs every program.

## I/O mapping (`--io-map`)

On a Logix controller, I/O is in module tags (`Local:1:I.Data`) that the
controller refreshes asynchronously; on plcc hardware it is in the `%I` / `%Q` /
`%M` process image (docs/process-image.md). `--io-map` binds the two with a TOML
table from a Logix tag path to a direct address:

```toml
"Local:1:I.Data.0" = "%IX0.0"   # I1: start push button (alias StartPB)
"Local:1:I.Data.1" = "%IX0.1"   # I2: stop push button  (alias StopPB)
"Local:2:O.Data.0" = "%QX0.0"   # relay 1: motor contactor

[io]                            # optional section, same meaning
"Level" = "%IW2"                # a controller INT tag: I2 analog, raw 0..4095
"High"  = "%QX0.4"              # USER LED
```

Any tag path works: module tag members and bits, controller tags, aliases,
`Program:X.Tag`. The address size must fit the type (`X` BOOL, `B` SINT/USINT,
`W` INT/UINT, `D` DINT/UDINT/REAL, `L` LINT/ULINT/LREAL). Each entry becomes a
hidden located variable (`lx__io0 AT %IX0.0 : BOOL`); `%I` entries are copied into
the tag before each program runs, `%Q` entries from the tag after it, `%M`
entries both ways (Modbus registers on the Opta). Copying at program boundaries
is one of the orders Logix's asynchronous I/O update allows.

`tests/fixtures/l5x/opta_io.L5X` with `opta_io.toml` is a CompactLogix seal-in on
local 1769 modules, run on the Opta generic runtime's map:

```bash
plcc compile tests/fixtures/l5x/opta_io.L5X --io-map tests/fixtures/l5x/opta_io.toml \
    -o runtime/plc.o --target thumbv7em-none-eabi --emit-header runtime/plc.h \
    --image-size I=18 --image-size Q=1 --image-size M=64
arduino-cli compile -b arduino:mbed_opta:opta \
    --build-property "compiler.c.elf.extra_flags=$PWD/runtime/plc.o" runtime
```

(`~/opta_plcc/build.sh` forwards only its first argument to plcc; run the two
steps by hand, or add `--io-map` to its plcc line.)

## Measured on public L5X exports

The corpus is every `.L5X` in 59 public GitHub repositories, cloned (depth 1)
into `tests/external/l5x/<owner>__<repo>/` (gitignored; test input only, nothing
from it is in this repository). Byte-identical files are counted once:
**1,275 hand-made exports**, plus **4,085 tool-generated** controller exports in
`jholm90/logixMemoryMap/samples/generated`, of which every 10th (409) is
measured. Each file is run through `plcc check` and then
`plcc compile -o x.o` (host target) on its own, under a 4 GiB cap. "Compiles"
means both succeed.

| Export (`TargetType`) | Files | Compile | % |
|---|---:|---:|---:|
| Controller (whole project) | 173 | 89 | 51 |
| Add-On Instruction | 384 | 319 | 83 |
| Program | 112 | 30 | 27 |
| Routine | 168 | 42 | 25 |
| Rung | 12 | 7 | 58 |
| DataType, Module, Tag, Trend | 416 | 416 | 100 |
| other (no `TargetType`) | 10 | 7 | 70 |
| **hand-made, all** | **1,275** | **911** | **71** |
| generated controllers (sample) | 409 | 375 | 92 |

At the start of this round the same corpus gave 892 hand-made (70 %) and 375
generated; the fixes since are the FIFO/LIFO instructions, SWPB, MODULE
references, extra JSR inputs, AOI alias members in decorated data, FB-instance
initializers (a codegen bug that dropped every AOI tag's initial data),
indirect-bit assignment in ST and `\Program.Tag` operands.

What the remaining failures are (hand-made, first diagnostic per file):

| Cause | Files |
|---|---:|
| a tag the file does not contain (mostly Routine/Program/Rung exports that reference controller tags) | 86 |
| FBD routine (82), SFC routine (8), FBD routine in an AOI (6) | 96 |
| a type plcc cannot model: motion (`MOTION_INSTRUCTION`, `AXIS_*`, `COORDINATE_SYSTEM`), `ALARM_*`, module-defined types without data, UDTs or string types missing from the export | 75 |
| an instruction plcc does not implement (motion MAM/MCS/MCLM/..., PID, FAL, FSC, AVE, SRT, ACL, BTDT, ...) | 25 |
| COP/FLL between unlike structures (byte reinterpretation) | 11 |
| source-protected routines (4), malformed XML (5) | 9 |
| everything else (one to seven files each) | 62 |

The generated sample's 34 failures: 14 exports that declare a data type twice
(invalid input), 12 unimplemented instructions (motion), 4 motion types, 4
other.

Repositories:
[adrians-cci/bench-test](https://github.com/adrians-cci/bench-test),
[alairjunior/l5x2c](https://github.com/alairjunior/l5x2c),
[alex-controlx/rslogix5000-aois](https://github.com/alex-controlx/rslogix5000-aois),
[atmassey/LogixAOIs](https://github.com/atmassey/LogixAOIs),
[cmseaton42/Allen-Bradley-Toolkit](https://github.com/cmseaton42/Allen-Bradley-Toolkit),
[Colt-H/L5X-Creator](https://github.com/Colt-H/L5X-Creator),
[ControlZebra/ladder-visualizer](https://github.com/ControlZebra/ladder-visualizer),
[CorbinFerguson/ThesisBuildTool](https://github.com/CorbinFerguson/ThesisBuildTool),
[daniel-SCAU/plckodetest](https://github.com/daniel-SCAU/plckodetest),
[danielCamiloP/TecnomecatroniX](https://github.com/danielCamiloP/TecnomecatroniX),
[danomagnum/gologix](https://github.com/danomagnum/gologix),
[davidjrb/paintbooth](https://github.com/davidjrb/paintbooth),
[Desunovu/project-graveyard](https://github.com/Desunovu/project-graveyard),
[Dkell88/QCA-Toolset](https://github.com/Dkell88/QCA-Toolset),
[dmroeder/panelview_running_file](https://github.com/dmroeder/panelview_running_file),
[drbitboy/plc_rng](https://github.com/drbitboy/plc_rng),
[ElektrikleEnjinear/AB_CTRL_LGX_AOIs](https://github.com/ElektrikleEnjinear/AB_CTRL_LGX_AOIs),
[etymology/dune-monorepo](https://github.com/etymology/dune-monorepo),
[fanno/l5x-emulator](https://github.com/fanno/l5x-emulator),
[Festo-North-America/FMCP](https://github.com/Festo-North-America/FMCP),
[Gaskony-Ignition/module-plc-emulator](https://github.com/Gaskony-Ignition/module-plc-emulator),
[ghanemja/senior-design](https://github.com/ghanemja/senior-design),
[GTMichelli-Dev/northwest-grain-growers](https://github.com/GTMichelli-Dev/northwest-grain-growers),
[gwbischof/VacuumGroups](https://github.com/gwbischof/VacuumGroups),
[iotrustlab/miniwatertreatment](https://github.com/iotrustlab/miniwatertreatment),
[iroxusux/Pyrox](https://github.com/iroxusux/Pyrox),
[jbcre8iv/LogixWeave](https://github.com/jbcre8iv/LogixWeave),
[JeremyMedders/LogixLibraries](https://github.com/JeremyMedders/LogixLibraries),
[jholm90/logixMemoryMap](https://github.com/jholm90/logixMemoryMap),
[jmorit/l5x_test](https://github.com/jmorit/l5x_test),
[joyautomation/nautilus](https://github.com/joyautomation/nautilus),
[JPoirier55/aois](https://github.com/JPoirier55/aois),
[juflunaca/Tile-Tech](https://github.com/juflunaca/Tile-Tech),
[kyle-goodwin/ra-oop](https://github.com/kyle-goodwin/ra-oop),
[legonigel/l5x](https://github.com/legonigel/l5x),
[meccomarking/PubEtherMark](https://github.com/meccomarking/PubEtherMark),
[Mikecranesync/MIRA](https://github.com/Mikecranesync/MIRA),
[nickytoothiccy/_LazyTool](https://github.com/nickytoothiccy/_LazyTool),
[NigoroJr/hashigo](https://github.com/NigoroJr/hashigo),
[patrickjmcd/plc-common-objects](https://github.com/patrickjmcd/plc-common-objects),
[Pdk1001/pch01](https://github.com/Pdk1001/pch01),
[petem903/studio5000-learning](https://github.com/petem903/studio5000-learning),
[qiaolin1998/Code-Builder](https://github.com/qiaolin1998/Code-Builder),
[rapter-xx/l5x-analyzer](https://github.com/rapter-xx/l5x-analyzer),
[reh3376/acd-l5x-tool-lib](https://github.com/reh3376/acd-l5x-tool-lib),
[reh3376/plc-gbt](https://github.com/reh3376/plc-gbt),
[RickyRick89/ProjectPolyglot](https://github.com/RickyRick89/ProjectPolyglot),
[rmsems/Transcat-L33ER](https://github.com/rmsems/Transcat-L33ER),
[RockwellAutomation/ra-logix-cicd](https://github.com/RockwellAutomation/ra-logix-cicd),
[RockwellAutomation/ra-logix-designer-vcs-custom-tools](https://github.com/RockwellAutomation/ra-logix-designer-vcs-custom-tools),
[shahrul-amin/Automated-Precision-Assembly-and-Inspection-Station](https://github.com/shahrul-amin/Automated-Precision-Assembly-and-Inspection-Station),
[shashankpatil-dev/plc-rag](https://github.com/shashankpatil-dev/plc-rag),
[Techno11/KetteringFRCCRobotCode](https://github.com/Techno11/KetteringFRCCRobotCode),
[teddy19991002/AB_PLC_SIMULATIONS](https://github.com/teddy19991002/AB_PLC_SIMULATIONS),
[teddy19991002/Robust-Control-and-Simulation](https://github.com/teddy19991002/Robust-Control-and-Simulation),
[tnunnink/L5Sharp](https://github.com/tnunnink/L5Sharp),
[tomha85/devagent](https://github.com/tomha85/devagent),
[W-P-I/_WPI-FunctionBlocks](https://github.com/W-P-I/_WPI-FunctionBlocks),
[zwood16/py_l5x](https://github.com/zwood16/py_l5x).

On the Arduino Opta: `tests/fixtures/l5x/opta_io.L5X` with `opta_io.toml`
compiles for `thumbv7em-none-eabi` and links into the generic Opta runtime with
`arduino-cli compile -b arduino:mbed_opta:opta` (174 KB flash, 60 KB RAM for the
whole sketch).

## Limitations

- **FBD and SFC routines** (in programs and AOIs) are reported as not supported;
  a project using them does not compile. Source-protected (`<EncodedData>`)
  routines cannot be compiled at all.
- **Not implemented instructions**: PID and the FBD process instructions
  (PIDE, ...), the file/array instructions FAL, FSC, AVE, SRT, STD, FBC/DDT,
  sequencers (SQI/SQO/SQL), ASCII serial (ARD, AWT, ACL, ...), motion (MAM,
  MSO, MCS, ...), alarms (ALMD, ALMA), BTDT, MVMT. Each is a diagnostic at its
  rung.
- **Controller objects**: GSV/SSV compile with a warning and do nothing; MSG
  never completes. `S:MINOR` is never cleared by plcc; there is no fault log,
  no fault routine, no `MajorFaultRecord`.
- **Types**: motion axes and groups, `ALARM_*`, `COORDINATE_SYSTEM`, and module
  types whose export carries no decorated data are left out (an error where
  used). A UDT or string type the export does not include cannot be guessed.
- **Byte-level reinterpretation**: COP/CPS between unlike structures (Logix
  copies raw bytes) and FLL of a structure are errors; plcc's memory layout
  (one byte per BOOL member, no hidden `SINT` hosts) is not Logix's.
- **Partial exports**: a Routine or Rung export usually references tags that
  are not in the file; those are "unknown tag" errors. Program exports with
  their tags compile when the controller tags they use are included.
- Bit index out of range in an indirect bit `tag.[n]` is ignored (Logix: major
  fault 4/20).
- `.L5K` (the older text export) is not read.

## Tests

- Fixtures written for plcc: `tests/fixtures/l5x/` — seal-in; TON/TOF/RTO,
  CTU/CTD/RES, counter overflow; one-shots, latches, nested branches, compares,
  LIM, MEQ, S:FS; math and conversions with status flags; UDTs, AOI, aliases,
  JSR with parameters and return values, RET, JMP/LBL, MCR, TND, COP, FLL;
  BSL/BSR, FOR/BRK; FFL/FFU/LFL/LFU and SWPB; the out-of-range subscript
  fault; tasks (continuous, periodic, event, inhibited); ST routines; strings;
  the I/O map; broken files under `errors/`.
- `crates/plcc-l5x/tests/` lowers each fixture, compiles it with the Logix
  prelude and the bundled stdlib, JIT-runs it through `plcc_get_app()` scan by
  scan with a fake clock, and checks tags by name through the runtime contract.
- `crates/plcc-cli/tests/l5x_inputs.rs` runs the binary: `check`, `--dump-st`, a
  Cortex-M object with `--emit-header`, error rendering.
