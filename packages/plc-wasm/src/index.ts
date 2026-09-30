// SPDX-License-Identifier: MPL-2.0
//
// Runs plcc's wasm32 output: supplies its imports, drives plcc_init /
// plcc_run_task per the runtime contract (docs/process-image.md), and exposes
// the %I/%Q/%M process image and tags by symbol offset.

export { type Clock, FakeClock, RealClock } from "./clock";
export {
  PlcFault,
  FAULT_TRAP,
  FAULT_DIV_BY_ZERO,
  FAULT_ARRAY_BOUNDS,
  FAULT_NULL_REFERENCE,
  describeFault,
  libm,
  buildImports,
} from "./imports";
export { ABI_VERSION, type Area, PlcModule, type PlcOptions, type TaskInfo } from "./plc";
export { Runner, ScanCycle, type PlcState, type ScanHooks } from "./scan";
export { type SymbolTable, type SymbolVariable, TagCodec, type TagValue } from "./tags";
