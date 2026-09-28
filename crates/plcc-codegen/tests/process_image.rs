// SPDX-License-Identifier: MPL-2.0

//! The process image (`%I`/`%Q`/`%M`), AT bindings, and the task table / entry
//! points of the runtime contract, exercised through the JIT exactly as a runtime
//! would use them: through `plcc_get_app()` only, never a program name.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;
use plcc_codegen::direct_address::Area;
use std::ffi::CStr;
use std::os::raw::c_char;

#[repr(C)]
struct Image {
    input: *mut u8,
    input_size: u32,
    output: *mut u8,
    output_size: u32,
    memory: *mut u8,
    memory_size: u32,
}

#[repr(C)]
struct Instance {
    name: *const c_char,
    program_type: *const c_char,
    init: extern "C" fn(*mut u8),
    scan: extern "C" fn(*mut u8),
    state: *mut u8,
    state_size: u64,
}

#[repr(C)]
struct Task {
    name: *const c_char,
    interval_ns: i64,
    priority: u32,
    program_count: u32,
    single: Option<extern "C" fn() -> u8>,
    programs: *const Instance,
}

#[repr(C)]
struct Retain {
    name: *const c_char,
    data: *mut u8,
    size: u64,
}

#[repr(C)]
struct App {
    abi_version: u32,
    task_count: u32,
    tasks: *const Task,
    image: *const Image,
    init: extern "C" fn(),
    run_task: extern "C" fn(u32),
    retain: *const Retain,
    retain_count: u32,
    retain_signature: u32,
}

/// A loaded, JIT-compiled module seen through its `plcc_app` descriptor.
struct Loaded<'a> {
    app: &'a App,
}

impl Loaded<'_> {
    fn image(&self) -> &Image {
        unsafe { &*self.app.image }
    }
    fn input(&self) -> &mut [u8] {
        let i = self.image();
        unsafe { std::slice::from_raw_parts_mut(i.input, i.input_size as usize) }
    }
    fn output(&self) -> &mut [u8] {
        let i = self.image();
        unsafe { std::slice::from_raw_parts_mut(i.output, i.output_size as usize) }
    }
    fn memory(&self) -> &mut [u8] {
        let i = self.image();
        unsafe { std::slice::from_raw_parts_mut(i.memory, i.memory_size as usize) }
    }
    fn tasks(&self) -> &[Task] {
        unsafe { std::slice::from_raw_parts(self.app.tasks, self.app.task_count as usize) }
    }
    fn task_programs(&self, t: usize) -> &[Instance] {
        let t = &self.tasks()[t];
        unsafe { std::slice::from_raw_parts(t.programs, t.program_count as usize) }
    }
    fn retain(&self) -> &[Retain] {
        unsafe { std::slice::from_raw_parts(self.app.retain, self.app.retain_count as usize) }
    }
    fn run(&self, task: u32) {
        (self.app.run_task)(task)
    }
}

fn cstr(p: *const c_char) -> String {
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn with_app<R>(source: &str, f: impl FnOnce(&Loaded) -> R) -> R {
    with_app_sized(source, &[], f)
}

fn with_app_sized<R>(source: &str, sizes: &[(Area, u32)], f: impl FnOnce(&Loaded) -> R) -> R {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "image");
    for (a, n) in sizes {
        compiler.set_image_size(*a, *n);
    }
    compiler.compile(&unit).expect("codegen failed");
    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("JIT");
    let get = ee.get_function_address("plcc_get_app").expect("plcc_get_app");
    let get: extern "C" fn() -> *const App = unsafe { std::mem::transmute(get) };
    let app = unsafe { &*get() };
    assert_eq!(app.abi_version, 1);
    (app.init)();
    f(&Loaded { app })
}

fn compile_err(source: &str) -> plcc_codegen::compiler::CodegenError {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "image");
    match compiler.compile(&unit) {
        Ok(()) => panic!("expected a compile error"),
        Err(e) => e,
    }
}

