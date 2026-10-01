// SPDX-License-Identifier: MPL-2.0
//! plcc-wasm-link against wasm-ld: for each object in tests/data (rebuilt by
//! tests/data/build.sh), link it, validate the result, compare its imports
//! and exports with wasm-ld's output for the same object, then run both
//! modules side by side in wasmi — `plcc_init`, then every task for many
//! scans with inputs poked into %I / %M and a controlled clock — and require
//! identical process images, program state, PRINT output and faults.

use std::fmt::Debug;

use plcc_wasm_link::{LinkError, Options, link};
use wasmi::{
    Caller, Engine, Error, Extern, ExternType, F32, F64, Instance, Linker, Module, Store, Val,
    ValType,
};

/// Objects plcc produced (plus `indirect`, from LLVM IR).
const PLCC_OBJECTS: &[&str] = &[
    "seal_in_O0",
    "seal_in_O2",
    "ton_O0",
    "ton_O2",
    "div_zero_O0",
    "div_zero_O2",
    "stdlib_math_O2",
    "opta_io_O2",
];

/// A file in tests/data, or at `name` if that is an absolute path.
fn data(name: &str) -> Vec<u8> {
    let path = if name.starts_with('/') {
        name.to_string()
    } else {
        format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))
    };
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Our link of `<name>.o`, validated, and wasm-ld's `<name>.ld.wasm`.
fn both(name: &str) -> (Vec<u8>, Vec<u8>) {
    let ours = link(&data(&format!("{name}.o")), &Options::default())
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut features = wasmparser::WasmFeatures::default();
    features.remove(wasmparser::WasmFeatures::GC); // plain MVP + post-MVP features only
    wasmparser::Validator::new_with_features(features)
        .validate_all(&ours)
        .unwrap_or_else(|e| panic!("{name}: our output does not validate: {e}"));
    (ours, data(&format!("{name}.ld.wasm")))
}

// ---- interface comparison -------------------------------------------------

fn interface(wasm: &[u8]) -> (Vec<String>, Vec<String>) {
    let module = Module::new(&Engine::default(), wasm).expect("module");
    let imports = module
        .imports()
        .map(|i| format!("{}.{}: {:?}", i.module(), i.name(), i.ty()))
        .collect();
    let exports = module
        .exports()
        .map(|e| {
            let ty = match e.ty() {
                // Sizes depend on the layout; kinds and signatures must match.
                ExternType::Memory(_) => "memory".to_string(),
                ExternType::Table(t) => format!("table {:?}", t.element()),
                ty => format!("{ty:?}"),
            };
            format!("{}: {ty}", e.name())
        })
        .collect();
    (imports, exports)
}

fn compare_interface(name: &str, plcc_abi: bool) {
    let (ours, theirs) = both(name);
    let (oi, oe) = interface(&ours);
    let (ti, te) = interface(&theirs);
    assert_eq!(oi, ti, "{name}: imports");
    assert_eq!(oe, te, "{name}: exports");
    let required = [
        "memory",
        "plcc_init",
        "plcc_run_task",
        "plcc_get_app",
        "plcc_image_q",
        "__indirect_function_table",
    ];
    for required in required.iter().filter(|_| plcc_abi) {
        assert!(
            oe.iter().any(|e| e.starts_with(&format!("{required}:"))),
            "{name}: no export {required}"
        );
    }
}

#[test]
fn imports_and_exports_match_wasm_ld() {
    for name in PLCC_OBJECTS {
        compare_interface(name, true);
    }
    compare_interface("indirect", false);
}

/// Every `<name>.o` with a `<name>.ld.wasm` beside it in
/// `$PLCC_WASM_LINK_CORPUS` (an absolute path): interface and 100 lockstep
/// scans; our link is written to `<name>.linked.wasm`. For a wider sweep than
/// the committed objects, e.g. every fixture at -O0 and -O2.
#[test]
#[ignore = "needs PLCC_WASM_LINK_CORPUS"]
fn corpus_matches_wasm_ld() {
    let dir = std::env::var("PLCC_WASM_LINK_CORPUS").expect("PLCC_WASM_LINK_CORPUS");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("corpus dir") {
        let path = entry.expect("entry").path();
        let Some(base) = path.to_str().and_then(|p| p.strip_suffix(".o")) else {
            continue;
        };
        if std::path::Path::new(&format!("{base}.ld.wasm")).exists() {
            compare_interface(base, true);
            lockstep(base, 100);
            // Left beside the object for inspection (wasm-objdump, plc-wasm).
            let (ours, _) = both(base);
            std::fs::write(format!("{base}.linked.wasm"), ours).expect("write");
            count += 1;
        }
    }
    assert!(count > 0, "no objects in {dir}");
    eprintln!("{count} objects match wasm-ld");
}

