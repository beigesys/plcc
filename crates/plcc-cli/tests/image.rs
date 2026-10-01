// SPDX-License-Identifier: MPL-2.0

//! `plcc image` (docs/program-image.md) on real plcc output:
//!
//! - every fixture program, at -O0 and -O2, links for the Opta, passes the
//!   loader's checks (`plcc image --info`), and — when ld.lld is installed —
//!   has the same text and .data bytes as ld.lld with an equivalent script;
//! - images RUN under `qemu-arm -cpu cortex-m7` in a harness that uses the
//!   Opta runtime's own loader checks and service table
//!   (runtimes/arduino-opta/loader/emu), when qemu-arm and arm-none-eabi-gcc
//!   are installed. Each tool-dependent part prints "skipped" without them.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

fn repo(p: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(p)
}

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("plcc_image_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn plcc(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_plcc")).args(args).output().unwrap()
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// A diagnostic with miette's line wrapping undone: it wraps at the terminal
/// width and draws a `│` gutter, so a long path can split a message in two.
fn diag(o: &Output) -> String {
    text(o).split_whitespace().filter(|w| *w != "│").collect::<Vec<_>>().join(" ")
}

fn tool(env: &str, default: &str) -> Option<PathBuf> {
    let p = std::env::var_os(env).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(default));
    Command::new(&p).arg("--version").output().ok().filter(|o| o.status.success()).map(|_| p)
}

/// Compile `src` for the Opta and link an image; returns (object, image, report).
fn build(src: &Path, opt: &str, tag: &str) -> (PathBuf, PathBuf, serde_json::Value) {
    let dir = tmp();
    let stem = format!("{}-{tag}", src.file_stem().unwrap().to_string_lossy());
    let obj = dir.join(format!("{stem}.o"));
    let img = dir.join(format!("{stem}.img"));
    let o = plcc(&["compile", src.to_str().unwrap(), "-o", obj.to_str().unwrap(), "--device", "arduino-opta", opt]);
    assert!(o.status.success(), "compile {}: {}", src.display(), text(&o));
    let o = plcc(&["image", obj.to_str().unwrap(), "--device", "arduino-opta", "-o", img.to_str().unwrap(), "--json"]);
    assert!(o.status.success(), "image {}: {}", src.display(), text(&o));
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    (obj, img, report)
}

fn fixtures() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for d in ["tests/fixtures/programs", "tests/fixtures/codegen", "crates/plcc-cli/tests/data/image"] {
        for e in std::fs::read_dir(repo(d)).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "st") {
                v.push(p);
            }
        }
    }
    v.sort();
    v
}

/// ld.lld's text and .data for `obj` at the addresses of `report`.
fn lld(obj: &Path, report: &serde_json::Value) -> Option<(Vec<u8>, Vec<u8>)> {
    let lld = tool("PLCC_LLD", "/usr/lib/llvm-21/bin/ld.lld")?;
    let objcopy = tool("PLCC_OBJCOPY", "/usr/lib/llvm-21/bin/llvm-objcopy")?;
    let h = &report["header"];
    let base = obj.with_extension("");
    let script = base.with_extension("ld");
    std::fs::write(
        &script,
        format!(
            "SECTIONS {{\n  . = {};\n  .text : {{ *(.text*) *(.rodata*) }}\n  . = {};\n  .data : AT({}) {{ *(.data*) }}\n  .bss : {{ *(.bss*) *(COMMON) }}\n  /DISCARD/ : {{ *(.ARM.exidx*) *(.ARM.extab*) *(.ARM.attributes) *(.comment) *(.note*) }}\n}}\n",
            h["textAddr"], h["dataAddr"], h["dataLoad"]
        ),
    )
    .unwrap();
    let elf = base.with_extension("lld.elf");
    let mut cmd = Command::new(lld);
    cmd.arg("-O0").arg("-T").arg(&script).arg("-o").arg(&elf).arg(obj);
    for i in report["imports"].as_array().unwrap() {
        cmd.arg(format!("--defsym={}={:#x}", i["name"].as_str().unwrap(), i["veneer"].as_u64().unwrap() | 1));
    }
    let o = cmd.output().unwrap();
    assert!(o.status.success(), "ld.lld {}: {}", obj.display(), text(&o));
    let section = |s: &str| {
        let out = base.with_extension(format!("lld{s}.bin"));
        assert!(Command::new(&objcopy).args(["-O", "binary", "-j", s]).arg(&elf).arg(&out).status().unwrap().success());
        std::fs::read(out).unwrap_or_default()
    };
    Some((section(".text"), section(".data")))
}