// ── Process image and AT ──────────────────────────────────────────────────

#[test]
fn at_bits_and_words_are_the_image() {
    let src = r#"
PROGRAM Main
VAR
    start AT %IX0.0 : BOOL;
    stop  AT %IX0.3 : BOOL;
    raw   AT %IW1 : INT;
    run   AT %QX0.1 : BOOL;
    level AT %QW1 : INT;
    count : DINT;
END_VAR
    run := start AND NOT stop;
    level := raw * 2;
    count := count + 1;
END_PROGRAM
"#;
    with_app(src, |app| {
        assert_eq!(app.image().input_size, 4, "%IW1 ends at byte 4");
        assert_eq!(app.image().output_size, 4);
        app.input()[0] = 0b0000_0001;
        app.input()[2..4].copy_from_slice(&21i16.to_ne_bytes());
        app.output()[0] = 0b1000_0000; // a bit the program never touches
        app.run(0);
        assert_eq!(app.output()[0], 0b1000_0010, "RMW sets bit 1, keeps bit 7");
        assert_eq!(i16::from_ne_bytes([app.output()[2], app.output()[3]]), 42);

        app.input()[0] = 0b0000_1001; // stop pressed
        app.run(0);
        assert_eq!(app.output()[0], 0b1000_0000, "RMW clears bit 1 only");
    });
}

#[test]
fn overlapping_addresses_alias() {
    let src = r#"
PROGRAM Main
VAR
    b3 AT %IX0.3 : BOOL;
    whole AT %IB0 : BYTE;
    w AT %MW0 : WORD;
    lo AT %MB0 : BYTE;
    hi AT %MX1.7 : BOOL;
    seen_b3 : BOOL;
    seen_byte : BYTE;
END_VAR
    seen_b3 := b3;
    seen_byte := whole;
    w := 16#8001;
END_PROGRAM
"#;
    with_app(src, |app| {
        app.input()[0] = 0x08;
        app.run(0);
        let st = app.task_programs(0)[0].state;
        let _ = st;
        // %MW0 = bytes 0..1 native order; %MB0 is its first byte, %MX1.7 the top bit
        // of the second on a little-endian host.
        let w = u16::from_ne_bytes([app.memory()[0], app.memory()[1]]);
        assert_eq!(w, 0x8001);
        if cfg!(target_endian = "little") {
            assert_eq!(app.memory()[0], 0x01);
            assert_eq!(app.memory()[1] & 0x80, 0x80);
        }
    });
    // And read back through the aliases.
    let src2 = r#"
PROGRAM Main
VAR
    whole AT %IB0 : BYTE;
    b3 AT %IX0.3 : BOOL;
    out_byte AT %QB0 : BYTE;
    out_b3 AT %QX1.0 : BOOL;
END_VAR
    out_byte := whole;
    out_b3 := b3;
END_PROGRAM
"#;
    with_app(src2, |app| {
        app.input()[0] = 0x08;
        app.run(0);
        assert_eq!(app.output()[0], 0x08);
        assert_eq!(app.output()[1], 0x01);
    });
}

#[test]
fn direct_addresses_in_statements() {
    let src = r#"
PROGRAM Main
VAR
    n : INT;
END_VAR
    %QX0.2 := %IX0.5;
    %QW1 := %IW0 + 1;
    %MD1 := DINT#100000;
    n := %IW0;
END_PROGRAM
"#;
    with_app(src, |app| {
        app.input()[0] = 0b0010_0000;
        app.input()[1] = 0;
        app.run(0);
        assert_eq!(app.output()[0], 0b0000_0100);
        let w0 = u16::from_ne_bytes([app.input()[0], app.input()[1]]);
        let q = u16::from_ne_bytes([app.output()[2], app.output()[3]]);
        assert_eq!(q, w0 + 1);
        let md1 = u32::from_ne_bytes(app.memory()[4..8].try_into().unwrap());
        assert_eq!(md1, 100000, "%MD1 is bytes 4..8");
        assert_eq!(app.image().memory_size, 8);
    });
}

