FUNCTION_BLOCK PID
VAR_INPUT
    setpoint : REAL;
    measured : REAL;
    kp : REAL := 1.0;
    ki : REAL := 0.1;
    kd : REAL := 0.05;
    dt : REAL := 0.01;
    out_min : REAL := -100.0;
    out_max : REAL := 100.0;
END_VAR
VAR_OUTPUT
    output : REAL;
END_VAR
VAR
    err : REAL;
    prev_err : REAL := 0.0;
    integral : REAL := 0.0;
    derivative : REAL;
END_VAR
    err := setpoint - measured;
    integral := integral + err * dt;
    derivative := (err - prev_err) / dt;
    output := kp * err + ki * integral + kd * derivative;

    (* Clamp output *)
    IF output > out_max THEN
        output := out_max;
    ELSIF output < out_min THEN
        output := out_min;
    END_IF;

    prev_err := err;
END_FUNCTION_BLOCK

PROGRAM PIDController
VAR
    pid_inst : PID;
    sensor_value : REAL := 0.0;
    target_value : REAL := 50.0;
    control_output : REAL;
END_VAR
    pid_inst(setpoint := target_value, measured := sensor_value);
    control_output := pid_inst.output;
END_PROGRAM