// ---- execution --------------------------------------------------------------

#[derive(Default)]
struct Host {
    now_ns: i64,
    prints: Vec<String>,
    faults: Vec<(i32, String)>,
}

fn c_string(memory: &[u8], ptr: i32) -> String {
    let bytes = &memory[ptr as usize..];
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn caller_memory<'a>(caller: &'a Caller<'_, Host>) -> &'a [u8] {
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .expect("memory export");
    memory.data(caller)
}

fn libm(name: &str) -> Option<fn(&[f64]) -> f64> {
    Some(
        match name
            .strip_suffix('f')
            .filter(|n| *n != "modf")
            .unwrap_or(name)
        {
            "sin" => |a| a[0].sin(),
            "cos" => |a| a[0].cos(),
            "tan" => |a| a[0].tan(),
            "asin" => |a| a[0].asin(),
            "acos" => |a| a[0].acos(),
            "atan" => |a| a[0].atan(),
            "atan2" => |a| a[0].atan2(a[1]),
            "exp" => |a| a[0].exp(),
            "log" => |a| a[0].ln(),
            "log10" => |a| a[0].log10(),
            "pow" => |a| a[0].powf(a[1]),
            "fmod" => |a| a[0] % a[1],
            "round" => |a| a[0].round(),
            _ => return None,
        },
    )
}

struct Plc {
    store: Store<Host>,
    instance: Instance,
    app: u32,
}

impl Plc {
    fn new(wasm: &[u8], now_ns: i64) -> Plc {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm).expect("module");
        let mut linker = Linker::<Host>::new(&engine);
        for import in module.imports() {
            let ExternType::Func(ty) = import.ty() else {
                panic!("non-function import {}", import.name())
            };
            let (m, n) = (import.module(), import.name());
            match n {
                "plcc_monotonic_ns" => {
                    linker.func_wrap(m, n, |c: Caller<'_, Host>| c.data().now_ns)
                }
                "plcc_print" => linker.func_wrap(m, n, |mut c: Caller<'_, Host>, ptr: i32| {
                    let s = c_string(caller_memory(&c), ptr);
                    c.data_mut().prints.push(s);
                }),
                "plcc_fault" => linker.func_wrap(
                    m,
                    n,
                    |mut c: Caller<'_, Host>, code: i32, ptr: i32| -> Result<(), Error> {
                        let at = c_string(caller_memory(&c), ptr);
                        c.data_mut().faults.push((code, at));
                        Err(Error::new("PLC fault"))
                    },
                ),
                "ext" => linker.func_wrap(m, n, |x: i32| x + 100),
                "sink" => linker.func_wrap(m, n, |_: i32| {}),
                _ => {
                    let f = libm(n).unwrap_or_else(|| panic!("no host function for {m}.{n}"));
                    let result = ty.results()[0];
                    linker.func_new(m, n, ty.clone(), move |_, params, results| {
                        let args: Vec<f64> = params
                            .iter()
                            .map(|p| match p {
                                Val::F32(x) => f64::from(x.to_float()),
                                Val::F64(x) => x.to_float(),
                                other => panic!("libm argument {other:?}"),
                            })
                            .collect();
                        let r = f(&args);
                        results[0] = match result {
                            ValType::F32 => Val::F32(F32::from_float(r as f32)),
                            _ => Val::F64(F64::from_float(r)),
                        };
                        Ok(())
                    })
                }
            }
            .expect("define import");
        }
        let mut store = Store::new(
            &engine,
            Host {
                now_ns,
                ..Host::default()
            },
        );
        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .expect("instantiate");
        let mut plc = Plc {
            store,
            instance,
            app: 0,
        };
        if plc
            .instance
            .get_export(&plc.store, "plcc_get_app")
            .is_some()
        {
            plc.app = plc.call("plcc_get_app", &[]).expect("plcc_get_app")[0] as u32;
        }
        plc
    }

    fn call(&mut self, name: &str, args: &[i32]) -> Result<Vec<i32>, Error> {
        let f = self
            .instance
            .get_func(&self.store, name)
            .unwrap_or_else(|| panic!("no export {name}"));
        let params: Vec<Val> = args.iter().map(|&a| Val::I32(a)).collect();
        let mut results = vec![Val::I32(0); f.ty(&self.store).results().len()];
        f.call(&mut self.store, &params, &mut results)?;
        Ok(results
            .iter()
            .map(|r| r.i32().expect("i32 result"))
            .collect())
    }

    fn memory(&self) -> &[u8] {
        let memory = self
            .instance
            .get_memory(&self.store, "memory")
            .expect("memory");
        memory.data(&self.store)
    }

    fn memory_mut(&mut self) -> &mut [u8] {
        let memory = self
            .instance
            .get_memory(&self.store, "memory")
            .expect("memory");
        memory.data_mut(&mut self.store)
    }

    fn u32_at(&self, addr: u32) -> u32 {
        let a = addr as usize;
        u32::from_le_bytes(self.memory()[a..a + 4].try_into().expect("4 bytes"))
    }

    fn symbol(&self, name: &str) -> u32 {
        let global = self
            .instance
            .get_global(&self.store, name)
            .unwrap_or_else(|| panic!("no global {name}"));
        global.get(&self.store).i32().expect("i32 global") as u32
    }

    /// Byte range of process-image area 0 (%I), 1 (%Q) or 2 (%M), from `plcc_app`.
    fn area(&self, area: u32) -> std::ops::Range<usize> {
        let image = self.u32_at(self.app + 12);
        let ptr = self.u32_at(image + area * 8) as usize;
        ptr..ptr + self.u32_at(image + area * 8 + 4) as usize
    }

    fn task_count(&self) -> u32 {
        self.u32_at(self.app + 4)
    }

    /// Task and program names through the contract's pointers (exercises
    /// MEMORY_ADDR_I32 relocations in data).
    fn task_names(&self) -> Vec<String> {
        let tasks = self.u32_at(self.app + 8);
        let mut out = Vec::new();
        for t in 0..self.task_count() {
            let task = tasks + t * 32;
            out.push(c_string(self.memory(), self.u32_at(task) as i32));
            let programs = self.u32_at(task + 28);
            for p in 0..self.u32_at(task + 20) {
                let prog = programs + p * 32;
                out.push(c_string(self.memory(), self.u32_at(prog) as i32));
                out.push(c_string(self.memory(), self.u32_at(prog + 4) as i32));
            }
        }
        out
    }
}

