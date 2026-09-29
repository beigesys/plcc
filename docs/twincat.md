<!-- SPDX-License-Identifier: MPL-2.0 -->

# Beckhoff TwinCAT 3 projects

plcc compiles TwinCAT 3 PLC projects as they sit in a repository: the
`.plcproj` and its `.TcPOU` / `.TcDUT` / `.TcGVL` / `.TcIO` / `.TcTTO`
objects. The `plcc-twincat` crate reassembles each object's CDATA pieces
into ST text, parses it with the ordinary ST parser and maps every span back
into the XML file, so diagnostics point at the line and column inside the
`.TcPOU`. Type checking, codegen, the runtime contract, the C header and the
simulator then work as for `.st` files.

```bash
plcc check   MyPlc/MyPlc.plcproj                 # parse + type check
plcc check   MyPlc/                              # the one .plcproj below the directory
plcc compile MyPlc/MyPlc.plcproj -o plc.o --target thumbv7em-none-eabi --emit-header plc.h
plcc sim     MyPlc/MyPlc.plcproj --scans 100     # JIT-run the task configuration
plcc parse   MyPlc/POUs/MAIN.TcPOU --dump-ast    # one object
plcc compile App/App.plcproj TcLib/TcLib.plcproj -o plc.o   # app + an open-source library
```

Where a behaviour is a judgment call plcc does what CODESYS/TwinCAT does; the
decisions are recorded in [codesys-compatibility.md](codesys-compatibility.md).

## Inputs

| Input | Stands for |
|---|---|
| `.plcproj` | its `<Compile Include="...">` items with a TwinCAT object extension, in project order. Items with `<ExcludeFromBuild>true</ExcludeFromBuild>`, or in a `<Folder>` that has it, are left out (the deepest setting wins). Paths match without regard to case, as on Windows. Visualizations, text lists and image pools hold no code and are skipped. |
| directory | the one `.plcproj` below it; with none, every object file below it (sorted). Several projects in one tree is an error that lists them: name the one to build. `_Boot`, `_CompileInfo`, `_Libraries` are never read. |
| `.TcPOU` | PROGRAM, FUNCTION_BLOCK or FUNCTION, with its nested `<Method>`, `<Property>` (`<Get>` / `<Set>`) and `<Action>` elements |
| `.TcDUT` | `TYPE ... END_TYPE` (STRUCT, STRUCT EXTENDS, ENUM with a base type and attributes, UNION, alias) |
| `.TcGVL` | a global variable list; its name qualifies its variables (`GVL_Main.nScans`), parameter lists included |
| `.TcIO` | an INTERFACE with method and property prototypes, EXTENDS |
| `.TcTTO` | a task: cycle time, priority, the programs it calls |

`.st` files and PLCopen XML mix freely with TwinCAT inputs.

### Libraries

A `.plcproj` names its libraries (`<PlaceholderReference>`, `<LibraryReference>`)
but holds none of their code. plcc provides:

- the IEC standard library, which covers **Tc2_Standard** (TON, TOF, TP,
  CTU, CTD, CTUD, R_TRIG, F_TRIG, SR, RS, and the string functions);
  `Tc2_Standard.TON` and a bare `TON` are the same block.
- nothing else from Beckhoff: **Tc2_System, Tc2_Utilities, Tc2_MC2,
  Tc2_EtherCAT, Tc3_EventLogger, Tc3_DynamicMemory, ...** wrap TwinCAT
  runtime services (ADS, file I/O, NC axes, EtherCAT). A name from one of them
  is a type-check error that names the library, at the use:

  ```
  × `ADSLOGSTR` is part of the Tc2_System library, which plcc does not provide
  ```

  and a failing check ends with a note listing the libraries the project
  references that nothing provided.