#[test]
fn fixtures_link_check_and_match_lld() {
    let mut compared = 0;
    let mut linked = 0;
    for src in fixtures() {
        for opt in ["-O0", "-O2"] {
            let (obj, img, report) = build(&src, opt, opt.trim_start_matches('-'));
            linked += 1;
            let o = plcc(&["image", "--info", img.to_str().unwrap(), "--device", "arduino-opta"]);
            assert!(o.status.success() && text(&o).contains("valid for arduino-opta v2"), "{}", text(&o));
            let bytes = std::fs::read(&img).unwrap();
            assert_eq!(bytes.len() as u64, report["size"].as_u64().unwrap());
            assert_eq!(report["address"], 0x0818_0000);
            // No veneer for anything but services; plcc objects import at least the clock or nothing.
            for i in report["imports"].as_array().unwrap() {
                assert!(i["index"].as_u64().unwrap() < 185);
            }
            if let Some((t, d)) = lld(&obj, &report) {
                assert_eq!(&bytes[128..128 + t.len()], &t[..], "{} {opt}: text differs from ld.lld", src.display());
                let h = &report["header"];
                let off = (h["dataLoad"].as_u64().unwrap() - 0x0818_0000) as usize;
                assert_eq!(&bytes[off..off + d.len()], &d[..], "{} {opt}: .data differs from ld.lld", src.display());
                compared += 1;
            }
        }
    }
    assert!(linked >= 40, "{linked}");
    if compared == 0 {
        eprintln!("skipped the ld.lld comparison: ld.lld/llvm-objcopy not found");
    } else {
        assert_eq!(compared, linked);
    }
}

