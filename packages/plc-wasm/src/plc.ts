// SPDX-License-Identifier: MPL-2.0
//
// A plcc wasm32 module, instantiated: the runtime contract of
// docs/process-image.md read from the module's own exports (plcc_get_app →
// task table, process image), plus typed tag access from its symbol table
// (`plcc compile --emit-symbols`).

import type { Clock } from "./clock";
import { FAULT_TRAP, PlcFault, buildImports } from "./imports";
import { type SymbolTable, TagCodec, type TagValue } from "./tags";

/** Process-image area. */
export type Area = "I" | "Q" | "M";

export interface TaskInfo {
  index: number;
  name: string;
  /** 0n: not cyclic. */
  intervalNs: bigint;
  /** IEC: 0 is the highest priority. */
  priority: number;
  programCount: number;
  /** Has a SINGLE trigger. */
  hasSingle: boolean;
  programs: { name: string; programType: string }[];
}

export interface PlcOptions {
  clock: Clock;
  /** PRINT output. Default: console.log with a `[PLC]` prefix. */
  onPrint?: (message: string) => void;
  /** `plcc compile --emit-symbols` output (parsed JSON), for tag access by name. */
  symbols?: SymbolTable;
  /** Extra or replacement `env` imports. */
  imports?: Record<string, (...args: never[]) => unknown>;
}

/** ABI version this runner understands (`PLCC_ABI_VERSION`). */
export const ABI_VERSION = 1;

// wasm32 layouts of the contract structs (natural C layout, 4-byte pointers).
const APP = { abi: 0, taskCount: 4, tasks: 8, image: 12, init: 16, runTask: 20, retain: 24, retainCount: 28, retainSignature: 32 };
const TASK = { size: 32, name: 0, interval: 8, priority: 16, programCount: 20, single: 24, programs: 28 };
const PROGRAM = { size: 32, name: 0, programType: 4 };
const IMAGE = { input: 0, inputSize: 4, output: 8, outputSize: 12, memory: 16, memorySize: 20 };

interface Exports {
  memory: WebAssembly.Memory;
  plcc_init(): void;
  plcc_run_task(task: number): void;
  plcc_get_app(): number;
  __indirect_function_table?: WebAssembly.Table;
  [name: string]: unknown;
}

const decoder = new TextDecoder();

/** One instantiated PLC program. */
export class PlcModule {
  private instance!: WebAssembly.Instance;
  private ex!: Exports;
  private app = 0;
  /** Faulted: no task may run until `restart()`. */
  fault: PlcFault | null = null;
  readonly tasks: TaskInfo[] = [];
  readonly tags: TagCodec | null;
  readonly abiVersion: number = 0;

  private constructor(
    readonly module: WebAssembly.Module,
    private readonly options: PlcOptions,
  ) {
    this.tags = options.symbols ? new TagCodec(options.symbols, this) : null;
  }

  /** Compile, instantiate and cold-start (`plcc_init`) a module. */
  static async load(source: BufferSource | WebAssembly.Module, options: PlcOptions): Promise<PlcModule> {
    const module = source instanceof WebAssembly.Module ? source : await WebAssembly.compile(source);
    const plc = new PlcModule(module, options);
    plc.instantiate();
    return plc;
  }

  private instantiate(): void {
    const imports = buildImports(this.module, {
      nowNs: () => this.options.clock.nowNs(),
      print: this.options.onPrint ?? ((m) => console.log(`[PLC] ${m}`)),
      cString: (p) => this.cString(p),
      extra: this.options.imports,
    });
    this.instance = new WebAssembly.Instance(this.module, imports);
    const ex = this.instance.exports as unknown as Exports;
    for (const name of ["memory", "plcc_init", "plcc_run_task", "plcc_get_app"]) {
      if (!(name in ex)) {
        throw new Error(
          `the module does not export \`${name}\`: link it with wasm-ld --no-entry --export-dynamic ` +
            "--allow-undefined --export-table (docs/studio-wasm.md)",
        );
      }
    }
    this.ex = ex;
    this.app = ex.plcc_get_app();
    const abi = this.u32(this.app + APP.abi);
    if (abi !== ABI_VERSION) throw new Error(`the module has runtime ABI version ${abi}; this runner supports ${ABI_VERSION}`);
    (this as { abiVersion: number }).abiVersion = abi;
    this.readTasks();
    this.fault = null;
    this.guard(() => ex.plcc_init());
  }

  /**
   * Cold restart after a fault (or at any time): a fresh instance, new
   * memory, `plcc_init()`. The faulted instance's state is not trustworthy and
   * its stack pointer was not unwound, so it is discarded.
   */
  restart(): void {
    this.instantiate();
  }

