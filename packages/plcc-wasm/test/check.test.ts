// SPDX-License-Identifier: MPL-2.0
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";
import { check, load, parse, validatePath, version } from "../src/index";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = join(here, "../../../tests/fixtures");
const fixture = (rel: string) => readFileSync(join(fixtures, rel), "utf8");

const SEAL_IN = `PROGRAM SealIn
VAR
    start AT %IX0.0 : BOOL;
    stop  AT %IX0.1 : BOOL;
    motor AT %QX0.0 : BOOL;
    t : TON;
END_VAR
motor := (start OR motor) AND NOT stop;
t(IN := motor, PT := T#1s);
END_PROGRAM
`;

beforeAll(async () => {
  await load(readFileSync(join(here, "../pkg/plcc_wasm_bg.wasm")));
});

describe("plcc-wasm", () => {
  it("reports its version", async () => {
    expect(await version()).toMatch(/^\d+\.\d+\.\d+/);
  });

  it("checks a valid program and outlines its tags", async () => {
    const r = await check({ files: { "main.st": SEAL_IN } });
    expect(r.ok).toBe(true);
    expect(r.diagnostics).toEqual([]);
    expect(r.declarations).toBe(1);
    const motor = r.tags!.image.find((t) => t.name === "motor")!;
    expect(motor).toMatchObject({ area: "Q", byte_offset: 0, bit: 0, address: "%QX0.0", iec_type: "BOOL" });
    expect(r.tags!.tasks[0]).toMatchObject({ name: "MainTask", implicit: true });
  });

  it("locates a type error with UTF-16 columns", async () => {
    // The é before the error shifts byte offsets but not UTF-16 columns by more than one.
    const src = "PROGRAM P\nVAR x : BOOL; y : INT; z : INT; END_VAR\n(* é *) z := x + y;\nEND_PROGRAM\n";
    const r = await check({ files: { "src/p.st": src } });
    expect(r.ok).toBe(false);
    const d = r.diagnostics.find((d) => d.severity === "error")!;
    expect(d).toMatchObject({ file: "src/p.st", stage: "typecheck" });
    expect(d.span!.start.line).toBe(3);
    // The span's UTF-16 index points at the same text in the JS string.
    const at = src.slice(d.span!.start.utf16, d.span!.end.utf16);
    expect(at.length).toBeGreaterThan(0);
    expect(src.split("\n")[2].slice(d.span!.start.col - 1)).toContain(at.trim());
  });

  it("reports parse errors per file", async () => {
    const r = await check({ files: { "a.st": "PROGRAM A\nx := ;\nEND_PROGRAM\n", "b.st": SEAL_IN } });
    expect(r.ok).toBe(false);
    expect(r.diagnostics.every((d) => d.file === "a.st" && d.stage === "parse")).toBe(true);
  });

  it("rejects path traversal", async () => {
    const r = await check({ files: { "../evil.st": SEAL_IN } });
    expect(r.ok).toBe(false);
    expect(r.diagnostics[0].stage).toBe("input");
    expect(await validatePath("a/../b.st")).toMatch(/\.\./);
    expect(await validatePath("src/main.st")).toBeNull();
  });

  it("reads PLCopen XML", async () => {
    const r = await check({ files: { "ld.xml": fixture("plcopen/ld_seal_in.xml") } });
    expect(r.ok).toBe(true);
  });

  it("reads L5X with an I/O map", async () => {
    const r = await check({
      files: { "opta_io.L5X": fixture("l5x/opta_io.L5X") },
      io_map: fixture("l5x/opta_io.toml"),
    });
    expect(r.diagnostics.filter((d) => d.severity === "error")).toEqual([]);
    expect(r.tags!.image.map((t) => t.address)).toContain("%IX0.0");
  });

  it("reads a TwinCAT project from memory", async () => {
    const dir = join(fixtures, "twincat/Demo");
    const rels = [
      "Demo.plcproj", "PlcTask.TcTTO", "DUTs/E_Mode.TcDUT", "DUTs/ST_Sample.TcDUT", "POUs/MAIN.TcPOU",
      "POUs/PRG_Stats.TcPOU", "POUs/I_Counter.TcIO", "POUs/FB_Counter.TcPOU", "GVLs/GVL_Main.TcGVL",
      "POUs/Drafts/FB_Draft.TcPOU",
    ];
    const files = Object.fromEntries(rels.map((r) => [`Demo/${r}`, readFileSync(join(dir, r), "utf8")]));
    const r = await check({ files });
    expect(r.ok).toBe(true);
    expect(r.tags!.tasks[0].implicit).toBe(false);
  });

  it("returns a malformed request as a diagnostic", async () => {
    const r = await check({ files: { "a.st": SEAL_IN }, stdlib: "bogus" as never });
    expect(r.ok).toBe(false);
    expect(r.diagnostics[0].message).toMatch(/stdlib/);
  });

  it("dumps one file's AST", async () => {
    const r = await parse("main.st", SEAL_IN);
    expect(r.diagnostics).toEqual([]);
    expect(JSON.stringify(r.ast)).toContain("SealIn");
  });
});
