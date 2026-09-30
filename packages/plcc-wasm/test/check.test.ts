// SPDX-License-Identifier: MPL-2.0
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";
import { catalog, check, convert, load, parse, validatePath, version } from "../src/index";

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

  it("converts ST to ladder JSON, then to L5X and PLCopen", async () => {
    const st = "PROGRAM P VAR a : BOOL; b : BOOL; q : BOOL; END_VAR q := (a OR q) AND NOT b; END_PROGRAM";
    const j = await convert({ files: { "p.st": st }, to: "ladder-json" });
    expect(j.ok).toBe(true);
    const model = JSON.parse(j.output!);
    expect(model.dialect).toBe("iec");
    const l5x = await convert({ files: { "p.json": j.output! }, to: "l5x" });
    expect(l5x.ok).toBe(true);
    expect(l5x.output).toContain("RSLogix5000Content");
    const back = await convert({ files: { "p.L5X": l5x.output! }, to: "plcopen" });
    expect(back.ok).toBe(true);
    expect(back.diagnostics.every((d) => d.severity !== "error")).toBe(true);
    expect(back.diagnostics.every((d) => ["convert", "l5x"].includes(d.stage))).toBe(true);
  });

  it("converts an L5X project to ST with the Logix prelude", async () => {
    const r = await convert({ files: { "s.L5X": fixture("l5x/seal_in.L5X") }, to: "st", prelude: true });
    expect(r.ok).toBe(true);
    expect(r.output).toContain("Logix prelude");
  });

  it("reports conversion errors as diagnostics", async () => {
    const r = await convert({ files: { "a.st": "PROGRAM P x := ; END_PROGRAM" }, to: "ladder-json" });
    expect(r.ok).toBe(false);
    expect(r.diagnostics[0].stage).toBe("parse");
    const bad = await convert({ files: { "a.st": "" }, to: "docx" as never });
    expect(bad.diagnostics[0].message).toMatch(/to:/);
  });

  it("lists the ladder instruction catalog", async () => {
    const logix = await catalog("logix");
    const xic = logix.find((s) => s.name === "XIC")!;
    expect(xic).toMatchObject({ role: "input", category: "bit" });
    const iec = await catalog("iec");
    expect(iec.find((s) => s.name === "TON")).toMatchObject({ instance: true, role: "box" });
  });

  it("dumps one file's AST", async () => {
    const r = await parse("main.st", SEAL_IN);
    expect(r.diagnostics).toEqual([]);
    expect(JSON.stringify(r.ast)).toContain("SealIn");
  });
});