#[test]
fn image_cli_errors() {
    let dir = tmp();
    // A device without a program slot.
    let src = repo("tests/fixtures/programs/blink.st");
    let (obj, _, _) = build(&src, "-O2", "cli");
    let out = dir.join("x.img");
    let o = plcc(&["image", obj.to_str().unwrap(), "--device", "simulator", "-o", out.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(diag(&o).contains("has no program slot"), "{}", text(&o));
    // An object for another target.
    let wasm = dir.join("blink-wasm.o");
    let o = plcc(&["compile", src.to_str().unwrap(), "-o", wasm.to_str().unwrap(), "--target", "wasm32-unknown-unknown"]);
    assert!(o.status.success(), "{}", text(&o));
    let o = plcc(&["image", wasm.to_str().unwrap(), "--device", "arduino-opta", "-o", out.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(text(&o).contains("not an ELF object"), "{}", text(&o));
    // A bad build id; a given build id lands in the header.
    let o = plcc(&["image", obj.to_str().unwrap(), "--device", "arduino-opta", "-o", out.to_str().unwrap(), "--build-id", "xyz"]);
    assert!(!o.status.success() && text(&o).contains("32 hex digits"), "{}", text(&o));
    let id = "00112233445566778899aabbccddeeff";
    let o = plcc(&["image", obj.to_str().unwrap(), "--device", "arduino-opta", "-o", out.to_str().unwrap(), "--build-id", id, "--map"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains(id) && text(&o).contains("plcc_get_app"), "{}", text(&o));
    // --info on a corrupted image.
    let mut b = std::fs::read(&out).unwrap();
    let n = b.len();
    b[n - 40] ^= 0xff;
    std::fs::write(&out, &b).unwrap();
    let o = plcc(&["image", "--info", out.to_str().unwrap(), "--device", "arduino-opta"]);
    // miette wraps long messages behind a `│` gutter.
    let t = text(&o).split_whitespace().filter(|w| *w != "│").collect::<Vec<_>>().join(" ");
    assert!(!o.status.success() && t.contains("body CRC mismatch"), "{t}");
}

/// The emulator harness binary, built once (None: tools missing).
fn harness() -> Option<&'static PathBuf> {
    static H: OnceLock<Option<PathBuf>> = OnceLock::new();
    H.get_or_init(|| {
        tool("PLCC_QEMU_ARM", "qemu-arm")?;
        tool("PLCC_ARM_CC", "arm-none-eabi-gcc")?;
        let out = std::env::temp_dir().join(format!("plcc_image_harness_{}", std::process::id()));
        let o = Command::new("sh")
            .arg(repo("runtimes/arduino-opta/loader/emu/build.sh"))
            .arg(&out)
            .env("CC", std::env::var_os("PLCC_ARM_CC").unwrap_or_else(|| "arm-none-eabi-gcc".into()))
            .output()
            .unwrap();
        assert!(o.status.success(), "harness build: {}", text(&o));
        Some(out)
    })
    .as_ref()
}

/// Run `img` in the harness: (exit code, stdout).
fn emulate(img: &Path, scans: u32, step_ms: u32, inputs: &str) -> Option<(i32, String)> {
    let h = harness()?;
    let qemu = std::env::var_os("PLCC_QEMU_ARM").unwrap_or_else(|| "qemu-arm".into());
    let mut cmd = Command::new(qemu);
    cmd.args(["-cpu", "cortex-m7"]).arg(h).arg(img).arg(scans.to_string()).arg(step_ms.to_string());
    if !inputs.is_empty() {
        cmd.arg(inputs);
    }
    let o = cmd.output().unwrap();
    Some((o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned()))
}

fn m_bytes(line: &str) -> Vec<u8> {
    let hex = line.split(" M=").nth(1).unwrap().trim();
    (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect()
}

fn q_of(line: &str) -> &str {
    line.split(" Q=").nth(1).unwrap().split(' ').next().unwrap()
}

#[test]
fn emulated_services_and_print() {
    let (_, img, _) = build(&repo("crates/plcc-cli/tests/data/image/services.st"), "-O2", "emu");
    let Some((code, out)) = emulate(&img, 1, 10, "") else {
        eprintln!("skipped: qemu-arm or arm-none-eabi-gcc not found");
        return;
    };
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("check ok\napp tasks=1\n"), "{out}");
    assert!(out.contains("print services ok\n"), "{out}");
    let m = m_bytes(out.lines().find(|l| l.starts_with("scan 0 ")).unwrap());
    let i64_at = |o: usize| i64::from_le_bytes(m[o..o + 8].try_into().unwrap());
    let f64_at = |o: usize| f64::from_le_bytes(m[o..o + 8].try_into().unwrap());
    let f32_at = |o: usize| f32::from_le_bytes(m[o..o + 4].try_into().unwrap());
    let n: i64 = 1_234_567_890_123;
    assert_eq!(i64_at(8), n / 1000, "LINT division (__divdi3/__aeabi_ldivmod)");
    assert_eq!(i64_at(16), n % 1000, "LINT MOD");
    let s = 0.5f64.sin() + 0.5f64.powf(2.5);
    assert!((f64_at(24) - s).abs() < 1e-12, "SIN + EXPT (libm): {} vs {s}", f64_at(24));
    assert_eq!(f32_at(32), n as f32, "LINT_TO_REAL (__floatdisf)");
    assert!((f32_at(36) - 0.25f32.cos()).abs() < 1e-6, "COS(REAL) (cosf)");
    assert_eq!(i64_at(40), n * 3);
}

#[test]
fn emulated_fault_reaches_the_runtime() {
    let (_, img, _) = build(&repo("crates/plcc-cli/tests/data/image/div_zero.st"), "-O2", "emu");
    let Some((code, out)) = emulate(&img, 10, 20, "") else {
        eprintln!("skipped: qemu-arm or arm-none-eabi-gcc not found");
        return;
    };
    // Through the plcc_fault service (not the object's weak trapping default),
    // with the source location, on the third scan.
    assert_eq!(code, 3, "{out}");
    let fault = out.lines().find(|l| l.starts_with("fault ")).unwrap();
    assert!(fault.starts_with("fault 1 ") && fault.ends_with("div_zero.st:13:10: Div"), "{fault}");
    assert_eq!(out.lines().filter(|l| l.starts_with("scan ")).count(), 2, "{out}");
}

#[test]
fn emulated_timer_chase() {
    let (_, img, _) = build(&repo("crates/plcc-cli/tests/data/image/chase.st"), "-O2", "emu");
    let Some((code, out)) = emulate(&img, 40, 10, "01") else {
        eprintln!("skipped: qemu-arm or arm-none-eabi-gcc not found");
        return;
    };
    assert_eq!(code, 0, "{out}");
    // MainTask runs every 20 ms; TON(PT := 100 ms) restarts one scan after Q.
    let mut changes = Vec::new();
    let mut last = "";
    for l in out.lines().filter(|l| l.starts_with("scan ")) {
        let q = q_of(l);
        if q != last {
            let t: u32 = l.split(" t=").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap();
            changes.push((t, q.to_string()));
            last = q;
        }
    }
    let want: Vec<(u32, String)> = [(0, "01"), (100, "02"), (240, "04"), (380, "01")].iter().map(|(t, q)| (*t, q.to_string())).collect();
    assert_eq!(changes, want, "{out}");
    // Without %IX0.0 the relays stay off.
    let (_, out) = emulate(&img, 10, 10, "00").unwrap();
    assert!(out.lines().filter(|l| l.starts_with("scan ")).all(|l| q_of(l) == "00"), "{out}");
}

#[test]
fn emulated_gcc_object_with_data_and_common() {
    let Some(cc) = tool("PLCC_ARM_CC", "arm-none-eabi-gcc") else {
        eprintln!("skipped: arm-none-eabi-gcc not found");
        return;
    };
    let dir = tmp();
    let obj = dir.join("data_prog.o");
    let st = Command::new(cc)
        .args(["-mcpu=cortex-m7", "-mthumb", "-mfloat-abi=softfp", "-mfpu=fpv5-d16", "-O1", "-fcommon", "-fno-builtin", "-c"])
        .arg(repo("crates/plcc-cli/tests/data/image/data_prog.c"))
        .arg("-o")
        .arg(&obj)
        .status()
        .unwrap();
    assert!(st.success());
    let img = dir.join("data_prog.img");
    let o = plcc(&["image", obj.to_str().unwrap(), "--device", "arduino-opta", "-o", img.to_str().unwrap(), "--json"]);
    assert!(o.status.success(), "{}", text(&o));
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(report["header"]["dataSize"].as_u64().unwrap() >= 8, "{report}");
    assert!(report["sections"].as_array().unwrap().iter().any(|s| s["name"] == "COMMON shared"), "{report}");
    let Some((code, out)) = emulate(&img, 3, 10, "") else {
        eprintln!("skipped: qemu-arm not found");
        return;
    };
    assert_eq!(code, 0, "{out}");
    // .data copied (counter starts at 7; the pointer to the greeting relocated),
    // COMMON zeroed (shared starts at 0).
    assert!(out.contains("print hello from .data\n"), "{out}");
    let scans: Vec<&str> = out.lines().filter(|l| l.starts_with("scan ")).collect();
    assert_eq!(scans.len(), 3, "{out}");
    for (k, l) in scans.iter().enumerate() {
        let m = m_bytes(l);
        assert_eq!(i32::from_le_bytes(m[0..4].try_into().unwrap()), 8 + k as i32, "{l}");
        assert_eq!(i32::from_le_bytes(m[4..8].try_into().unwrap()), 2 * (k as i32 + 1), "{l}");
    }
}

#[test]
fn emulated_loader_refuses_bad_images() {
    let (_, img, _) = build(&repo("tests/fixtures/programs/blink.st"), "-O2", "bad");
    if harness().is_none() {
        eprintln!("skipped: qemu-arm or arm-none-eabi-gcc not found");
        return;
    }
    let good = std::fs::read(&img).unwrap();
    let dir = tmp();
    let case = |name: &str, f: &dyn Fn(&mut Vec<u8>)| -> String {
        let mut b = good.clone();
        f(&mut b);
        let p = dir.join(format!("{name}.img"));
        std::fs::write(&p, &b).unwrap();
        let (code, out) = emulate(&p, 3, 10, "").unwrap();
        assert_eq!(code, 2, "{name}: {out}");
        assert!(!out.contains("app tasks"), "{name}: the loader went past the checks: {out}");
        out
    };
    assert!(case("empty", &|b| b.iter_mut().for_each(|x| *x = 0xff)).contains("reject empty slot"));
    assert!(case("body", &|b| {
        let n = b.len();
        b[n / 2] ^= 1
    })
    .contains("reject body CRC mismatch"));
    assert!(case("header", &|b| b[20] ^= 1).contains("reject header CRC mismatch"));
    assert!(case("truncated", &|b| b.truncate(b.len() / 2)).contains("reject body CRC mismatch"));
    // The good image runs every scan (blink toggles a variable; no fault).
    let (code, out) = emulate(&img, 5, 20, "").unwrap();
    assert_eq!(code, 0, "{out}");
    assert_eq!(out.lines().filter(|l| l.starts_with("scan ")).count(), 5, "{out}");
}

/// The runtime's fault guard (runtimes/arduino-opta/loader/plcc_guard.c) on
/// QEMU's Cortex-M7 board, set up as the Opta runs it (vectors in RAM at
/// 0x20000000, thread mode on the PSP, a 10 ms tick): a UsageFault, the
/// watchdog, a BusFault and plcc_fault each stop the task, turn the outputs
/// off and leave the program runnable; a fault outside the program goes to the
/// original handler.
#[test]
fn emulated_guard_contains_faults() {
    let (Some(cc), Some(qemu)) = (tool("PLCC_ARM_CC", "arm-none-eabi-gcc"), tool("PLCC_QEMU_SYSTEM_ARM", "qemu-system-arm")) else {
        eprintln!("skipped: arm-none-eabi-gcc or qemu-system-arm not found");
        return;
    };
    let dir = tmp();
    let elf = dir.join("guard_test.elf");
    let o = Command::new("sh")
        .arg(repo("runtimes/arduino-opta/loader/emu/build-guard.sh"))
        .arg(&elf)
        .env("CC", &cc)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", text(&o));
    let obj = dir.join("guard_prog.o");
    let st = Command::new(&cc)
        .args(["-mcpu=cortex-m7", "-mthumb", "-mfloat-abi=softfp", "-mfpu=fpv5-d16", "-O1", "-fno-builtin", "-c"])
        .arg(repo("crates/plcc-cli/tests/data/image/guard_prog.c"))
        .arg("-o")
        .arg(&obj)
        .status()
        .unwrap();
    assert!(st.success());
    let img = dir.join("guard.img");
    let manifest = repo("crates/plcc-cli/tests/data/image/guard-test.toml");
    let o = plcc(&["image", obj.to_str().unwrap(), "--device", manifest.to_str().unwrap(), "-o", img.to_str().unwrap()]);
    assert!(o.status.success(), "{}", text(&o));
    let o = Command::new(qemu)
        .args(["-M", "mps2-an500", "-nographic", "-semihosting-config", "enable=on,target=native", "-kernel"])
        .arg(&elf)
        .arg("-device")
        .arg(format!("loader,file={},addr=0x00100000", img.display()))
        .output()
        .unwrap();
    // Semihosting output goes to stdout or stderr depending on QEMU's chardev setup.
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    let want = [
        "started",
        "run 0 ok m0=101",
        "run 1 fault code=65537 exc=3 pc_in_text=1 off=1 where=HardFault",
        "run 0 ok m0=102",
        "run 2 fault code=65538 exc=0 pc_in_text=1 off=1 where=watchdog: a scan ran longer than 500 ms",
        "run 0 ok m0=103",
        "run 3 fault code=65537 exc=3 pc_in_text=1 off=1 where=HardFault",
        "run 0 ok m0=104",
        "run 4 fault code=1 exc=0 pc_in_text=0 off=1 where=guard_prog.c:61: task 4",
        "run 0 ok m0=105",
        "chained off=1",
    ];
    let got: Vec<&str> = out.lines().map(str::trim_end).collect();
    assert_eq!(got, want, "{out}");
}

/// Every fixture program runs 30 scans in the emulator without a fault or a
/// rejection (programs that divide by zero on purpose are not fixtures).
#[test]
fn emulated_fixtures_run() {
    if harness().is_none() {
        eprintln!("skipped: qemu-arm or arm-none-eabi-gcc not found");
        return;
    }
    let mut ran = 0;
    for src in fixtures() {
        if src.file_name().is_some_and(|n| n == "div_zero.st") {
            continue;
        }
        let (_, img, _) = build(&src, "-O2", "run");
        let (code, out) = emulate(&img, 30, 10, "").unwrap();
        assert_eq!(code, 0, "{}: {out}", src.display());
        assert!(out.contains("check ok") && !out.contains("fault "), "{}: {out}", src.display());
        ran += 1;
    }
    assert!(ran >= 20, "{ran}");
}
