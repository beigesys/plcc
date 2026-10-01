// SPDX-License-Identifier: MPL-2.0
//
// plcc's front end in the browser: parse + type-check ST, PLCopen XML, L5X and
// TwinCAT projects held in memory. Typed wrapper over the wasm-bindgen module
// in ../pkg (built by ../build.sh from crates/plcc-wasm).

import init, * as raw from "../pkg/plcc_wasm.js";

export type Severity = "error" | "warning" | "advice";
export type Stage = "parse" | "plcopen" | "l5x" | "twincat" | "io-map" | "input" | "typecheck" | "convert" | "codegen";

/** 1-based line; `col` is 1-based in UTF-16 code units; `utf16` is a JS string index. */
export interface Position {
  line: number;
  col: number;
  /** Byte offset into the UTF-8 text. */
  offset: number;
  utf16: number;
}

export interface Label {
  start: Position;
  end: Position;
  message: string | null;
}

export interface Diagnostic {
  /** The caller's path; null for request-level problems. */
  file: string | null;
  severity: Severity;
  stage: Stage;
  code: string | null;
  message: string;
  help: string | null;
  /** The primary range (first label). */
  span: Label | null;
  labels: Label[];
  /**
   * For a ladder model input (`.json`): where in the model. `span` is then a
   * range in the rung's text or in the ST box's code.
   */
  ladder?: LadderRef;
}

/** A place in a ladder model (plcc-ladder ids). */
export interface LadderRef {
  pou?: string;
  routine?: string;
  rung?: number;
  /** The innermost element (an ST box for a problem in its code). */
  element?: number;
  /** Operand (pin) index of the element. */
  operand?: number;
  /** A variable (tag) the diagnostic is about. */
  tag?: string;
}

export interface ImageTag {
  path: string;
  scope: string;
  name: string;
  /** Canonical address, e.g. `%IX0.3`, `%QW1`. */
  address: string;
  area: "I" | "Q" | "M";
  size_prefix: "X" | "B" | "W" | "D" | "L";
  byte_offset: number;
  bit: number | null;
  bits: number;
  iec_type: string;
  file: string;
}

export interface VarTag {
  name: string;
  kind: string;
  iec_type: string;
  retain: boolean;
  constant: boolean;
  at: string | null;
}

export interface PouTags {
  name: string;
  kind: "program" | "function_block";
  file: string;
  variables: VarTag[];
}

export interface TaskTag {
  name: string;
  interval_ns: number | null;
  priority: number | null;
  single: string | null;
  instances: { name: string; program: string }[];
  implicit: boolean;
}

export interface Tags {
  image: ImageTag[];
  programs: PouTags[];
  function_blocks: PouTags[];
  globals: VarTag[];
  tasks: TaskTag[];
}

export interface CheckRequest {
  /** path → text. Paths are relative and `/`-separated. */
  files: Record<string, string>;
  /** Inputs in order; default: every .plcproj, else every source file. */
  entry?: string[];
  /** L5X I/O map (TOML), see docs/l5x.md. */
  io_map?: string;
  stdlib?: "bundled-st" | "none";
  /** Include the tag outline (default true). */
  tags?: boolean;
}

export interface CheckResult {
  ok: boolean;
  diagnostics: Diagnostic[];
  tags: Tags | null;
  declarations: number;
}

let ready: Promise<void> | null = null;
let poisoned: string | null = null;

/**
 * Load the module. With a bundler, call with no argument (the .wasm is fetched
 * next to the JS glue); otherwise pass a URL, Response, bytes or a
 * WebAssembly.Module. Idempotent.
 */
export function load(source?: unknown): Promise<void> {
  if (!ready) {
    ready = init(source === undefined ? undefined : { module_or_path: source as never }).then(() => undefined);
  }
  return ready;
}

/**
 * Whether an internal compiler error (a Rust panic, which aborts) has left the
 * instance unusable. wasm-bindgen keeps one instance per JS realm, so the only
 * recovery is a new realm: run this package in a Web Worker and restart the
 * worker when this is non-null.
 */