  private readTasks(): void {
    const count = this.u32(this.app + APP.taskCount);
    const base = this.u32(this.app + APP.tasks);
    this.tasks.length = 0;
    for (let i = 0; i < count; i++) {
      const t = base + i * TASK.size;
      const programCount = this.u32(t + TASK.programCount);
      const progs = this.u32(t + TASK.programs);
      const programs = [];
      for (let p = 0; p < programCount; p++) {
        const at = progs + p * PROGRAM.size;
        programs.push({ name: this.cString(this.u32(at + PROGRAM.name)), programType: this.cString(this.u32(at + PROGRAM.programType)) });
      }
      const single = this.u32(t + TASK.single);
      if (single && !this.ex.__indirect_function_table) {
        throw new Error("a task has a SINGLE trigger but the module does not export its function table (link with --export-table)");
      }
      this.tasks.push({
        index: i,
        name: this.cString(this.u32(t + TASK.name)),
        intervalNs: this.view().getBigInt64(t + TASK.interval, true),
        priority: this.u32(t + TASK.priority),
        programCount,
        hasSingle: single !== 0,
        programs,
      });
    }
  }

  /** Current value of task `index`'s SINGLE trigger (false if it has none). */
  single(index: number): boolean {
    const t = this.u32(this.app + APP.tasks) + index * TASK.size;
    const fn = this.u32(t + TASK.single);
    if (!fn) return false;
    const f = this.ex.__indirect_function_table!.get(fn) as () => number;
    return this.guard(() => (f() & 0xff) !== 0) ?? false;
  }

  /**
   * Run every program instance of task `index` once. Returns false (and sets
   * `fault`) if the task faulted; throws if the PLC is already faulted.
   */
  runTask(index: number): boolean {
    if (this.fault) throw new Error(`the PLC is stopped by a fault (${this.fault.message}); restart() it`);
    this.guard(() => this.ex.plcc_run_task(index));
    return this.fault === null;
  }

  /** Run `f`, turning a PlcFault or a wasm trap into a stop. */
  private guard<T>(f: () => T): T | undefined {
    try {
      return f();
    } catch (e) {
      if (e instanceof PlcFault) this.fault = e;
      else if (e instanceof WebAssembly.RuntimeError) this.fault = new PlcFault(FAULT_TRAP, e.message);
      else throw e;
      // Outputs to their safe state (all off), as the runtime contract asks.
      this.image("Q").fill(0);
      return undefined;
    }
  }

  get memory(): WebAssembly.Memory {
    return this.ex.memory;
  }

  view(): DataView {
    return new DataView(this.ex.memory.buffer);
  }

  private u32(addr: number): number {
    return this.view().getUint32(addr, true);
  }

  /** A NUL-terminated UTF-8 string at `ptr`. */
  cString(ptr: number, max = 4096): string {
    const bytes = new Uint8Array(this.ex.memory.buffer, ptr, Math.min(max, this.ex.memory.buffer.byteLength - ptr));
    const end = bytes.indexOf(0);
    return decoder.decode(end < 0 ? bytes : bytes.subarray(0, end));
  }

  /** Address of an exported data symbol (`plcc_inst_main`, `plcc_image_i`). */
  symbolAddress(symbol: string): number {
    const g = this.ex[symbol];
    if (!(g instanceof WebAssembly.Global)) throw new Error(`the module does not export the symbol \`${symbol}\``);
    return g.value as number;
  }

  /**
   * A live view of a process-image area (`%I`, `%Q`, `%M`). Views are
   * invalidated if memory grows: take a new one per access.
   */
  image(area: Area): Uint8Array {
    const img = this.u32(this.app + APP.image);
    const [ptr, size] =
      area === "I" ? [IMAGE.input, IMAGE.inputSize] : area === "Q" ? [IMAGE.output, IMAGE.outputSize] : [IMAGE.memory, IMAGE.memorySize];
    return new Uint8Array(this.ex.memory.buffer, this.u32(img + ptr), this.u32(img + size));
  }

  /** Read one bit of an area (`%IX3.5` → readBit("I", 3, 5)). */
  readBit(area: Area, byte: number, bit: number): boolean {
    return ((this.image(area)[byte] >> bit) & 1) === 1;
  }

  writeBit(area: Area, byte: number, bit: number, value: boolean): void {
    const img = this.image(area);
    img[byte] = value ? img[byte] | (1 << bit) : img[byte] & ~(1 << bit);
  }

  /** A tag value by path (`Main.t.ET`, `SealIn.motor`); needs `symbols`. */
  read(path: string): TagValue {
    if (!this.tags) throw new Error("no symbol table: pass `symbols` to PlcModule.load");
    return this.tags.read(path);
  }

  write(path: string, value: TagValue): void {
    if (!this.tags) throw new Error("no symbol table: pass `symbols` to PlcModule.load");
    this.tags.write(path, value);
  }
}
