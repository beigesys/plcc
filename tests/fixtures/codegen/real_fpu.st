(* REAL arithmetic for the FPU check (crates/plcc-cli/tests/fpu.rs) and the
   Opta hardware check: results land in %M, so `img` shows them.
   %MW10 = REAL_TO_INT(((1.5 + 2.25) * (1.5 * 2.25) + 0.5) * 100) = 1316,
   %MW11 = REAL_TO_INT(SQRT(2.0) * 1000) = 1414. *)
FUNCTION Scale : REAL
VAR_INPUT
    raw : REAL;
    gain : REAL;
    offset : REAL;
END_VAR
    Scale := raw * gain + offset;
END_FUNCTION

PROGRAM Main
VAR
    a AT %MD0 : REAL := 1.5;
    b AT %MD1 : REAL := 2.25;
    sum AT %MD2 : REAL;
    prod AT %MD3 : REAL;
    q AT %MD4 : REAL;
    r AT %MW10 : INT;
    s AT %MW11 : INT;
END_VAR
    sum := a + b;
    prod := a * b;
    q := Scale(raw := sum, gain := prod, offset := 0.5);
    r := REAL_TO_INT(q * 100.0);
    s := REAL_TO_INT(SQRT(2.0) * 1000.0);
END_PROGRAM
