(* ═══════════════════════════════════════════════════════════ *)
(*  Water Treatment Plant — Industrial PLC Program            *)
(*  Registers 0-39 are SCADA-accessible via Modbus            *)
(* ═══════════════════════════════════════════════════════════ *)

(* ── Function Blocks ── *)

FUNCTION_BLOCK TankLevel
VAR_INPUT
    inlet_flow : REAL;
    outlet_flow : REAL;
    max_capacity : REAL;
END_VAR
VAR_OUTPUT
    level : REAL;
    level_pct : INT;
    high_alarm : BOOL;
    low_alarm : BOOL;
END_VAR
VAR
    high_sp : REAL := 90.0;
    low_sp : REAL := 10.0;
END_VAR
    level := level + (inlet_flow - outlet_flow) * 0.001;
    IF level < 0.0 THEN level := 0.0; END_IF;
    IF level > max_capacity THEN level := max_capacity; END_IF;
    level_pct := REAL_TO_INT(level * 100.0 / max_capacity);
    high_alarm := level_pct > REAL_TO_INT(high_sp);
    low_alarm := level_pct < REAL_TO_INT(low_sp);
END_FUNCTION_BLOCK

FUNCTION_BLOCK PumpControl
VAR_INPUT
    cmd_start : BOOL;
    cmd_stop : BOOL;
    speed_sp : INT;
    fault_reset : BOOL;
END_VAR
VAR_OUTPUT
    running : BOOL;
    speed : INT;
    fault : BOOL;
    hours : INT;
END_VAR
VAR
    scans : INT := 0;
END_VAR
    IF fault_reset THEN fault := FALSE; END_IF;
    IF cmd_stop THEN running := FALSE; END_IF;
    IF cmd_start AND NOT fault THEN running := TRUE; END_IF;
    IF running THEN
        speed := speed_sp;
        scans := scans + 1;
        hours := scans / 3600;
    ELSE
        speed := 0;
    END_IF;
END_FUNCTION_BLOCK

FUNCTION_BLOCK ValveControl
VAR_INPUT
    cmd_open : BOOL;
    cmd_close : BOOL;
END_VAR
VAR_OUTPUT
    position : INT;
    opened : BOOL;
    closed : BOOL;
END_VAR
    IF cmd_open AND NOT cmd_close THEN
        IF position < 100 THEN position := position + 5; END_IF;
    ELSIF cmd_close AND NOT cmd_open THEN
        IF position > 0 THEN position := position - 5; END_IF;
    END_IF;
    opened := position >= 100;
    closed := position <= 0;
END_FUNCTION_BLOCK


(* ── Main Program ── *)

PROGRAM WaterTreatment
VAR
    (* ═══════════════════════════════════════════ *)
    (* COMMANDS — write from SCADA (R0-R9)        *)
    (* ═══════════════════════════════════════════ *)
    cmd_start : INT := 0;         (* R0  write 1 to start *)
    cmd_stop : INT := 0;          (* R1  write 1 to stop *)
    cmd_flush : INT := 0;         (* R2  write 1 to flush *)
    cmd_alarm_ack : INT := 0;     (* R3  write 1 to acknowledge alarms *)
    cmd_alarm_reset : INT := 0;   (* R4  write 1 to reset faults *)
    sp_raw_tank : INT := 50;      (* R5  raw tank level setpoint % *)
    sp_clean_tank : INT := 70;    (* R6  clean tank level setpoint % *)
    sp_inlet_pump : INT := 60;    (* R7  inlet pump speed % *)
    sp_transfer_pump : INT := 40; (* R8  transfer pump speed % *)
    sp_outlet_pump : INT := 30;   (* R9  outlet pump speed % *)

    (* ═══════════════════════════════════════════ *)
    (* STATUS — read from SCADA (R10-R19)         *)
    (* ═══════════════════════════════════════════ *)
    running : INT := 0;           (* R10 process running *)
    stopped : INT := 1;           (* R11 process stopped *)
    flushing : INT := 0;          (* R12 flush in progress *)
    faulted : INT := 0;           (* R13 fault/alarm state *)
    cycle_count : INT := 0;       (* R14 scan counter *)
    raw_level : INT := 0;         (* R15 raw tank level % *)
    clean_level : INT := 0;       (* R16 clean tank level % *)
    system_ok : INT := 1;         (* R17 all systems healthy *)
    alarm_word : INT := 0;        (* R18 alarm bitmask *)
    total_flow : INT := 0;        (* R19 cumulative output *)

    (* ═══════════════════════════════════════════ *)
    (* EQUIPMENT STATUS (R20-R31)                 *)
    (* ═══════════════════════════════════════════ *)
    inlet_pump_run : INT := 0;    (* R20 *)
    inlet_pump_speed : INT := 0;  (* R21 *)
    inlet_pump_fault : INT := 0;  (* R22 *)
    xfer_pump_run : INT := 0;     (* R23 *)
    xfer_pump_speed : INT := 0;   (* R24 *)
    xfer_pump_fault : INT := 0;   (* R25 *)
    outlet_pump_run : INT := 0;   (* R26 *)
    outlet_pump_speed : INT := 0; (* R27 *)
    outlet_pump_fault : INT := 0; (* R28 *)
    inlet_vlv_pos : INT := 0;     (* R29 *)
    outlet_vlv_pos : INT := 0;    (* R30 *)
    drain_vlv_pos : INT := 0;     (* R31 *)

    (* ═══════════════════════════════════════════ *)
    (* ALARMS (R32-R39)                           *)
    (* ═══════════════════════════════════════════ *)
    alm_raw_high : INT := 0;      (* R32 raw tank high level *)
    alm_raw_low : INT := 0;       (* R33 raw tank low level *)
    alm_clean_high : INT := 0;    (* R34 clean tank high level *)
    alm_clean_low : INT := 0;     (* R35 clean tank low level *)
    alm_inlet_fault : INT := 0;   (* R36 inlet pump fault *)
    alm_xfer_fault : INT := 0;    (* R37 transfer pump fault *)
    alm_outlet_fault : INT := 0;  (* R38 outlet pump fault *)
    alm_count : INT := 0;         (* R39 total active alarms *)

    (* ═══════════════════════════════════════════ *)
    (* INTERNAL — FB instances after registers     *)
    (* ═══════════════════════════════════════════ *)
    raw_tank : TankLevel;
    clean_tank : TankLevel;
    p_inlet : PumpControl;
    p_transfer : PumpControl;
    p_outlet : PumpControl;
    v_inlet : ValveControl;
    v_outlet : ValveControl;
    v_drain : ValveControl;

    prev_running : INT := -1;
    init_done : INT := 0;