/// Defined, exported data symbols in writable segments (`.data`/`.bss`) and
/// their sizes, from the object's symbol table: the state to compare.
fn state_symbols(object: &[u8]) -> Vec<(String, u32)> {
    use wasmparser::{KnownCustom, Linking, Payload, SymbolFlags, SymbolInfo};
    let mut segments = Vec::new();
    let mut symbols = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(object) {
        let Payload::CustomSection(c) = payload.expect("payload") else {
            continue;
        };
        let KnownCustom::Linking(l) = c.as_known() else {
            continue;
        };
        for sub in l.subsections() {
            match sub.expect("subsection") {
                Linking::SegmentInfo(m) => {
                    segments = m
                        .into_iter()
                        .map(|s| s.expect("segment").name.to_string())
                        .collect()
                }
                Linking::SymbolTable(m) => {
                    symbols = m.into_iter().map(|s| s.expect("symbol")).collect()
                }
                _ => {}
            }
        }
    }
    symbols
        .into_iter()
        .filter_map(|s| match s {
            SymbolInfo::Data {
                flags,
                name,
                symbol: Some(d),
            } if !flags.intersects(SymbolFlags::BINDING_LOCAL | SymbolFlags::VISIBILITY_HIDDEN)
                && !segments[d.index as usize].starts_with(".rodata") =>
            {
                Some((name.to_string(), d.size))
            }
            _ => None,
        })
        .collect()
}

fn state(plc: &Plc, symbols: &[(String, u32)]) -> Vec<(String, Vec<u8>)> {
    symbols
        .iter()
        .map(|(name, size)| {
            let a = plc.symbol(name) as usize;
            (name.clone(), plc.memory()[a..a + *size as usize].to_vec())
        })
        .collect()
}

