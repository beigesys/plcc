(* Water Treatment Plant Control *)
(* Registers 0-19 are SCADA-accessible via Modbus *)

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
    high_limit : REAL := 90.0;
    low_limit : REAL := 10.0;
END_VAR
    level := level + (inlet_flow - outlet_flow) * 0.001;
    IF level < 0.0 THEN level := 0.0; END_IF;
    IF level > max_capacity THEN level := max_capacity; END_IF;
    level_pct := REAL_TO_INT(level * 100.0 / max_capacity);
    high_alarm := level_pct > REAL_TO_INT(high_limit);
    low_alarm := level_pct < REAL_TO_INT(low_limit);
END_FUNCTION_BLOCK

FUNCTION_BLOCK PumpControl
VAR_INPUT
    enable : BOOL;
    speed_setpoint : INT;
    fault_reset : BOOL;
END_VAR
VAR_OUTPUT
    running : BOOL;
    actual_speed : INT;
    fault : BOOL;
END_VAR
VAR
    runtime_scans : INT := 0;
END_VAR
    IF fault_reset THEN fault := FALSE; END_IF;
    IF enable AND NOT fault THEN
        running := TRUE;
        actual_speed := speed_setpoint;
        runtime_scans := runtime_scans + 1;
    ELSE
        running := FALSE;
        actual_speed := 0;
    END_IF;
END_FUNCTION_BLOCK

FUNCTION_BLOCK ValveControl
VAR_INPUT
    open_cmd : BOOL;
    close_cmd : BOOL;
END_VAR
VAR_OUTPUT
    position : INT;
    is_open : BOOL;
    is_closed : BOOL;
END_VAR
    IF open_cmd AND NOT close_cmd THEN
        IF position < 100 THEN position := position + 5; END_IF;
    ELSIF close_cmd AND NOT open_cmd THEN
        IF position > 0 THEN position := position - 5; END_IF;
    END_IF;
    is_open := position >= 100;
    is_closed := position <= 0;
END_FUNCTION_BLOCK

PROGRAM WaterTreatment
VAR
    (* ══════════════════════════════════════════════ *)
    (* SCADA REGISTERS — first in struct = predictable offsets *)
    (* Register = variable index, each INT = 2 bytes *)
    (* ══════════════════════════════════════════════ *)

    (* Commands — writable from SCADA *)
    mode : INT := 0;              (* R0  — 0=stop 1=run 2=flush 3=alarm *)
    alarm_reset : INT := 0;       (* R1  — write 1 to reset alarm *)
    raw_tank_sp : INT := 50;      (* R2  — raw tank setpoint % *)
    clean_tank_sp : INT := 70;    (* R3  — clean tank setpoint % *)
    inlet_speed : INT := 60;      (* R4  — inlet pump speed % *)
    transfer_speed : INT := 40;   (* R5  — transfer pump speed % *)
    outlet_speed : INT := 30;     (* R6  — outlet pump speed % *)

    (* Status — readable from SCADA *)
    cycle_count : INT := 0;       (* R7  — scan counter *)
    raw_level : INT := 0;         (* R8  — raw tank level % *)
    clean_level : INT := 0;       (* R9  — clean tank level % *)
    inlet_running : INT := 0;     (* R10 — inlet pump running *)
    transfer_running : INT := 0;  (* R11 — transfer pump running *)
    outlet_running : INT := 0;    (* R12 — outlet pump running *)
    inlet_valve_pos : INT := 0;   (* R13 — inlet valve position % *)
    outlet_valve_pos : INT := 0;  (* R14 — outlet valve position % *)
    drain_valve_pos : INT := 0;   (* R15 — drain valve position % *)
    alarm_active : INT := 0;      (* R16 — alarm active flag *)
    alarm_count : INT := 0;       (* R17 — alarm counter *)
    system_ok : INT := 1;         (* R18 — system healthy *)
    total_flow : INT := 0;        (* R19 — total output flow *)

    (* ══════════════════════════════════════════════ *)
    (* Internal — FB instances after SCADA registers  *)
    (* ══════════════════════════════════════════════ *)
    raw_tank : TankLevel;
    clean_tank : TankLevel;
    inlet_pump : PumpControl;
    transfer_pump : PumpControl;
    outlet_pump : PumpControl;
    inlet_valve : ValveControl;
    outlet_valve : ValveControl;
    drain_valve : ValveControl;

    prev_mode : INT := -1;
    started : INT := 0;