END_VAR

    (* ── Startup ── *)
    IF init_done = 0 THEN
        PRINT('Water Treatment PLC started');
        init_done := 1;
    END_IF;
    cycle_count := cycle_count + 1;

    (* ── Process commands (edge-triggered, auto-clear) ── *)
    IF cmd_start = 1 THEN
        cmd_start := 0;
        IF faulted = 0 THEN
            running := 1; stopped := 0; flushing := 0;
            PRINT('CMD: Process START');
        END_IF;
    END_IF;

    IF cmd_stop = 1 THEN
        cmd_stop := 0;
        running := 0; stopped := 1; flushing := 0;
        PRINT('CMD: Process STOP');
    END_IF;

    IF cmd_flush = 1 THEN
        cmd_flush := 0;
        running := 0; stopped := 0; flushing := 1;
        PRINT('CMD: FLUSH started');
    END_IF;

    IF cmd_alarm_ack = 1 THEN
        cmd_alarm_ack := 0;
        PRINT('CMD: Alarms acknowledged');
    END_IF;

    IF cmd_alarm_reset = 1 THEN
        cmd_alarm_reset := 0;
        faulted := 0;
        system_ok := 1;
        p_inlet(cmd_start := FALSE, cmd_stop := FALSE, speed_sp := 0, fault_reset := TRUE);
        p_transfer(cmd_start := FALSE, cmd_stop := FALSE, speed_sp := 0, fault_reset := TRUE);
        p_outlet(cmd_start := FALSE, cmd_stop := FALSE, speed_sp := 0, fault_reset := TRUE);
        PRINT('CMD: Faults reset');
    END_IF;

    (* ── Process control ── *)
    IF running = 1 THEN
        (* Inlet: fill raw tank to setpoint *)
        IF raw_level < sp_raw_tank THEN
            p_inlet(cmd_start := TRUE, cmd_stop := FALSE, speed_sp := sp_inlet_pump, fault_reset := FALSE);
            v_inlet(cmd_open := TRUE, cmd_close := FALSE);
        ELSE
            p_inlet(cmd_start := FALSE, cmd_stop := TRUE, speed_sp := 0, fault_reset := FALSE);
            v_inlet(cmd_open := FALSE, cmd_close := TRUE);
        END_IF;

        (* Transfer: move water from raw to clean *)
        IF raw_level > 20 AND clean_level < sp_clean_tank THEN
            p_transfer(cmd_start := TRUE, cmd_stop := FALSE, speed_sp := sp_transfer_pump, fault_reset := FALSE);
        ELSE
            p_transfer(cmd_start := FALSE, cmd_stop := TRUE, speed_sp := 0, fault_reset := FALSE);
        END_IF;

        (* Outlet: distribute clean water *)
        IF clean_level > 30 THEN
            p_outlet(cmd_start := TRUE, cmd_stop := FALSE, speed_sp := sp_outlet_pump, fault_reset := FALSE);
            v_outlet(cmd_open := TRUE, cmd_close := FALSE);
        ELSE
            p_outlet(cmd_start := FALSE, cmd_stop := TRUE, speed_sp := 0, fault_reset := FALSE);
            v_outlet(cmd_open := FALSE, cmd_close := TRUE);
        END_IF;

        v_drain(cmd_open := FALSE, cmd_close := TRUE);

    ELSIF flushing = 1 THEN
        (* Flush: drain everything *)
        p_inlet(cmd_start := FALSE, cmd_stop := TRUE, speed_sp := 0, fault_reset := FALSE);
        p_transfer(cmd_start := FALSE, cmd_stop := TRUE, speed_sp := 0, fault_reset := FALSE);
        p_outlet(cmd_start := FALSE, cmd_stop := TRUE, speed_sp := 0, fault_reset := FALSE);
        v_inlet(cmd_open := FALSE, cmd_close := TRUE);
        v_outlet(cmd_open := FALSE, cmd_close := TRUE);
        v_drain(cmd_open := TRUE, cmd_close := FALSE);

        (* Auto-stop flush when tanks are empty *)
        IF raw_level = 0 AND clean_level = 0 THEN
            flushing := 0; stopped := 1;
            PRINT('Flush complete');
        END_IF;

    ELSE
        (* Stopped / faulted *)
        p_inlet(cmd_start := FALSE, cmd_stop := TRUE, speed_sp := 0, fault_reset := FALSE);
        p_transfer(cmd_start := FALSE, cmd_stop := TRUE, speed_sp := 0, fault_reset := FALSE);
        p_outlet(cmd_start := FALSE, cmd_stop := TRUE, speed_sp := 0, fault_reset := FALSE);
        v_inlet(cmd_open := FALSE, cmd_close := TRUE);
        v_outlet(cmd_open := FALSE, cmd_close := TRUE);
        v_drain(cmd_open := FALSE, cmd_close := TRUE);
    END_IF;

    (* ── Tank simulation ── *)
    raw_tank(
        inlet_flow := INT_TO_REAL(p_inlet.speed) * 0.5,
        outlet_flow := INT_TO_REAL(p_transfer.speed) * 0.3,
        max_capacity := 1000.0
    );
    clean_tank(
        inlet_flow := INT_TO_REAL(p_transfer.speed) * 0.3,
        outlet_flow := INT_TO_REAL(p_outlet.speed) * 0.2,
        max_capacity := 500.0
    );

    (* ── Copy to SCADA registers ── *)
    raw_level := raw_tank.level_pct;
    clean_level := clean_tank.level_pct;

    inlet_pump_run := BOOL_TO_INT(p_inlet.running);
    inlet_pump_speed := p_inlet.speed;
    inlet_pump_fault := BOOL_TO_INT(p_inlet.fault);

    xfer_pump_run := BOOL_TO_INT(p_transfer.running);
    xfer_pump_speed := p_transfer.speed;
    xfer_pump_fault := BOOL_TO_INT(p_transfer.fault);

    outlet_pump_run := BOOL_TO_INT(p_outlet.running);
    outlet_pump_speed := p_outlet.speed;
    outlet_pump_fault := BOOL_TO_INT(p_outlet.fault);

    inlet_vlv_pos := v_inlet.position;
    outlet_vlv_pos := v_outlet.position;
    drain_vlv_pos := v_drain.position;

    total_flow := total_flow + p_outlet.speed;

    (* ── Alarms ── *)
    alm_raw_high := BOOL_TO_INT(raw_tank.high_alarm);
    alm_raw_low := BOOL_TO_INT(raw_tank.low_alarm);
    alm_clean_high := BOOL_TO_INT(clean_tank.high_alarm);
    alm_clean_low := BOOL_TO_INT(clean_tank.low_alarm);
    alm_inlet_fault := BOOL_TO_INT(p_inlet.fault);
    alm_xfer_fault := BOOL_TO_INT(p_transfer.fault);
    alm_outlet_fault := BOOL_TO_INT(p_outlet.fault);

    (* Alarm word: bitmask of all active alarms *)
    alarm_word := alm_raw_high + alm_raw_low * 2 + alm_clean_high * 4
                + alm_clean_low * 8 + alm_inlet_fault * 16
                + alm_xfer_fault * 32 + alm_outlet_fault * 64;

    alm_count := alm_raw_high + alm_raw_low + alm_clean_high + alm_clean_low
               + alm_inlet_fault + alm_xfer_fault + alm_outlet_fault;

    (* Trip to fault if too many alarms persist *)
    IF running = 1 AND alm_count > 0 THEN
        IF alarm_word > 0 THEN
            faulted := 1;
            running := 0;
            system_ok := 0;
            PRINT('FAULT: alarms active, process stopped');
        END_IF;
    END_IF;

END_PROGRAM