export function poisonedReason(): string | null {
  return poisoned;
}

async function guarded<T>(f: () => T, onError: (e: unknown) => T): Promise<T> {
  await (ready ?? load());
  if (poisoned) return onError(new Error(`module unusable after an earlier internal error: ${poisoned}`));
  try {
    return f();
  } catch (e) {
    poisoned = e instanceof Error ? e.message : String(e);
    return onError(e);
  }
}

function internalError(e: unknown): CheckResult {
  return {
    ok: false,
    tags: null,
    declarations: 0,
    diagnostics: [
      {
        file: null,
        severity: "error",
        stage: "input",
        code: "plcc::internal",
        message: `internal compiler error: ${e instanceof Error ? e.message : String(e)}`,
        help: "please report this with the project that triggered it; restart the worker to continue",
        span: null,
        labels: [],
      },
    ],
  };
}

/** Parse and type-check, exactly as `plcc check` does. */
export function check(req: CheckRequest): Promise<CheckResult> {
  return guarded(() => JSON.parse(raw.check(JSON.stringify(req))) as CheckResult, internalError);
}

/** One file's AST (`plcc parse --dump-ast`), or null with diagnostics. */
export function parse(path: string, text: string): Promise<{ ast: unknown; diagnostics: Diagnostic[] }> {
  return guarded(
    () => JSON.parse(raw.parse(path, text)),
    (e) => ({ ast: null, diagnostics: internalError(e).diagnostics }),
  );
}

export type ConvertFormat = "st" | "plcopen" | "l5x" | "ladder-json";
export type LadderDialect = "iec" | "logix";

export interface ConvertRequest {
  /** path → text: .st, PLCopen .xml, .L5X, a ladder model .json, TwinCAT files. */
  files: Record<string, string>;
  /** Inputs; ladder outputs take exactly one. Default: every file. */
  entry?: string[];
  to: ConvertFormat;
  /** Dialect of ladder-json / st output; plcopen is always IEC, l5x Logix. */
  dialect?: LadderDialect;
  /** L5X to ST: append the Logix prelude so the ST compiles on its own. */
  prelude?: boolean;
}

export interface ConvertResult {
  ok: boolean;
  output: string | null;
  /** Reader errors, plus translation / write warnings and ST-to-ladder notes (`stage: "convert"`). */
  diagnostics: Diagnostic[];
}

/** Convert between notations and ladder dialects, as `plcc convert` does. */
export function convert(req: ConvertRequest): Promise<ConvertResult> {
  return guarded(
    () => JSON.parse(raw.convert(JSON.stringify(req))) as ConvertResult,
    (e) => ({ ok: false, output: null, diagnostics: internalError(e).diagnostics }),
  );
}

export interface PinSpec {
  name: string;
  dir: "input" | "output" | "in_out";
}

/** One instruction of the ladder palette (`plcc_ladder::catalog::Spec`). */
export interface InstructionSpec {
  name: string;
  category:
    | "bit" | "timer" | "counter" | "edge" | "compare" | "math" | "move" | "logical"
    | "expression" | "program_control" | "file" | "string" | "other";
  /** Logix input/output instruction, or an IEC box. */
  role: "input" | "output" | "box";
  pins: PinSpec[];
  /** The last pin repeats. */
  variadic: boolean;
  power_in?: string;
  power_out?: string;
  /** IEC function block (needs an instance). */
  instance: boolean;
}

/** The ladder instruction catalog of a dialect: names, pins, roles, categories. */
export async function catalog(dialect: LadderDialect): Promise<InstructionSpec[]> {
  await (ready ?? load());
  const json = raw.catalog(dialect);
  if (json == null) throw new Error(`unknown ladder dialect ${dialect}`);
  return JSON.parse(json) as InstructionSpec[];
}

/** null if `path` is a valid project path, else the reason. */
export async function validatePath(path: string): Promise<string | null> {
  await (ready ?? load());
  return raw.validate_path(path) ?? null;
}

