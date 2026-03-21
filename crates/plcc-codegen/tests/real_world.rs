// SPDX-License-Identifier: MPL-2.0

//! Real-world PLC program patterns — the kind of code that runs on critical infrastructure.

use inkwell::context::Context;
use inkwell::OptimizationLevel;
use plcc_codegen::Compiler;

fn jit_run(source: &str, scan_fn_name: &str, state_size: usize, num_scans: usize) -> Vec<u8> {
    let (unit, errors) = plcc_st::parse(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");

    let context = Context::create();
    let mut compiler = Compiler::new(&context, "test");
    compiler.compile(&unit).expect("codegen failed");

    let ee = compiler
        .module()
        .create_jit_execution_engine(OptimizationLevel::None)
        .expect("JIT failed");

    let mut state = vec![0u8; state_size];
    let state_ptr = state.as_mut_ptr();

    // Call init
    let init_fn_name = scan_fn_name.replace("_scan", "_init");
    if let Ok(init_ptr) = ee.get_function_address(&init_fn_name) {
        let init: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(init_ptr) };
        init(state_ptr);
    }

    let fn_ptr = ee.get_function_address(scan_fn_name).expect("fn not found");
    let scan: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(fn_ptr) };
    for _ in 0..num_scans {
        scan(state_ptr);
    }
    state
}

fn read_i16(state: &[u8], offset: usize) -> i16 {
    i16::from_ne_bytes([state[offset], state[offset + 1]])
}

fn read_f32(state: &[u8], offset: usize) -> f32 {
    f32::from_ne_bytes(state[offset..offset + 4].try_into().unwrap())
}

// ── Industrial control patterns ──

#[test]
fn pump_control_interlock() {
    // Real pattern: pump only runs if pressure is in safe range AND not in emergency stop
    let source = r#"
PROGRAM PumpControl
VAR
    pressure : INT := 0;
    e_stop : INT := 0;
    pump_run : INT := 0;
    alarm : INT := 0;
    cycle : INT := 0;
END_VAR
    cycle := cycle + 1;

    (* Simulate pressure rising *)
    IF cycle <= 5 THEN
        pressure := cycle * 20;
    ELSE
        pressure := 100;
    END_IF;

    (* Interlock logic *)
    IF e_stop = 1 THEN
        pump_run := 0;
        alarm := 1;
    ELSIF pressure < 10 THEN
        pump_run := 0;
        alarm := 2;
    ELSIF pressure > 90 THEN
        pump_run := 0;
        alarm := 3;
    ELSE
        pump_run := 1;
        alarm := 0;
    END_IF;
END_PROGRAM
"#;
    // State: pressure(2) + e_stop(2) + pump_run(2) + alarm(2) + cycle(2) = 10 bytes
    // After 1 scan: pressure=20, pump_run=1, alarm=0 (in range 10..90)
    let state = jit_run(source, "pumpcontrol_scan", 10, 1);
    assert_eq!(read_i16(&state, 0), 20, "pressure after scan 1");
    assert_eq!(read_i16(&state, 4), 1, "pump should run at pressure 20");
    assert_eq!(read_i16(&state, 6), 0, "no alarm");

    // After 5 scans: pressure=100, pump_run=0, alarm=3 (over pressure)
    let state = jit_run(source, "pumpcontrol_scan", 10, 5);
    assert_eq!(read_i16(&state, 0), 100, "pressure after scan 5");
    assert_eq!(read_i16(&state, 4), 0, "pump should stop at high pressure");
    assert_eq!(read_i16(&state, 6), 3, "high pressure alarm");
}

