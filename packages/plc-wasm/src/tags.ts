// SPDX-License-Identifier: MPL-2.0
//
// Typed access to program variables by path, from the symbol table
// `plcc compile --emit-symbols` writes (docs/process-image.md, "Header and
// symbol table"): each variable is `{ path, symbol, offset, size, iec_type,
// bit }`, where `symbol` is an exported object (`plcc_inst_main`,
// `plcc_globals`, `plcc_image_q`, ...) and `offset` a byte offset into it.

export interface SymbolVariable {
  path: string;
  symbol: string;
  offset: number;
  size: number;
  iec_type: string;
  retain: boolean;
  bit: number | null;
}

/** The parts of the symbols JSON this runner uses (it has more). */
export interface SymbolTable {
  abi_version: number;
  target: string;
  pointer_size: number;
  byte_order: "little" | "big";
  variables: SymbolVariable[];
  image: Record<"I" | "Q" | "M", { size: number; symbol: string }>;
  tasks: { name: string; interval_ns: number; priority: number; single: boolean; instances: string[] }[];
  [key: string]: unknown;
}

/** BOOL → boolean; ≤ 32-bit numbers → number; 64-bit (LINT, TIME, …) → bigint; strings → string. */
export type TagValue = boolean | number | bigint | string;

interface Host {
  view(): DataView;
  symbolAddress(symbol: string): number;
  memory: WebAssembly.Memory;
}

type Kind =
  | { k: "bool" }
  | { k: "int"; bytes: 1 | 2 | 4; signed: boolean }
  | { k: "big"; signed: boolean }
  | { k: "real"; bytes: 4 | 8 }
  | { k: "string"; wide: boolean };

const INT: Record<string, Kind> = {
  SINT: { k: "int", bytes: 1, signed: true },
  INT: { k: "int", bytes: 2, signed: true },
  DINT: { k: "int", bytes: 4, signed: true },
  USINT: { k: "int", bytes: 1, signed: false },
  UINT: { k: "int", bytes: 2, signed: false },
  UDINT: { k: "int", bytes: 4, signed: false },
  BYTE: { k: "int", bytes: 1, signed: false },
  WORD: { k: "int", bytes: 2, signed: false },
  DWORD: { k: "int", bytes: 4, signed: false },
  CHAR: { k: "int", bytes: 1, signed: false },
  WCHAR: { k: "int", bytes: 2, signed: false },
  LINT: { k: "big", signed: true },
  ULINT: { k: "big", signed: false },
  LWORD: { k: "big", signed: false },
  // Durations and dates are i64 nanoseconds (runtime-symbols.md).
  TIME: { k: "big", signed: true },
  LTIME: { k: "big", signed: true },
  DATE: { k: "big", signed: true },
  LDATE: { k: "big", signed: true },
  TIME_OF_DAY: { k: "big", signed: true },
  TOD: { k: "big", signed: true },
  LTOD: { k: "big", signed: true },
  DATE_AND_TIME: { k: "big", signed: true },
  DT: { k: "big", signed: true },
  LDT: { k: "big", signed: true },
  REAL: { k: "real", bytes: 4 },
  LREAL: { k: "real", bytes: 8 },
  BOOL: { k: "bool" },
};

function kindOf(v: SymbolVariable): Kind {
  const t = v.iec_type.toUpperCase();
  if (t.startsWith("STRING")) return { k: "string", wide: false };
  if (t.startsWith("WSTRING")) return { k: "string", wide: true };
  if (INT[t]) return INT[t];
  // Enumerations and other scalars of unknown name: an integer of their size.
  if (v.size === 8) return { k: "big", signed: true };
  if (v.size === 1 || v.size === 2 || v.size === 4) return { k: "int", bytes: v.size, signed: true };
  throw new Error(`\`${v.path}\` is a ${v.iec_type} (${v.size} bytes): read its members instead`);
}

export class TagCodec {
  private readonly byPath = new Map<string, SymbolVariable>();
  private readonly little: boolean;

  constructor(
    readonly symbols: SymbolTable,
    private readonly host: Host,
  ) {
    if (symbols.pointer_size !== 4 || !symbols.target.startsWith("wasm32")) {
      throw new Error(`the symbol table is for ${symbols.target}, not a wasm32 module`);
    }
    this.little = symbols.byte_order !== "big";
    for (const v of symbols.variables) this.byPath.set(v.path.toUpperCase(), v);
  }