fn assert_same<T: PartialEq + Debug>(what: &str, ours: T, theirs: T) {
    assert_eq!(
        ours, theirs,
        "{what}: plcc-wasm-link (left) and wasm-ld (right) differ"
    );
}

/// What a run saw, for fixture-specific checks.
#[derive(Default)]
struct Run {
    q_ever_set: bool,
    prints: Vec<String>,
    faults: Vec<(i32, String)>,
}

/// Run both links of `name` side by side for `scans` scans.
fn lockstep(name: &str, scans: u32) -> Run {
    let (ours, theirs) = both(name);
    let symbols = state_symbols(&data(&format!("{name}.o")));
    let mut now: i64 = 1_000_000_000;
    let start = |now| {
        let mut pair = [Plc::new(&ours, now), Plc::new(&theirs, now)];
        for plc in &mut pair {
            plc.call("plcc_init", &[]).expect("plcc_init");
        }
        pair
    };
    let mut pair = start(now);
    assert!(
        !symbols.is_empty() && pair[0].task_count() > 0,
        "{name}: nothing to compare"
    );
    assert_same(
        &format!("{name}: task names"),
        pair[0].task_names(),
        pair[1].task_names(),
    );
    let mut run = Run::default();
    let mut rng: u32 = 0x1234_5678;
    for scan in 0..scans {
        // Inputs change every 40 scans; %M is overwritten now and then, and
        // zeroed once (div_zero's divisor).
        let poke_i = scan % 40 == 0;
        let poke_m = scan % 25 == 3 || scan == 60;
        let i_len = pair[0].area(0).len();
        let m_len = pair[0].area(2).len();
        let mut inputs = Vec::new();
        for _ in 0..i_len + m_len {
            rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
            inputs.push((rng >> 16) as u8 | 1);
        }
        for plc in &mut pair {
            let (i, m) = (plc.area(0), plc.area(2));
            if poke_i {
                plc.memory_mut()[i].copy_from_slice(&inputs[..i_len]);
            }
            if poke_m {
                let fill = if scan == 60 {
                    vec![0; m_len]
                } else {
                    inputs[i_len..].to_vec()
                };
                plc.memory_mut()[m].copy_from_slice(&fill);
            }
            plc.store.data_mut().now_ns = now;
        }
        let mut faulted = false;
        for task in 0..pair[0].task_count() {
            let a = pair[0].call("plcc_run_task", &[task as i32]).is_err();
            let b = pair[1].call("plcc_run_task", &[task as i32]).is_err();
            assert_same(&format!("{name}: scan {scan} task {task} faulted"), a, b);
            faulted |= a;
            if a {
                break;
            }
        }
        let where_ = format!("{name}: scan {scan}");
        for area in 0..3 {
            let (o, t) = (pair[0].area(area), pair[1].area(area));
            assert_same(
                &format!("{where_}: area {area}"),
                &pair[0].memory()[o],
                &pair[1].memory()[t],
            );
        }
        assert_same(
            &format!("{where_}: state"),
            state(&pair[0], &symbols),
            state(&pair[1], &symbols),
        );
        assert_same(
            &format!("{where_}: prints"),
            &pair[0].store.data().prints,
            &pair[1].store.data().prints,
        );
        assert_same(
            &format!("{where_}: faults"),
            &pair[0].store.data().faults,
            &pair[1].store.data().faults,
        );
        let q = pair[0].area(1);
        run.q_ever_set |= pair[0].memory()[q].iter().any(|&b| b != 0);
        now += 13_000_000;
        if faulted {
            // As plc-wasm does: a faulted instance is discarded, cold restart.
            run.faults
                .extend(pair[0].store.data().faults.iter().cloned());
            run.prints
                .extend(pair[0].store.data().prints.iter().cloned());
            pair = start(now);
        }
    }
    run.faults
        .extend(pair[0].store.data().faults.iter().cloned());
    run.prints
        .extend(pair[0].store.data().prints.iter().cloned());
    run
}

