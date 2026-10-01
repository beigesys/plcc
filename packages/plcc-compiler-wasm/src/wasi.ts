// SPDX-License-Identifier: MPL-2.0
//
// The smallest WASI (preview 1) host the compiler needs: stdin is the request,
// stdout collects the response, stderr is kept for diagnostics, no files, no
// arguments beyond the program name, no environment. Everything else fails
// with ENOSYS / EBADF, which the program never needs.

const ESUCCESS = 0;
const EBADF = 8;
const ENOSYS = 52;
const ENOTCAPABLE = 76;

/** Thrown by `proc_exit`; `run` turns it into the exit code. */
export class WasiExit extends Error {
  readonly code: number;
  constructor(code: number) {
    super(`exit ${code}`);
    this.code = code;
  }
}

export interface WasiRun {
  exitCode: number;
  stdout: Uint8Array;
  stderr: string;
}

function concat(chunks: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

/**
 * Instantiate `module` with a fresh WASI host, run `_start`, and return what
 * it wrote. A trap (a Rust panic aborts) is rethrown with stderr attached.
 */
export async function runWasi(module: WebAssembly.Module, stdin: Uint8Array, args: string[] = ["plcc-compiler"]): Promise<WasiRun> {
  let memory: WebAssembly.Memory | null = null;
  const mem = () => new DataView(memory!.buffer);
  const bytes = () => new Uint8Array(memory!.buffer);
  const out: Uint8Array[] = [];
  const err: Uint8Array[] = [];
  let inAt = 0;
  const enc = new TextEncoder();
  const argBytes = args.map((a) => enc.encode(`${a}\0`));

  const iovs = (ptr: number, len: number) => {
    const v = mem();
    const list: [number, number][] = [];
    for (let i = 0; i < len; i++) list.push([v.getUint32(ptr + i * 8, true), v.getUint32(ptr + i * 8 + 4, true)]);
    return list;
  };

  const wasi = {
    args_sizes_get(argc: number, bufSize: number) {
      mem().setUint32(argc, argBytes.length, true);
      mem().setUint32(bufSize, argBytes.reduce((n, a) => n + a.length, 0), true);
      return ESUCCESS;
    },
    args_get(argv: number, buf: number) {
      let at = buf;
      argBytes.forEach((a, i) => {
        mem().setUint32(argv + i * 4, at, true);
        bytes().set(a, at);
        at += a.length;
      });
      return ESUCCESS;
    },
    environ_sizes_get(count: number, size: number) {
      mem().setUint32(count, 0, true);
      mem().setUint32(size, 0, true);
      return ESUCCESS;
    },
    environ_get() {
      return ESUCCESS;
    },
    clock_time_get(_id: number, _precision: bigint, time: number) {
      const ns = BigInt(Math.round(performance.now() * 1e6));
      mem().setBigUint64(time, ns, true);
      return ESUCCESS;
    },
    clock_res_get(_id: number, res: number) {
      mem().setBigUint64(res, 1000n, true);
      return ESUCCESS;
    },
    random_get(buf: number, len: number) {
      // Only for hash seeds; any bytes do.
      crypto.getRandomValues(bytes().subarray(buf, buf + len));
      return ESUCCESS;
    },
    fd_write(fd: number, iov: number, iovcnt: number, nwritten: number) {
      if (fd !== 1 && fd !== 2) return EBADF;
      let n = 0;
      for (const [p, l] of iovs(iov, iovcnt)) {
        (fd === 1 ? out : err).push(bytes().slice(p, p + l));
        n += l;
      }
      mem().setUint32(nwritten, n, true);
      return ESUCCESS;
    },
    fd_read(fd: number, iov: number, iovcnt: number, nread: number) {
      if (fd !== 0) return EBADF;
      let n = 0;
      for (const [p, l] of iovs(iov, iovcnt)) {
        const chunk = stdin.subarray(inAt, inAt + l);
        bytes().set(chunk, p);
        inAt += chunk.length;
        n += chunk.length;
        if (chunk.length < l) break;
      }
      mem().setUint32(nread, n, true);
      return ESUCCESS;
    },
    fd_fdstat_get(fd: number, stat: number) {
      if (fd > 2) return EBADF;
      const v = mem();
      v.setUint8(stat, 2); // character device
      v.setUint16(stat + 2, 0, true);
      v.setBigUint64(stat + 8, 0xffffffffn, true);
      v.setBigUint64(stat + 16, 0xffffffffn, true);
      return ESUCCESS;
    },
    fd_close(fd: number) {
      return fd <= 2 ? ESUCCESS : EBADF;
    },
    fd_prestat_get() {
      return EBADF; // no preopened directories
    },
    fd_prestat_dir_name() {
      return EBADF;
    },
    fd_seek() {
      return ENOSYS;
    },
    fd_filestat_get() {
      return EBADF;
    },
    fd_pread() {
      return EBADF;
    },
    fd_readdir() {
      return EBADF;
    },
    path_open() {
      return ENOTCAPABLE;
    },
    path_filestat_get() {
      return ENOTCAPABLE;
    },
    path_create_directory() {
      return ENOTCAPABLE;
    },
    path_readlink() {
      return ENOTCAPABLE;
    },
    path_remove_directory() {
      return ENOTCAPABLE;
    },
    path_unlink_file() {
      return ENOTCAPABLE;
    },
    poll_oneoff() {
      return ENOSYS;
    },
    sched_yield() {
      return ESUCCESS;
    },
    proc_exit(code: number) {
      throw new WasiExit(code);
    },
  };
  // Anything else the module imports from WASI: refuse politely.
  const imports = WebAssembly.Module.imports(module);
  const table: Record<string, WebAssembly.ImportValue> = { ...wasi } as unknown as Record<string, WebAssembly.ImportValue>;
  const extra: Record<string, Record<string, WebAssembly.ImportValue>> = {};
  for (const i of imports) {
    if (i.kind !== "function") continue;
    if (i.module === "wasi_snapshot_preview1") {
      if (!(i.name in table)) table[i.name] = () => ENOSYS;
    } else {
      // plcc's own runtime symbols are never called by the compiler.
      (extra[i.module] ??= {})[i.name] = () => {
        throw new Error(`${i.module}.${i.name} called`);
      };
    }
  }
  const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: table, ...extra });
  memory = instance.exports.memory as WebAssembly.Memory;
  let exitCode = 0;
  try {
    (instance.exports._start as () => void)();
  } catch (e) {
    if (e instanceof WasiExit) exitCode = e.code;
    else {
      const stderr = new TextDecoder().decode(concat(err));
      const message = e instanceof Error ? e.message : String(e);
      throw new Error(`the compiler crashed (${message})${stderr ? `: ${stderr.trim()}` : ""}`);
    }
  }
  return { exitCode, stdout: concat(out), stderr: new TextDecoder().decode(concat(err)) };
}