#[test]
fn traffic_light_sequence() {
    // State machine: RED -> GREEN -> YELLOW -> RED
    let source = r#"
PROGRAM TrafficLight
VAR
    state : INT := 0;
    timer : INT := 0;
    red : INT := 1;
    yellow : INT := 0;
    green : INT := 0;
END_VAR
    timer := timer + 1;
    CASE state OF
        0: (* RED *)
            red := 1; yellow := 0; green := 0;
            IF timer >= 10 THEN
                state := 1;
                timer := 0;
            END_IF;
        1: (* GREEN *)
            red := 0; yellow := 0; green := 1;
            IF timer >= 8 THEN
                state := 2;
                timer := 0;
            END_IF;
        2: (* YELLOW *)
            red := 0; yellow := 1; green := 0;
            IF timer >= 3 THEN
                state := 0;
                timer := 0;
            END_IF;
    END_CASE;
END_PROGRAM
"#;
    // State: state(2) + timer(2) + red(2) + yellow(2) + green(2) = 10 bytes
    // After 1 scan: state=0, timer=1, red=1
    let state = jit_run(source, "trafficlight_scan", 10, 1);
    assert_eq!(read_i16(&state, 0), 0, "still RED");
    assert_eq!(read_i16(&state, 4), 1, "red on");
    assert_eq!(read_i16(&state, 8), 0, "green off");

    // After 11 scans: state transitions to GREEN on scan 10, outputs set on scan 11
    let state = jit_run(source, "trafficlight_scan", 10, 11);
    assert_eq!(read_i16(&state, 0), 1, "switched to GREEN");
    assert_eq!(read_i16(&state, 4), 0, "red off");
    assert_eq!(read_i16(&state, 8), 1, "green on");

    // After 19 scans: GREEN for 8, transition to YELLOW
    let state = jit_run(source, "trafficlight_scan", 10, 19);
    assert_eq!(read_i16(&state, 0), 2, "switched to YELLOW");
    assert_eq!(read_i16(&state, 6), 1, "yellow on");

    // After 22 scans: YELLOW for 3, back to RED
    let state = jit_run(source, "trafficlight_scan", 10, 22);
    assert_eq!(read_i16(&state, 0), 0, "back to RED");
    assert_eq!(read_i16(&state, 4), 1, "red on again");
}

#[test]
fn pid_proportional_only() {
    // Simplified P-only controller — verify output tracks error
    let source = r#"
PROGRAM PController
VAR
    setpoint : REAL := 50.0;
    measured : REAL := 0.0;
    kp : REAL := 2.0;
    error : REAL;
    output : REAL;
END_VAR
    measured := measured + output * 0.1;
    error := setpoint - measured;
    output := kp * error;
END_PROGRAM
"#;
    // State: setpoint(4) + measured(4) + kp(4) + error(4) + output(4) = 20 bytes
    // After 1 scan: error=50, output=100, measured starts moving
    let state = jit_run(source, "pcontroller_scan", 20, 1);
    let error = read_f32(&state, 12);
    let output = read_f32(&state, 16);
    assert!((error - 50.0).abs() < 0.01, "initial error = {error}");
    assert!((output - 100.0).abs() < 0.01, "initial output = {output}");

    // After 50 scans: should converge toward setpoint
    let state = jit_run(source, "pcontroller_scan", 20, 50);
    let measured = read_f32(&state, 4);
    let error = read_f32(&state, 12);
    assert!(measured > 40.0, "measured should approach setpoint, got {measured}");
    assert!(error.abs() < 15.0, "error should decrease, got {error}");
}

#[test]
fn batch_counter_with_reset() {
    // Count items in a batch, reset every 100
    let source = r#"
PROGRAM BatchCounter
VAR
    item_count : INT := 0;
    batch_count : INT := 0;
    batch_complete : INT := 0;
END_VAR
    item_count := item_count + 1;
    IF item_count >= 100 THEN
        batch_count := batch_count + 1;
        item_count := 0;
        batch_complete := 1;
    ELSE
        batch_complete := 0;
    END_IF;
END_PROGRAM
"#;
    // State: item_count(2) + batch_count(2) + batch_complete(2) = 6 bytes
    // After 99 scans: item=99, batch=0, complete=0
    let state = jit_run(source, "batchcounter_scan", 6, 99);
    assert_eq!(read_i16(&state, 0), 99, "99 items counted");
    assert_eq!(read_i16(&state, 2), 0, "no batch yet");
    assert_eq!(read_i16(&state, 4), 0, "not complete");

    // After 100 scans: batch completes
    let state = jit_run(source, "batchcounter_scan", 6, 100);
    assert_eq!(read_i16(&state, 0), 0, "counter reset");
    assert_eq!(read_i16(&state, 2), 1, "first batch done");
    assert_eq!(read_i16(&state, 4), 1, "complete signal");

    // After 250 scans: 2 complete batches + 50 items
    let state = jit_run(source, "batchcounter_scan", 6, 250);
    assert_eq!(read_i16(&state, 0), 50, "50 items into 3rd batch");
    assert_eq!(read_i16(&state, 2), 2, "2 batches complete");
}