An **open-source library** (TcUnit, TcOpen's TcoCore, ...) compiles from
source: pass its `.plcproj` after the application's. Every project after the
first is a library: its tasks are ignored, and a declaration whose name an
earlier project already declares (a library's own test `MAIN`) is left out.

### Tasks

Each `.TcTTO` becomes a TASK of a generated CONFIGURATION (one RESOURCE `PLC`):
`INTERVAL` from `<CycleTime>` (microseconds), `PRIORITY` from `<Priority>`,
and a program instance named like the program for every `<PouCall>`, so
`GVL`s, other POUs and the task all reach the one instance of `MAIN`, as in
TwinCAT. A program no task calls runs only when another POU calls it. Sources
that declare their own CONFIGURATION keep it. A project without a `.TcTTO`
runs every program not called by another POU in plcc's implicit task
(`--task-interval`, default `T#20ms`).

## Language

Everything plcc compiles from `.st` (see the [README](../README.md)), plus the
CODESYS/TwinCAT extensions real projects use:

| Construct | Notes |
|---|---|
| PROPERTY with GET / SET, in FBs, programs, CLASSes and INTERFACEs | lowered to getter/setter methods; late-bound through interfaces; `REFERENCE TO` properties; reading a SET-only or writing a GET-only property is an error |
| ACTION | a parameterless method: `inst.A()`, `A()` inside the POU |
| METHOD, PROPERTY and ACTION of a PROGRAM | `PRG.M()`, `PRG.Prop` |
| FB_init | runs once at instance initialization with the arguments written at the declaration (`buf : FB_Buffer(16);`, `THIS^`) |
| VAR_STAT, VAR_INST, VAR PERSISTENT | VAR_STAT is one variable shared by all instances; PERSISTENT is RETAIN |
| Qualified names | `GVL.x`, `Tc2_Standard.TON`, `E_Mode.Running`, `FB_Type.cConst` (a POU's VAR CONSTANT, also as an array bound or string length), in code, types, bounds and initializers |
| `{attribute ...}` pragmas | accepted anywhere; `'to_string'` is honoured, the rest ignored (`'qualified_only'` enumerators and GVLs are also reachable by their bare names, which TwinCAT rejects) |
| AND_THEN / OR_ELSE, `S=` / `R=`, `REF=`, REFERENCE TO, POINTER TO, `__ISVALIDREF` | |
| `TO_STRING` of an enumeration | the enumerator name with `{attribute 'to_string'}`, else the number |
| FOR with an array element or field as counter | `FOR idx[2] := 0 TO 3 DO` |
| `AT %I*` / `%Q*` | linked in the device tree, not in source: ordinary variables, reachable through the symbol table |
| LTIME up to `LTIME#213503d23h34m33s709ms551us615ns` | stored as its 64-bit pattern (see codesys-compatibility.md) |

### Not supported

Each of these is a diagnostic at the construct, never a silent miscompile.

| Construct | Status |
|---|---|
| LD / FBD (`<NWL>`), CFC, SFC bodies | "the body of function block `X` is written in LD/FBD (network list), which is not yet supported". Export the POU as PLCopen XML, which plcc compiles ([ladder.md](ladder.md)). SFC transitions are skipped with a warning. |
| Beckhoff libraries (Tc2_System, Tc2_Utilities, Tc2_MC2, Tc2_EtherCAT, Tc3_*) | reported by library, see above |
| Variable-length arrays, `ARRAY[*]`, `LOWER_BOUND` / `UPPER_BOUND` | parsed; "not supported yet" |
| `ANY` parameters (`.pValue`, `.diSize`, `.TypeClass`), `__SYSTEM.TYPE_CLASS` | undefined |
| `__NEW` / `__DELETE` | "dynamic memory is not supported" |
| `SIZEOF` in a constant expression (an array bound) | "not a constant integer expression" |
| An FB that extends a library FB of the same name (`TcoContext EXTENDS TcoCore.TcoContext`) | undefined base |
| `__QUERYINTERFACE`, `__QUERYPOINTER` | unknown function |
| `.TcTTO` tasks with non-ST names, several tasks calling one program | the first task that calls a program owns it |

## Measured on real projects

Test inputs only: fetched into `tests/external/twincat/` (gitignored), never
copied into the repository.

| Repository | License |
|---|---|
| https://github.com/TcOpenGroup/TcOpen | MIT |
| https://github.com/tcunit/TcUnit | MIT |
| https://github.com/bengeisler/TcLog | MIT |
| https://github.com/BurksEngineering/TcMatrix | MIT |
| https://github.com/Roald87/TcError | MIT |
| https://github.com/Roald87/TwincatTutorials | MIT |
| https://github.com/fisothemes/TwinCat-Dynamic-Collections | MIT |
| https://github.com/FellowWithLaptop/FB_CaseStateMachine | CC0-1.0 |
| https://github.com/pcdshub/lcls-twincat-motion (with its submodule https://github.com/pcdshub/tc_mca_std_lib) | SLAC, BSD-style |

```bash
mkdir -p tests/external/twincat && cd tests/external/twincat
git clone --depth 1 https://github.com/TcOpenGroup/TcOpen.git TcOpenGroup_TcOpen
git clone --depth 1 https://github.com/tcunit/TcUnit.git tcunit_TcUnit
# ... one directory per repository above
git clone --depth 1 https://github.com/pcdshub/lcls-twincat-motion.git pcdshub_lcls-twincat-motion
git clone --depth 1 https://github.com/pcdshub/tc_mca_std_lib.git \
    pcdshub_lcls-twincat-motion/lcls-twincat-motion/Library/tc_mca_std_lib
```

61 `.plcproj` projects, 1786 object files. Each project was checked with the
in-corpus library projects it references passed after it (TcOpen's TcoCore
for every Tco* project, TcUnit for its users).

| | Before (first measurement) | Now |
|---|---|---|
| Object files that parse | 1726 / 1738 (99.3%) | 1785 / 1786 (99.9%)¹ |
| Projects that parse | 57 / 61 | **61 / 61** |
| Projects that type-check and compile | 4 / 61 | **5 / 61** (7 without their library dependencies²) |
| Projects stopped only by Beckhoff libraries | 4 | **44** |
| Projects stopped by other errors (with or without Beckhoff libraries) | 57 (50 of them parse errors, most in a shared library) | 12 |

¹ The one file that does not parse is TcOpen's unfinished `TcoTaskResult`,
which its project excludes from the build (and plcc now does too).
² TcoAbstractions and its tests compile alone; with TcoCore, which they
reference, they stop at TcoCore's Beckhoff libraries.

Every project that fails stops at a diagnostic naming the cause; the 44 need
nothing but Tc2_System / Tc2_Utilities / Tc2_EtherCAT / Tc2_MC2 /
Tc3_EventLogger. The 12 others additionally use variable-length arrays
(TcUnit, TcMatrix, TcLog, TcError and lcls-twincat-motion through TcUnit), `ANY`
with `__SYSTEM.TYPE_CLASS` (DynamicCollections), an FB extending its library
namesake (TcoRexrothPress), or constants from LCLS libraries not in the
corpus.

The projects that compile (Roald87's tutorials: the oven digital twin and its
simulation PLC, page-fault prevention, units; TcOpen's Tc.Prober tests) are
compiled to native objects with `plcc compile`.

## Tests

- `crates/plcc-twincat/tests/objects.rs`: every object kind, spans inside the
  XML, graphical bodies reported, project file order and exclusions.
- `crates/plcc-twincat/tests/execution.rs`: the `tests/fixtures/twincat/Demo`
  project (an FB with a method, properties and an action, an interface with a
  property, a GVL, a struct and an enum DUT, `Tc2_Standard.TON`, a program
  with a method, property and action, a POU constant by name, an excluded
  draft folder, the `.TcTTO` task) type-checks, compiles and runs through the
  runtime contract.
- `crates/plcc-cli/tests/twincat_inputs.rs`: the CLI on projects,
  directories, single objects, LD bodies and syntax errors.
