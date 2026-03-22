(* Water Treatment Plant Control *)
(* Multiple tanks, pumps, valves, level/flow sensors *)
(* A realistic industrial PLC program with multiple FBs, *)
(* process control, alarm handling, and Modbus-mappable state. *)

FUNCTION_BLOCK TankLevel
VAR_INPUT
    inlet_flow : REAL;      (* L/min *)
    outlet_flow : REAL;     (* L/min *)
    max_capacity : REAL;    (* Liters *)
END_VAR
VAR_OUTPUT
    level : REAL;           (* Current level in Liters *)
    level_pct : INT;        (* 0-100% *)
    high_alarm : BOOL;
    low_alarm : BOOL;
END_VAR
VAR
    high_limit : REAL := 90.0;  (* % *)
    low_limit : REAL := 10.0;   (* % *)
END_VAR
    level := level + (inlet_flow - outlet_flow) * 0.001;  (* dt = 1ms sim *)
    IF level < 0.0 THEN level := 0.0; END_IF;
    IF level > max_capacity THEN level := max_capacity; END_IF;
    level_pct := REAL_TO_INT(level * 100.0 / max_capacity);
    high_alarm := level_pct > REAL_TO_INT(high_limit);
    low_alarm := level_pct < REAL_TO_INT(low_limit);
END_FUNCTION_BLOCK

FUNCTION_BLOCK PumpControl
VAR_INPUT
    enable : BOOL;
    speed_setpoint : INT;   (* 0-100% *)
    fault_reset : BOOL;
END_VAR
VAR_OUTPUT
    running : BOOL;
    actual_speed : INT;
    fault : BOOL;
    runtime_hrs : INT;
END_VAR
VAR
    runtime_scans : INT := 0;
END_VAR
    IF fault_reset THEN fault := FALSE; END_IF;
    IF enable AND NOT fault THEN
        running := TRUE;
        actual_speed := speed_setpoint;
        runtime_scans := runtime_scans + 1;
        runtime_hrs := runtime_scans / 3600;  (* assuming 1 scan/sec *)
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
    position : INT;   (* 0=closed, 100=open *)
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
    (* Process state *)
    mode : INT := 0;        (* 0=stop, 1=run, 2=flush, 3=alarm *)
    cycle_count : INT := 0;

    (* Tanks *)
    raw_tank : TankLevel;
    clean_tank : TankLevel;

    (* Pumps *)
    inlet_pump : PumpControl;
    transfer_pump : PumpControl;
    outlet_pump : PumpControl;

    (* Valves *)
    inlet_valve : ValveControl;
    outlet_valve : ValveControl;
    drain_valve : ValveControl;

    (* Setpoints — these are Modbus-writable *)
    raw_tank_sp : INT := 50;     (* target level % *)
    clean_tank_sp : INT := 70;   (* target level % *)
    inlet_speed : INT := 60;     (* pump speed % *)
    transfer_speed : INT := 40;
    outlet_speed : INT := 30;

    (* Status — these are Modbus-readable *)
    system_ok : BOOL := TRUE;
    alarm_count : INT := 0;
    total_flow : INT := 0;

    (* Track mode changes for debug output *)
    prev_mode : INT := -1;
    started : BOOL := FALSE;
END_VAR
    (* Startup message — first scan only *)
    IF NOT started THEN
        PRINT('Water treatment PLC started');
        started := TRUE;
    END_IF;

    cycle_count := cycle_count + 1;

    CASE mode OF
        0: (* STOPPED *)
            inlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            transfer_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            outlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            inlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            outlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            drain_valve(open_cmd := FALSE, close_cmd := TRUE);

        1: (* RUNNING *)
            (* Control raw water tank level *)
            IF raw_tank.level_pct < raw_tank_sp THEN
                inlet_pump(enable := TRUE, speed_setpoint := inlet_speed, fault_reset := FALSE);
                inlet_valve(open_cmd := TRUE, close_cmd := FALSE);
            ELSE
                inlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
                inlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            END_IF;

            (* Transfer from raw to clean *)
            IF raw_tank.level_pct > 20 AND clean_tank.level_pct < clean_tank_sp THEN
                transfer_pump(enable := TRUE, speed_setpoint := transfer_speed, fault_reset := FALSE);
            ELSE
                transfer_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            END_IF;

            (* Outlet pump *)
            IF clean_tank.level_pct > 30 THEN
                outlet_pump(enable := TRUE, speed_setpoint := outlet_speed, fault_reset := FALSE);
                outlet_valve(open_cmd := TRUE, close_cmd := FALSE);
            ELSE
                outlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
                outlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            END_IF;

            drain_valve(open_cmd := FALSE, close_cmd := TRUE);

        2: (* FLUSH *)
            inlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            transfer_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            outlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            inlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            outlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            drain_valve(open_cmd := TRUE, close_cmd := FALSE);

        3: (* ALARM *)
            inlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            transfer_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            outlet_pump(enable := FALSE, speed_setpoint := 0, fault_reset := FALSE);
            inlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            outlet_valve(open_cmd := FALSE, close_cmd := TRUE);
            drain_valve(open_cmd := FALSE, close_cmd := TRUE);
    END_CASE;

    (* Debug: report mode changes *)
    IF mode <> prev_mode THEN
        IF mode = 0 THEN
            PRINT('Mode changed to STOP');
        ELSIF mode = 1 THEN
            PRINT('Mode changed to RUN');
        ELSIF mode = 2 THEN
            PRINT('Mode changed to FLUSH');
        ELSIF mode = 3 THEN
            PRINT('Mode changed to ALARM');
        END_IF;
        prev_mode := mode;
    END_IF;

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

    (* Alarm handling *)
    IF raw_tank.high_alarm OR clean_tank.high_alarm THEN
        IF alarm_count = 0 THEN
            PRINT('ALARM: tank high level detected');
        END_IF;
        alarm_count := alarm_count + 1;
        IF alarm_count > 100 THEN
            PRINT('ALARM: threshold exceeded, entering alarm mode');
            mode := 3;
            system_ok := FALSE;
        END_IF;
    ELSE
        alarm_count := 0;
    END_IF;

    total_flow := total_flow + outlet_pump.actual_speed;
END_PROGRAM