END_VAR
    IF started = 0 THEN
        PRINT('Water treatment PLC started');
        started := 1;
    END_IF;

    cycle_count := cycle_count + 1;

    (* Alarm reset from SCADA *)
    IF alarm_reset = 1 THEN
        alarm_reset := 0;
        alarm_count := 0;
        alarm_active := 0;
        system_ok := 1;
        IF mode = 3 THEN
            PRINT('Alarm reset — returning to STOP');
            mode := 0;
        END_IF;
    END_IF;

    (* Mode change debug *)
    IF mode <> prev_mode THEN
        IF mode = 0 THEN PRINT('Mode: STOP');
        ELSIF mode = 1 THEN PRINT('Mode: RUN');
        ELSIF mode = 2 THEN PRINT('Mode: FLUSH');
        ELSIF mode = 3 THEN PRINT('Mode: ALARM');
        END_IF;
        prev_mode := mode;
    END_IF;

    CASE mode OF
        0: (* STOPPED *)
            inlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            transfer_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            outlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            inlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            outlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            drain_valve(open_cmd := FALSE, close_cmd := TRUE);

        1: (* RUNNING *)
            IF raw_level < raw_tank_sp THEN
                inlet_pump(enable := TRUE, speed_setpoint := inlet_speed, fault_reset := FALSE);
                inlet_valve(open_cmd := TRUE, close_cmd := FALSE);
            ELSE
                inlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
                inlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            END_IF;

            IF raw_level > 20 AND clean_level < clean_tank_sp THEN
                transfer_pump(enable := TRUE, speed_setpoint := transfer_speed, fault_reset := FALSE);
            ELSE
                transfer_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            END_IF;

            IF clean_level > 30 THEN
                outlet_pump(enable := TRUE, speed_setpoint := outlet_speed, fault_reset := FALSE);
                outlet_valve(open_cmd := TRUE, close_cmd := FALSE);
            ELSE
                outlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
                outlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            END_IF;

            drain_valve(open_cmd := FALSE, close_cmd := TRUE);

        2: (* FLUSH — drain everything *)
            inlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            transfer_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            outlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            inlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            outlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            drain_valve(open_cmd := TRUE, close_cmd := FALSE);

        3: (* ALARM — everything off *)
            inlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            transfer_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            outlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            inlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            outlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            drain_valve(open_cmd := FALSE, close_cmd := TRUE);
    END_CASE;

    (* Update tank levels *)
    raw_tank(
        inlet_flow := INT_TO_REAL(inlet_pump.actual_speed) * 0.5,
        outlet_flow := INT_TO_REAL(transfer_pump.actual_speed) * 0.3,
        max_capacity := 1000.0
    );
    clean_tank(
        inlet_flow := INT_TO_REAL(transfer_pump.actual_speed) * 0.3,
        outlet_flow := INT_TO_REAL(outlet_pump.actual_speed) * 0.2,
        max_capacity := 500.0
    );

    (* Copy status to SCADA registers *)
    raw_level := raw_tank.level_pct;
    clean_level := clean_tank.level_pct;
    inlet_running := BOOL_TO_INT(inlet_pump.running);
    transfer_running := BOOL_TO_INT(transfer_pump.running);
    outlet_running := BOOL_TO_INT(outlet_pump.running);
    inlet_valve_pos := inlet_valve.position;
    outlet_valve_pos := outlet_valve.position;
    drain_valve_pos := drain_valve.position;
    total_flow := total_flow + outlet_pump.actual_speed;

    (* Alarm handling — only in RUN mode *)
    IF mode = 1 THEN
        IF raw_tank.high_alarm OR clean_tank.high_alarm THEN
            IF alarm_count = 0 THEN
                PRINT('ALARM: tank high level');
            END_IF;
            alarm_count := alarm_count + 1;
            alarm_active := 1;
            IF alarm_count > 100 THEN
                PRINT('ALARM: entering alarm mode');
                mode := 3;
                system_ok := 0;
            END_IF;
        ELSE
            alarm_count := 0;
            alarm_active := 0;
        END_IF;
    END_IF;
END_PROGRAM
