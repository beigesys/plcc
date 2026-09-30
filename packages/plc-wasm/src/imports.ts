// SPDX-License-Identifier: MPL-2.0
//
// What a plcc wasm32 module imports from `env` (docs/runtime-symbols.md):
//   plcc_monotonic_ns() -> i64      the clock (a BigInt in JS)
//   plcc_print(ptr)                 PRINT; a NUL-terminated string
//   plcc_fault(code: i32, ptr)      a runtime fault; must not return
// plus libm functions LLVM lowers math intrinsics to on wasm (sinf, cos, powf,
// fmod, ...), which map onto Math.

/** Thrown out of `plcc_fault`, unwinding the scan that faulted. */
export class PlcFault extends Error {
  constructor(
    /** `PLCC_FAULT_*`: 1 division by zero, 2 array bounds, 3 null reference; 0 a wasm trap. */
    readonly code: number,
    /** `"file:line:col: POU"`, or the trap's message. */
    readonly where: string,
  ) {
    super(`PLC fault ${code} (${describeFault(code)}) at ${where}`);
    this.name = "PlcFault";
  }
}

export const FAULT_TRAP = 0;
export const FAULT_DIV_BY_ZERO = 1;
export const FAULT_ARRAY_BOUNDS = 2;
export const FAULT_NULL_REFERENCE = 3;

export function describeFault(code: number): string {
  switch (code) {
    case FAULT_TRAP:
      return "trap";
    case FAULT_DIV_BY_ZERO:
      return "integer division by zero";
    case FAULT_ARRAY_BOUNDS:
      return "array subscript out of range";
    case FAULT_NULL_REFERENCE:
      return "call through an unbound reference";
    default:
      return code >= 0x10000 ? "runtime-defined fault" : "runtime fault";
  }
}

type NumFn = (...args: number[]) => number;

/** C `round`: halves away from zero (Math.round rounds -2.5 to -2). */
const cRound = (x: number) => (x < 0 ? -Math.round(-x) : Math.round(x));

/** libm (double) names → implementations. The `f` variants round to f32. */
const LIBM: Record<string, NumFn> = {
  sin: Math.sin, cos: Math.cos, tan: Math.tan,
  asin: Math.asin, acos: Math.acos, atan: Math.atan, atan2: Math.atan2,
  sinh: Math.sinh, cosh: Math.cosh, tanh: Math.tanh,
  asinh: Math.asinh, acosh: Math.acosh, atanh: Math.atanh,
  exp: Math.exp, exp2: (x) => 2 ** x, expm1: Math.expm1,
  log: Math.log, log2: Math.log2, log10: Math.log10, log1p: Math.log1p,
  pow: Math.pow, sqrt: Math.sqrt, cbrt: Math.cbrt, hypot: Math.hypot,
  fabs: Math.abs, floor: Math.floor, ceil: Math.ceil, trunc: Math.trunc,
  round: cRound, rint: roundHalfEven, nearbyint: roundHalfEven, roundeven: roundHalfEven,
  fmod: (a, b) => a % b, fmin: Math.min, fmax: Math.max,
  copysign: (a, b) => (Object.is(Math.sign(b), -0) || b < 0 ? -Math.abs(a) : Math.abs(a)),
};

function roundHalfEven(x: number): number {
  const r = Math.round(x);
  return Math.abs(x % 1) === 0.5 && r % 2 !== 0 ? r - 1 : r;
}

/** The libm implementation for an import name, if it is one. */
export function libm(name: string): NumFn | undefined {
  if (Object.hasOwn(LIBM, name)) return LIBM[name];
  if (name.endsWith("f") && Object.hasOwn(LIBM, name.slice(0, -1))) {
    const f = LIBM[name.slice(0, -1)];
    return (...a: number[]) => Math.fround(f(...a));
  }
  return undefined;
}

export interface ImportHooks {
  nowNs(): bigint;
  print(message: string): void;
  /** Read a NUL-terminated UTF-8 string from linear memory. */
  cString(ptr: number): string;
  /** Extra `env` imports, by name (override the defaults). */
  extra?: Record<string, (...args: never[]) => unknown>;
}

/**
 * Build the import object for `module`. Throws if the module imports
 * something this runner cannot supply, naming every such import.
 */
export function buildImports(module: WebAssembly.Module, hooks: ImportHooks): WebAssembly.Imports {
  const env: Record<string, unknown> = {};
  const missing: string[] = [];
  for (const imp of WebAssembly.Module.imports(module)) {
    const where = `${imp.module}.${imp.name}`;
    if (imp.module !== "env" || imp.kind !== "function") {
      missing.push(`${where} (${imp.kind})`);
      continue;
    }
    const extra = hooks.extra?.[imp.name];
    if (extra) {
      env[imp.name] = extra;
      continue;
    }
    switch (imp.name) {
      case "plcc_monotonic_ns":
        env[imp.name] = () => BigInt.asIntN(64, hooks.nowNs());
        break;
      case "plcc_print":
        env[imp.name] = (ptr: number) => hooks.print(hooks.cString(ptr));
        break;
      case "plcc_fault":
        env[imp.name] = (code: number, ptr: number) => {
          throw new PlcFault(code >>> 0, ptr ? hooks.cString(ptr) : "?");
        };
        break;
      default: {
        const f = libm(imp.name);
        if (f) env[imp.name] = f;
        else missing.push(where);
      }
    }
  }
  if (missing.length) {
    throw new Error(`the module imports what this runner does not provide: ${missing.join(", ")}`);
  }
  return { env: env as WebAssembly.ModuleImports };
}