#[test]
fn moving_average_filter() {
    // Simple 4-point moving average using array
    let source = r#"
PROGRAM MovingAvg
VAR
    buffer : ARRAY[0..3] OF INT;
    index : INT := 0;
    sum : INT := 0;
    average : INT := 0;
    input_val : INT := 0;
    i : INT;
END_VAR
    input_val := input_val + 10;
    buffer[index] := input_val;
    index := index + 1;
    IF index >= 4 THEN
        index := 0;
    END_IF;

    sum := 0;
    FOR i := 0 TO 3 DO
        sum := sum + buffer[i];
    END_FOR;
    average := sum / 4;
END_PROGRAM
"#;
    // State: buffer[4](8) + index(2) + sum(2) + average(2) + input_val(2) + i(2) = 18 bytes
    // After 4 scans: buffer=[10,20,30,40], sum=100, average=25
    let state = jit_run(source, "movingavg_scan", 18, 4);
    let average = read_i16(&state, 12);
    assert_eq!(average, 25, "average of [10,20,30,40] = {average}");

    // After 5 scans: buffer=[50,20,30,40], wraps around, sum=140, avg=35
    let state = jit_run(source, "movingavg_scan", 18, 5);
    let average = read_i16(&state, 12);
    assert_eq!(average, 35, "average after 5 scans = {average}");
}

#[test]
fn alarm_priority_logic() {
    // Multiple alarm conditions with priority encoding
    let source = r#"
FUNCTION PriorityEncode : INT
VAR_INPUT
    critical : INT;
    high : INT;
    medium : INT;
    low : INT;
END_VAR
    IF critical <> 0 THEN
        PriorityEncode := 4;
    ELSIF high <> 0 THEN
        PriorityEncode := 3;
    ELSIF medium <> 0 THEN
        PriorityEncode := 2;
    ELSIF low <> 0 THEN
        PriorityEncode := 1;
    ELSE
        PriorityEncode := 0;
    END_IF;
END_FUNCTION

PROGRAM AlarmSystem
VAR
    temp_high : INT := 0;
    pressure_critical : INT := 0;
    vibration_medium : INT := 0;
    level_low : INT := 0;
    highest_priority : INT := 0;
END_VAR
    temp_high := 1;
    vibration_medium := 1;
    highest_priority := PriorityEncode(pressure_critical, temp_high, vibration_medium, level_low);
END_PROGRAM
"#;
    // State: 5 * INT(2) = 10 bytes
    // temp_high=1, pressure_critical=0, vibration_medium=1
    // Priority: critical=0, high=1 → result=3
    let state = jit_run(source, "alarmsystem_scan", 10, 1);
    let priority = read_i16(&state, 8);
    assert_eq!(priority, 3, "highest priority should be 3 (high), got {priority}");
}

