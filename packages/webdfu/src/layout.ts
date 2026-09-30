// SPDX-License-Identifier: MPL-2.0
//
// DfuSe memory layout strings: the alternate setting's interface name, e.g.
//   "@Internal Flash  2MB   /0x08000000/01*128Ka,15*128Kg"
// ST UM0424 / AN3156 ("DfuSe USB device firmware upgrade"): `@` name, then one
// or more `/start/count*size[unit]type,...` regions. Unit: ' ' bytes, 'K'
// KiB, 'M' MiB. Type is a letter a..g whose value - 'a' + 1 is a bit set:
// 1 readable, 2 erasable, 4 writeable ('a' read-only, 'g' read/erase/write).

export interface Sector {
  start: number;
  size: number;
  readable: boolean;
  erasable: boolean;
  writable: boolean;
  /** The layout letter, a..g. */
  type: string;
}

export interface MemoryLayout {
  name: string;
  sectors: Sector[];
}

export class LayoutError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "LayoutError";
  }
}

const UNITS: Record<string, number> = { " ": 1, B: 1, K: 1024, M: 1024 * 1024 };

/** Parse a DfuSe layout string. Throws LayoutError on anything malformed. */
export function parseLayout(text: string): MemoryLayout {
  const s = text.trim();
  if (!s.startsWith("@")) throw new LayoutError(`not a DfuSe memory layout (no leading @): "${text}"`);
  const parts = s.slice(1).split("/");
  if (parts.length < 3 || parts.length % 2 !== 1) throw new LayoutError(`malformed DfuSe memory layout: "${text}"`);
  const name = parts[0].trim();
  const sectors: Sector[] = [];
  for (let i = 1; i < parts.length; i += 2) {
    const startText = parts[i].trim();
    if (!/^0x[0-9a-f]+$/i.test(startText)) throw new LayoutError(`bad region address "${startText}" in "${text}"`);
    let addr = parseInt(startText, 16);
    for (const group of parts[i + 1].split(",")) {
      const m = /^\s*(\d+)\*(\d+)([ BKM]?)([a-g])\s*$/.exec(group);
      if (!m) throw new LayoutError(`bad sector group "${group}" in "${text}"`);
      const count = parseInt(m[1], 10);
      const size = parseInt(m[2], 10) * UNITS[m[3] || " "];
      const bits = m[4].charCodeAt(0) - "a".charCodeAt(0) + 1;
      if (size <= 0) throw new LayoutError(`zero-sized sectors in "${text}"`);
      for (let n = 0; n < count; n++) {
        sectors.push({
          start: addr,
          size,
          readable: (bits & 1) !== 0,
          erasable: (bits & 2) !== 0,
          writable: (bits & 4) !== 0,
          type: m[4],
        });
        addr += size;
      }
    }
  }
  return { name, sectors };
}

/** The sector containing `address`, if any. */
export function sectorAt(layout: MemoryLayout, address: number): Sector | undefined {
  return layout.sectors.find((s) => address >= s.start && address < s.start + s.size);
}

export const hex = (n: number) => `0x${n.toString(16).padStart(8, "0")}`;