#[test]
fn global_at_with_initializer_and_marker_area() {
    let src = r#"
VAR_GLOBAL
    lamp AT %QX2.4 : BOOL := TRUE;
    setpoint AT %MW2 : INT := 1234;
    total : DINT;
END_VAR
PROGRAM Main
    total := total + setpoint;
END_PROGRAM
"#;
    with_app(src, |app| {
        assert_eq!(app.output()[2], 0b0001_0000, "initializer applied by plcc_init");
        assert_eq!(i16::from_ne_bytes([app.memory()[4], app.memory()[5]]), 1234);
        app.memory()[4..6].copy_from_slice(&7i16.to_ne_bytes());
        app.run(0);
        app.run(0);
    });
}

#[test]
fn at_inside_function_block_instances() {
    let src = r#"
FUNCTION_BLOCK Blinker
VAR
    lamp AT %QX0.0 : BOOL;
END_VAR
    lamp := NOT lamp;
END_FUNCTION_BLOCK
PROGRAM Main
VAR
    b : Blinker;
END_VAR
    b();
END_PROGRAM
"#;
    with_app(src, |app| {
        app.run(0);
        assert_eq!(app.output()[0], 1);
        app.run(0);
        assert_eq!(app.output()[0], 0);
    });
}

#[test]
fn legacy_per_program_entry_points_still_see_the_image() {
    let src = r#"
PROGRAM Main
VAR
    x : INT;
    y AT %QW0 : INT := 5;
END_VAR
    x := x + y;
END_PROGRAM
"#;
    let (unit, errors) = plcc_st::parse(src);
    assert!(errors.is_empty());
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "legacy");
    compiler.compile(&unit).expect("codegen");
    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .unwrap();
    let init: extern "C" fn(*mut u8) =
        unsafe { std::mem::transmute(ee.get_function_address("main_init").unwrap()) };
    let scan: extern "C" fn(*mut u8) =
        unsafe { std::mem::transmute(ee.get_function_address("main_scan").unwrap()) };
    let mut state = [0u8; 16];
    init(state.as_mut_ptr());
    scan(state.as_mut_ptr());
    scan(state.as_mut_ptr());
    assert_eq!(i16::from_ne_bytes([state[0], state[1]]), 10);
}

#[test]
fn image_size_override() {
    let src = "PROGRAM Main VAR a AT %IX0.0 : BOOL; END_VAR END_PROGRAM";
    with_app_sized(src, &[(Area::Input, 64), (Area::Memory, 16)], |app| {
        assert_eq!(app.image().input_size, 64);
        assert_eq!(app.image().output_size, 0);
        assert_eq!(app.image().memory_size, 16);
    });
    let (unit, _) = plcc_st::parse("PROGRAM Main VAR a AT %IW4 : WORD; END_VAR END_PROGRAM");
    let ctx = Context::create();
    let mut compiler = Compiler::new(&ctx, "x");
    compiler.set_image_size(Area::Input, 4);
    let err = compiler.compile(&unit).unwrap_err().to_string();
    assert!(err.contains("too small"), "{err}");
}

