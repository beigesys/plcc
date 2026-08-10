# Roadmap

Two tracks. Compiler work is what makes programs run correctly; ecosystem work is what
makes the compiler usable by someone who is not its author. Compiler items are ordered
by measured impact, ecosystem items by what each one unblocks.

## Compiler

Counts come from `docs/oscat-conformance.md` — 559 real files, compiled one at a time,
currently 299 of them successfully. Anything without a number is a gap read off the
source, not measured.

1. **Pointer arithmetic** (`pt := pt + 1`). 23 files, and a decision before it is code:
   does `+ 1` advance one byte or one element? The readings agree for `POINTER TO BYTE`
   and disagree everywhere else, so guessing miscomputes addresses in silence. Write the
   answer into `docs/codesys-compatibility.md` first.
2. **String literals with storage.** `s := 'ready';` compiles to nothing at all today —
   no value, and an assignment that drops a valueless right-hand side without a word.
   16 OSCAT files, and two bugs one line apart.
3. **A valueless right-hand side must be a diagnostic.** The other half of the line
   above. Every silent `Ok(None)` drop this project has hit came from a caller treating
   "no value" as "nothing to do"; the lvalue path was audited for it, the rvalue path
   was not.
4. **String builtins nested in expressions** — `FIND(s, 'x')` inside an `IF`. 10 files.
   Needs a temporary for the result to live in, which is the same machinery item 2 wants.
5. **CODESYS bit access on a scalar** (`X.0 := A0`). 11 files. A dialect decision — see
   `docs/codesys-compatibility.md`.

After those, in no particular order:

- **Date and time-of-day literals compile to `0`.** Parsed, typed, and silently wrong at
  runtime. The DATE conversions are deliberately left unimplemented so nothing dresses
  the placeholder up as working.
- **Cross-file name resolution**: `plcc compile A.st B.st` fails where `B.st A.st`
  succeeds for one OSCAT pair, and a synthetic reproduction does not reproduce it.
  Compiling files together is the way past the largest failure bucket, so this matters.
- **Direct representation** (`%IX0.0`, `%QW4`, `%MD8`) — parsed, never lowered. Needs the
  process-image layout from `plcc-hal` to mean anything.
- The rest of the declared-but-not-lowered surface: `CONFIGURATION` / `RESOURCE` / `TASK`,
  `INTERFACE` codegen, `PROPERTY`, `THIS^` / `SUPER^`, `REFERENCE TO`.

## Ecosystem

Reordered so each item can reuse the one above it. Building the editor plugin first
means writing a formatter and a diagnostic pipeline twice.

1. **Formatter** — its own crate over the existing AST and spans. Configurable
   alignment and indentation. The LSP and the VS Code plugin are both clients of it,
   so it lands before either.
2. **Language server** — diagnostics first, from the type checker that already exists;
   hover, go-to-definition and completion after. Wraps the formatter for
   format-on-save.
3. **VS Code plugin** — a TextMate grammar for highlighting plus an LSP client. Thin,
   once 1 and 2 exist; a rewrite if it goes first.
4. **Test runner** (`plcc test`) — the open question is what an assertion looks like in
   ST, not how to run one. Needs a decision before it needs code.
5. **Simulator** (`plcc sim`) — scan loop over the JIT that the codegen tests already
   drive, plus the `plcc-hal` process image. The runner and the online editor both
   want it.
6. **Ladder → ST transpiler** — blocked on choosing an input format. PLCopen XML is
   the only interchange anyone actually exports; a bespoke format means no source of
   real test cases.
7. **Online editor** — ladder and ST, running the simulator in the browser. Needs the
   compiler built for `wasm32`, which it already targets, and items 1, 2 and 5.
8. **HMI runtime** — a separate product that consumes this one. Parked until there is
   something to run on it.