#[test]
fn seal_in_runs_like_wasm_ld() {
    for name in ["seal_in_O0", "seal_in_O2"] {
        let run = lockstep(name, 300);
        assert!(
            run.prints.iter().any(|p| p == "motor on"),
            "{name}: never printed"
        );
        assert!(run.q_ever_set, "{name}: motor never on");
        assert!(run.faults.is_empty());
    }
}

#[test]
fn ton_runs_like_wasm_ld() {
    for name in ["ton_O0", "ton_O2"] {
        let run = lockstep(name, 300);
        assert!(run.q_ever_set, "{name}: the timer never elapsed");
    }
}

#[test]
fn div_zero_faults_like_wasm_ld() {
    for name in ["div_zero_O0", "div_zero_O2"] {
        let run = lockstep(name, 120);
        assert!(
            !run.faults.is_empty() && run.faults.iter().all(|f| f.0 == 1),
            "{name}: {:?}",
            run.faults
        );
        assert!(
            run.faults[0].1.contains("div_zero.st"),
            "{name}: fault site {:?}",
            run.faults[0].1
        );
    }
}

#[test]
fn stdlib_math_and_l5x_run_like_wasm_ld() {
    lockstep("stdlib_math_O2", 50);
    lockstep("opta_io_O2", 300);
}

#[test]
fn indirect_calls_and_addresses() {
    let (ours, theirs) = both("indirect");
    let mut results = Vec::new();
    for wasm in [&ours, &theirs] {
        let mut m = Plc::new(wasm, 0);
        let triple = m.call("addr_of_triple", &[]).expect("call")[0];
        let mut r = vec![
            m.call("call_slot", &[0, 5]).expect("call")[0],
            m.call("call_slot", &[1, 5]).expect("call")[0],
            m.call("call_slot", &[2, 5]).expect("call")[0],
            m.call("call_ptr", &[triple, 4]).expect("call")[0],
            m.call("read_mid", &[]).expect("call")[0],
            m.call("read_hidden", &[]).expect("call")[0],
            m.call("weakfn", &[]).expect("call")[0],
            m.call("stack_user", &[21]).expect("call")[0],
        ];
        let zeros = m.symbol("zeros") as usize;
        r.push(i32::from(m.memory()[zeros + 5]));
        let strp = m.symbol("strp");
        r.push(c_string(m.memory(), m.u32_at(strp) as i32).len() as i32);
        results.push(r);
    }
    assert_eq!(results[0], vec![10, 105, 15, 12, 3, 7, 5, 21, 1, 5]);
    assert_same("indirect", &results[0], &results[1]);
}

#[test]
fn layout_matches_wasm_ld_defaults() {
    // Stack pointer: top of a 64 KiB stack above the data; memory covers it.
    let ours = link(&data("ton_O0.o"), &Options::default()).expect("link");
    let m = Plc::new(&ours, 0);
    let module = Module::new(&Engine::default(), &ours[..]).expect("module");
    assert!(module.get_export("__stack_pointer").is_none());
    assert!(m.symbol("plcc_inst_cpu_main") >= 1024);
    let pages = m
        .instance
        .get_memory(&m.store, "memory")
        .expect("memory")
        .size(&m.store);
    assert_eq!(pages, 2);
    let small = Options {
        stack_size: 1024,
        ..Options::default()
    };
    let small = link(&data("ton_O0.o"), &small).expect("link");
    let m = Plc::new(&small, 0);
    assert_eq!(
        m.instance
            .get_memory(&m.store, "memory")
            .expect("memory")
            .size(&m.store),
        1
    );
}

#[test]
fn rejects_what_plcc_never_emits() {
    let opts = Options::default();
    let err = link(&data("ctor.o"), &opts).expect_err("init functions");
    assert!(
        matches!(err, LinkError::Unsupported(ref m) if m.contains("init functions")),
        "{err}"
    );
    let err = link(&data("undef_data.o"), &opts).expect_err("undefined data");
    assert!(
        matches!(err, LinkError::UndefinedData(ref s) if s == "elsewhere"),
        "{err}"
    );
    assert!(matches!(
        link(b"\0asm\x01\0\0\0\x01", &opts),
        Err(LinkError::Malformed(_))
    ));
    assert!(link(b"not wasm", &opts).is_err());
    let odd = Options {
        stack_size: 1000,
        ..Options::default()
    };
    assert!(link(&data("ton_O0.o"), &odd).is_err());
}