// ---------------------------------------------------------------- device manifests

/** A problem in a device manifest (docs/device-manifest.md). */
export interface DeviceDiagnostic {
  file: string | null;
  severity: "error" | "warning";
  message: string;
  /** Document path of the value: `io[3].address`; empty for syntax errors. */
  path: string;
  /** 1-based; `col` counts characters. */
  line: number | null;
  col: number | null;
  /** Byte range in the UTF-8 text. */
  span?: { start: number; end: number };
}

/** One I/O point of an expanded manifest. */
export interface DeviceIoPoint {
  id: string;
  terminal: string;
  label: string;
  group: string;
  dir: "in" | "out" | "mem";
  kind: "digital" | "analog" | "register";
  type: string;
  address: string;
  range?: [number, number];
  eng?: [number, number];
  units?: string;
  description?: string;
}

/**
 * An expanded manifest (`plcc_device::Device`): the sections as written plus
 * `io` with every `repeat` unrolled. The section types are those of the
 * JSON Schema (`deviceSchema()`).
 */
export interface Device {
  device: { id: string; name: string; vendor: string; description: string; version: number; schema: number; source?: string; sha256?: string };
  target: {
    triple: string;
    cpu?: string;
    features?: string[];
    float_abi?: "soft" | "softfp" | "hard";
    runtime: { kind: string; abi: number };
    image: { I: number; Q: number; M: number };
  };
  flash?: Record<string, unknown>;
  console?: { transport: "webserial"; baud: number; commands: ("info" | "img" | "mw")[]; img_format: "hex-areas"; img_m_bytes: number };
  modbus?: Record<string, unknown>;
  io: DeviceIoPoint[];
}

export interface DeviceResult {
  ok: boolean;
  /** The manifest as written (with `repeat` entries), when it parses. */
  manifest: Record<string, unknown> | null;
  /** The expansion (`loadDevice` only), when it is valid. */
  device: Device | null;
  diagnostics: DeviceDiagnostic[];
}

function deviceError(e: unknown): DeviceResult {
  return {
    ok: false,
    manifest: null,
    device: null,
    diagnostics: [
      { file: null, severity: "error", message: `internal error: ${e instanceof Error ? e.message : String(e)}`, path: "", line: null, col: null },
    ],
  };
}

/** Parse a manifest: TOML syntax, value types and unknown keys. */
export function parseDevice(text: string, file?: string): Promise<DeviceResult> {
  return guarded(() => JSON.parse(raw.device_parse(text, file)) as DeviceResult, deviceError);
}

/** Parse and validate a manifest (diagnostics with line and column). */
export function validateDevice(text: string, file?: string): Promise<DeviceResult> {
  return guarded(() => JSON.parse(raw.device_validate(text, file)) as DeviceResult, deviceError);
}

/** Parse, validate and expand a manifest. */
export function loadDevice(text: string, file?: string): Promise<DeviceResult> {
  return guarded(() => JSON.parse(raw.device_load(text, file)) as DeviceResult, deviceError);
}

/** The manifests built into plcc (Opta, Simulator): the fallback catalog. */
export async function builtinDevices(): Promise<{ file: string; text: string }[]> {
  await (ready ?? load());
  return JSON.parse(raw.device_builtin()) as { file: string; text: string }[];
}

/** The JSON Schema of the manifest format, for editors. */
export async function deviceSchema(): Promise<unknown> {
  await (ready ?? load());
  return JSON.parse(raw.device_schema());
}

/**
 * Where a fault site `file:line:col` of a program compiled from a ladder
 * model is in the model (`model` is the model's JSON; line and column are
 * the site's), or null when it is not inside a rung or ST box.
 */
export async function locateLadder(model: string, line: number, col: number): Promise<LadderRef | null> {
  await (ready ?? load());
  const r = raw.locate_ladder(model, line, col);
  return r == null ? null : (JSON.parse(r) as LadderRef);
}

export async function version(): Promise<string> {
  await (ready ?? load());
  return raw.version();
}