  /** Every variable, in symbol-table order. */
  list(): SymbolVariable[] {
    return this.symbols.variables;
  }

  lookup(path: string): SymbolVariable {
    const v = this.byPath.get(path.toUpperCase());
    if (!v) throw new Error(`no variable \`${path}\` in the symbol table`);
    return v;
  }

  /** Linear-memory address of a variable. */
  address(path: string): number {
    const v = this.lookup(path);
    return this.host.symbolAddress(v.symbol) + v.offset;
  }

  read(path: string): TagValue {
    const v = this.lookup(path);
    const at = this.host.symbolAddress(v.symbol) + v.offset;
    const dv = this.host.view();
    const kind = kindOf(v);
    switch (kind.k) {
      case "bool":
        return v.bit === null ? dv.getUint8(at) !== 0 : ((dv.getUint8(at) >> v.bit) & 1) === 1;
      case "int":
        if (kind.bytes === 1) return kind.signed ? dv.getInt8(at) : dv.getUint8(at);
        if (kind.bytes === 2) return kind.signed ? dv.getInt16(at, this.little) : dv.getUint16(at, this.little);
        return kind.signed ? dv.getInt32(at, this.little) : dv.getUint32(at, this.little);
      case "big":
        return kind.signed ? dv.getBigInt64(at, this.little) : dv.getBigUint64(at, this.little);
      case "real":
        return kind.bytes === 4 ? dv.getFloat32(at, this.little) : dv.getFloat64(at, this.little);
      case "string": {
        const bytes = new Uint8Array(this.host.memory.buffer, at, v.size);
        if (!kind.wide) {
          const end = bytes.indexOf(0);
          return new TextDecoder("latin1").decode(end < 0 ? bytes : bytes.subarray(0, end));
        }
        let s = "";
        for (let i = 0; i + 1 < v.size; i += 2) {
          const c = dv.getUint16(at + i, this.little);
          if (c === 0) break;
          s += String.fromCharCode(c);
        }
        return s;
      }
    }
  }

  write(path: string, value: TagValue): void {
    const v = this.lookup(path);
    const at = this.host.symbolAddress(v.symbol) + v.offset;
    const dv = this.host.view();
    const kind = kindOf(v);
    const num = () => {
      if (typeof value === "boolean") return value ? 1 : 0;
      if (typeof value === "string") throw new TypeError(`\`${path}\` is a ${v.iec_type}, not a string`);
      return Number(value);
    };
    switch (kind.k) {
      case "bool": {
        const on = typeof value === "string" ? value !== "" && value !== "0" && value.toUpperCase() !== "FALSE" : Boolean(value);
        if (v.bit === null) dv.setUint8(at, on ? 1 : 0);
        else {
          const b = dv.getUint8(at);
          dv.setUint8(at, on ? b | (1 << v.bit) : b & ~(1 << v.bit));
        }
        return;
      }
      case "int":
        // Wraps like the PLC would (setInt* take the low bits).
        if (kind.bytes === 1) dv.setUint8(at, num() & 0xff);
        else if (kind.bytes === 2) dv.setUint16(at, num() & 0xffff, this.little);
        else dv.setUint32(at, num() >>> 0, this.little);
        return;
      case "big": {
        const big = typeof value === "bigint" ? value : BigInt(Math.trunc(num()));
        dv.setBigUint64(at, BigInt.asUintN(64, big), this.little);
        return;
      }
      case "real":
        if (kind.bytes === 4) dv.setFloat32(at, num(), this.little);
        else dv.setFloat64(at, num(), this.little);
        return;
      case "string": {
        if (typeof value !== "string") throw new TypeError(`\`${path}\` is a ${v.iec_type}; write a string`);
        const bytes = new Uint8Array(this.host.memory.buffer, at, v.size);
        bytes.fill(0);
        if (!kind.wide) {
          for (let i = 0; i < Math.min(value.length, v.size - 1); i++) bytes[i] = value.charCodeAt(i) & 0xff;
        } else {
          for (let i = 0; i < Math.min(value.length, v.size / 2 - 1); i++) dv.setUint16(at + 2 * i, value.charCodeAt(i), this.little);
        }
        return;
      }
    }
  }
}