#[test]
fn recipe_selector() {
    // Select recipe parameters based on product type
    let source = r#"
PROGRAM RecipeSelector
VAR
    product_type : INT := 0;
    speed : INT := 0;
    temperature : INT := 0;
    pressure : INT := 0;
END_VAR
    product_type := product_type + 1;
    IF product_type > 3 THEN product_type := 1; END_IF;

    CASE product_type OF
        1:
            speed := 100;
            temperature := 180;
            pressure := 50;
        2:
            speed := 200;
            temperature := 220;
            pressure := 80;
        3:
            speed := 50;
            temperature := 150;
            pressure := 30;
    END_CASE;
END_PROGRAM
"#;
    // State: 4 * INT(2) = 8 bytes
    // Scan 1: product=1, recipe 1
    let state = jit_run(source, "recipeselector_scan", 8, 1);
    assert_eq!(read_i16(&state, 2), 100, "recipe 1 speed");
    assert_eq!(read_i16(&state, 4), 180, "recipe 1 temp");

    // Scan 2: product=2, recipe 2
    let state = jit_run(source, "recipeselector_scan", 8, 2);
    assert_eq!(read_i16(&state, 2), 200, "recipe 2 speed");
    assert_eq!(read_i16(&state, 4), 220, "recipe 2 temp");

    // Scan 4: wraps to product=1
    let state = jit_run(source, "recipeselector_scan", 8, 4);
    assert_eq!(read_i16(&state, 0), 1, "wraps to product 1");
    assert_eq!(read_i16(&state, 2), 100, "back to recipe 1");
}

#[test]
fn math_in_control_loop() {
    // Use SQRT, ABS, MIN, MAX in a control calculation
    let source = r#"
PROGRAM MathControl
VAR
    input_val : REAL := 16.0;
    root : REAL;
    clamped : REAL;
    abs_val : REAL;
END_VAR
    root := SQRT(input_val);
    abs_val := ABS(-25.0);
    clamped := LIMIT(0.0, root, 3.0);
END_PROGRAM
"#;
    // State: 4 * REAL(4) = 16 bytes
    let state = jit_run(source, "mathcontrol_scan", 16, 1);
    // Struct order: input_val(4), root(4), clamped(4), abs_val(4)
    let root = read_f32(&state, 4);
    let clamped = read_f32(&state, 8);
    let abs_val = read_f32(&state, 12);
    assert!((root - 4.0).abs() < 0.1, "SQRT(16) = {root}");
    assert!((abs_val - 25.0).abs() < 0.1, "ABS(-25) = {abs_val}");
    assert!((clamped - 3.0).abs() < 0.1, "LIMIT(0, 4, 3) = {clamped}");
}

#[test]
fn conveyor_belt_sequence() {
    // Multi-step startup sequence with interlocks
    let source = r#"
PROGRAM ConveyorStartup
VAR
    step : INT := 0;
    timer : INT := 0;
    motor_on : INT := 0;
    belt_on : INT := 0;
    ready : INT := 0;
END_VAR
    timer := timer + 1;
    CASE step OF
        0: (* IDLE *)
            motor_on := 0; belt_on := 0; ready := 0;
            step := 1; timer := 0;
        1: (* START MOTOR *)
            motor_on := 1;
            IF timer >= 5 THEN
                step := 2; timer := 0;
            END_IF;
        2: (* START BELT *)
            belt_on := 1;
            IF timer >= 3 THEN
                step := 3; timer := 0;
            END_IF;
        3: (* RUNNING *)
            ready := 1;
    END_CASE;
END_PROGRAM
"#;
    // State: step(2) + timer(2) + motor_on(2) + belt_on(2) + ready(2) = 10 bytes
    // Scan 2: step transitions to 1 on scan 1, outputs set on scan 2
    let state = jit_run(source, "conveyorstartup_scan", 10, 2);
    assert_eq!(read_i16(&state, 0), 1, "step 1: motor starting");
    assert_eq!(read_i16(&state, 4), 1, "motor on");
    assert_eq!(read_i16(&state, 6), 0, "belt off");

    // Scan 7: motor ran for 5 scans, now step 2 with belt on
    let state = jit_run(source, "conveyorstartup_scan", 10, 7);
    assert_eq!(read_i16(&state, 0), 2, "step 2: belt starting");
    assert_eq!(read_i16(&state, 6), 1, "belt on");

    // Scan 10: belt ran for 3, now step 3 running
    let state = jit_run(source, "conveyorstartup_scan", 10, 10);
    assert_eq!(read_i16(&state, 0), 3, "step 3: running");
    assert_eq!(read_i16(&state, 8), 1, "ready signal");
}
