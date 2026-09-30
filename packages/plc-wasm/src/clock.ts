// SPDX-License-Identifier: MPL-2.0

/**
 * The PLC's one time source (`plcc_monotonic_ns`, docs/runtime-symbols.md):
 * nanoseconds since an arbitrary fixed epoch, never decreasing. TON/TOF/TP and
 * the scan scheduler read it.
 */
export interface Clock {
  nowNs(): bigint;
}

/** A clock the caller moves by hand, for tests and single-stepping. */
export class FakeClock implements Clock {
  private ns: bigint;

  constructor(startNs: bigint = 0n) {
    this.ns = startNs;
  }

  nowNs(): bigint {
    return this.ns;
  }

  /** Move forward by `ms` milliseconds (fractions allowed). */
  advanceMs(ms: number): void {
    this.advanceNs(BigInt(Math.round(ms * 1e6)));
  }

  advanceNs(ns: bigint): void {
    if (ns < 0n) throw new RangeError("a monotonic clock cannot go backwards");
    this.ns += ns;
  }
}

/** Wall time from `performance.now()` (available in windows and workers). */
export class RealClock implements Clock {
  private readonly origin = performance.now();

  nowNs(): bigint {
    return BigInt(Math.round((performance.now() - this.origin) * 1e6));
  }
}