#[test]
fn invalid_addresses_are_located_errors() {
    let cases = [
        ("a AT %IX0.8 : BOOL;", "bit 8"),
        ("a AT %IX0.1 : INT;", "only BOOL"),
        ("a AT %IW0 : DINT;", "16-bit location"),
        ("a AT %IB0 : BOOL;", "needs a bit address"),
        ("a AT %IB1 : ARRAY[0..1] OF WORD;", "alignment"),
        ("a AT %I* : BOOL;", "not yet supported"),
        ("a AT %IW1.2 : WORD;", "only X (bit) addresses"),
    ];
    for (decl, want) in cases {
        let src = format!("PROGRAM Main\nVAR\n    {decl}\nEND_VAR\nEND_PROGRAM\n");
        let err = compile_err(&src);
        let span = err.span().unwrap_or_else(|| panic!("{decl}: no span on `{err}`"));
        assert!(err.to_string().contains(want), "{decl}: `{err}` lacks `{want}`");
        assert!(
            span.start > 0 && span.end <= src.len() && span.start < span.end,
            "{decl}: bad span {span:?}"
        );
        assert!(src[span.start..span.end].contains('%') || src[span.start..span.end].contains(" a"),
            "{decl}: span `{}` does not point at the declaration", &src[span.start..span.end]);
    }
    let err = compile_err(
        "PROGRAM Main\nVAR_INPUT\n    a AT %IX0.0 : BOOL;\nEND_VAR\nEND_PROGRAM\n",
    );
    assert!(err.to_string().contains("VAR and VAR_GLOBAL"), "{err}");
}

#[test]
fn bit_variables_have_no_address() {
    let err = compile_err(
        r#"
FUNCTION_BLOCK Latch
VAR_IN_OUT q : BOOL; END_VAR
    q := TRUE;
END_FUNCTION_BLOCK
PROGRAM Main
VAR
    l : Latch;
    o AT %QX0.0 : BOOL;
END_VAR
    l(q := o);
END_PROGRAM
"#,
    );
    assert!(err.to_string().contains("single bit"), "{err}");
}

// ── CONFIGURATION / RESOURCE / TASK ───────────────────────────────────────

#[test]
fn no_configuration_gives_one_default_task() {
    let src = r#"
PROGRAM A VAR n : INT; END_VAR n := n + 1; %QB0 := INT_TO_BYTE(n); END_PROGRAM
PROGRAM B VAR n : INT; END_VAR n := n + 10; %QB1 := INT_TO_BYTE(n); END_PROGRAM
"#;
    with_app(src, |app| {
        assert_eq!(app.app.task_count, 1);
        let t = &app.tasks()[0];
        assert_eq!(cstr(t.name), "MainTask");
        assert_eq!(t.interval_ns, 20_000_000);
        let progs = app.task_programs(0);
        assert_eq!(progs.len(), 2);
        assert_eq!(cstr(progs[0].name), "A");
        assert_eq!(cstr(progs[1].program_type), "B");
        app.run(0);
        app.run(0);
        assert_eq!(app.output()[0..2], [2, 20]);
        app.run(7); // out of range: no-op
        assert_eq!(app.output()[0..2], [2, 20]);
    });
}

