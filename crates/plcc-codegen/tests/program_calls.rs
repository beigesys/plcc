// SPDX-License-Identifier: MPL-2.0

//! One PROGRAM calling another (`Sub();`, `Sub(k := 1, o => x);`, `Sub.o`), as
//! CODESYS allows. It was "unknown function". Without a CONFIGURATION a called
//! program runs only when called: the implicit task would otherwise run it a
//! second time every cycle.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use plcc_codegen::Compiler;

const SRC: &str = r#"
PROGRAM Sub
VAR_INPUT k : INT; END_VAR
VAR_OUTPUT o : INT; END_VAR
VAR n : INT := 100; END_VAR
n := n + k;
o := n;
END_PROGRAM
PROGRAM Main
VAR r : INT; r2 : INT; cycles : INT; END_VAR
cycles := cycles + 1;
Sub(k := 2);
r := Sub.o;
Sub(k := 1, o => r2);
END_PROGRAM
"#;

#[test]
fn a_program_calls_another_through_the_task_runtime() {
    let (unit, errors) = plcc_st::parse(SRC);
    assert!(errors.is_empty(), "{errors:?}");
    let ctx = Context::create();
    let mut c = Compiler::new(&ctx, "calls");
    c.compile(&unit).expect("codegen");
    let ee = c
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .unwrap();
    let contract = c.runtime_contract("x86_64-unknown-linux-gnu").unwrap();
    assert_eq!(contract.tasks.len(), 1);
    // Only Main is scheduled; Sub runs when Main calls it.
    assert_eq!(contract.tasks[0].instances.len(), 1);
    assert_eq!(contract.instances[contract.tasks[0].instances[0]].program, "Main");

    #[repr(C)]
    struct App {
        abi: u32,
        ntasks: u32,
        tasks: *const Task,
    }
    #[repr(C)]
    struct Task {
        name: *const u8,
        interval: i64,
        priority: u32,
        nprograms: u32,
        single: *const u8,
        programs: *const Prog,
    }
    #[repr(C)]
    struct Prog {
        name: *const u8,
        ty: *const u8,
        init: *const u8,
        scan: *const u8,
        instance: *mut i16,
        size: i64,
    }
    unsafe {
        let init: extern "C" fn() = std::mem::transmute(ee.get_function_address("plcc_init").unwrap());
        let run: extern "C" fn(u32) =
            std::mem::transmute(ee.get_function_address("plcc_run_task").unwrap());
        let app: extern "C" fn() -> *const App =
            std::mem::transmute(ee.get_function_address("plcc_get_app").unwrap());
        init();
        run(0);
        run(0);
        let task = &*(*app()).tasks;
        assert_eq!(task.nprograms, 1);
        let main = (*task.programs).instance;
        // Main { r, r2, cycles }. Sub.n starts at 100 and gets +2, +1 per cycle.
        assert_eq!(*main.add(2), 2, "Main ran twice");
        assert_eq!(*main, 105, "Sub.o after the first call of the second cycle");
        assert_eq!(*main.add(1), 106, "o => r2: Sub ran exactly four times");
    }
}