#[test]
fn configuration_tasks_and_instances() {
    let src = r#"
PROGRAM Counter
VAR_INPUT step : INT; END_VAR
VAR_OUTPUT count : INT; END_VAR
    count := count + step;
END_PROGRAM

CONFIGURATION Cell
    VAR_GLOBAL
        g_step : INT := 3;
        g_fast : INT;
        g_slow : INT;
        trigger : BOOL;
    END_VAR
    RESOURCE Cpu ON PLC
        TASK Fast (INTERVAL := T#5ms, PRIORITY := 0);
        TASK Slow (INTERVAL := T#100ms, PRIORITY := 2);
        TASK OnEvent (SINGLE := trigger, PRIORITY := 1);
        PROGRAM fast1 WITH Fast : Counter (step := g_step, count => g_fast);
        PROGRAM slow1 WITH Slow : Counter (step := 100, count => g_slow);
        PROGRAM bg : Counter;
    END_RESOURCE
END_CONFIGURATION
"#;
    with_app(src, |app| {
        let names: Vec<String> = app.tasks().iter().map(|t| cstr(t.name)).collect();
        assert_eq!(names, ["Fast", "Slow", "OnEvent", "__background"]);
        assert_eq!(app.tasks()[0].interval_ns, 5_000_000);
        assert_eq!(app.tasks()[1].priority, 2);
        assert_eq!(app.tasks()[3].priority, u32::MAX);
        assert_eq!(cstr(app.task_programs(0)[0].name), "Cpu.fast1");
        assert_eq!(cstr(app.task_programs(1)[0].program_type), "Counter");

        let single = app.tasks()[2].single.expect("SINGLE function");
        assert_eq!(single(), 0);
        assert!(app.tasks()[0].single.is_none());

        app.run(0);
        app.run(0);
        app.run(1);
        // Separate state per instance; outputs connected to globals.
        let fast = app.task_programs(0)[0].state;
        let slow = app.task_programs(1)[0].state;
        assert_ne!(fast, slow);
        let count_of = |p: *mut u8| unsafe { i16::from_ne_bytes([*p.add(2), *p.add(3)]) };
        assert_eq!(count_of(fast), 6);
        assert_eq!(count_of(slow), 100);
    });
}

#[test]
fn configuration_without_resource_and_connections_to_image() {
    let src = r#"
PROGRAM Relay
VAR_INPUT i : BOOL; END_VAR
VAR_OUTPUT o : BOOL; END_VAR
    o := NOT i;
END_PROGRAM
CONFIGURATION C
    VAR_GLOBAL
        sw AT %IX0.0 : BOOL;
        coil AT %QX0.0 : BOOL;
    END_VAR
    TASK T (INTERVAL := T#10ms);
    PROGRAM r WITH T : Relay (i := sw, o => coil);
END_CONFIGURATION
"#;
    with_app(src, |app| {
        assert_eq!(app.app.task_count, 1);
        app.run(0);
        assert_eq!(app.output()[0], 1);
        app.input()[0] = 1;
        app.run(0);
        assert_eq!(app.output()[0], 0);
    });
}

#[test]
fn configuration_errors() {
    let base = |body: &str| {
        format!("PROGRAM P VAR x : INT; END_VAR END_PROGRAM\nCONFIGURATION C RESOURCE R ON X {body} END_RESOURCE END_CONFIGURATION")
    };
    let cases = [
        (base("PROGRAM p WITH Nope : P;"), "not a TASK"),
        (base("TASK T (INTERVAL := T#1ms); PROGRAM p WITH T : Q;"), "not a PROGRAM"),
        (base("TASK T (SPEED := 3); PROGRAM p WITH T : P;"), "unknown TASK property"),
        (base("TASK T (INTERVAL := T#1ms); PROGRAM p WITH T : P (y := 1);"), "no variable"),
    ];
    for (src, want) in cases {
        let err = compile_err(&src);
        assert!(err.to_string().contains(want), "`{err}` lacks `{want}`");
        assert!(err.span().is_some(), "{err} has no span");
    }
}

#[test]
fn retain_regions_cover_program_fb_and_global_retain() {
    let src = r#"
FUNCTION_BLOCK Odo
VAR RETAIN km : LREAL; END_VAR
VAR scratch : INT; END_VAR
END_FUNCTION_BLOCK
VAR_GLOBAL RETAIN
    g_hours : UDINT;
END_VAR
PROGRAM Main
VAR RETAIN
    starts : DINT;
END_VAR
VAR
    odo : Odo;
    tmp : INT;
END_VAR
    starts := starts + 1;
END_PROGRAM
"#;
    with_app(src, |app| {
        let names: Vec<String> = app.retain().iter().map(|r| cstr(r.name)).collect();
        assert_eq!(names, ["Main.starts", "Main.odo.km", "GLOBAL.g_hours"]);
        let sizes: Vec<u64> = app.retain().iter().map(|r| r.size).collect();
        assert_eq!(sizes, [4, 8, 4]);
        app.run(0);
        app.run(0);
        let starts = unsafe { *(app.retain()[0].data as *const i32) };
        assert_eq!(starts, 2, "the region points at the live instance variable");
        assert_ne!(app.app.retain_signature, 0);
    });
}
