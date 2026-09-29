// SPDX-License-Identifier: MPL-2.0

//! FOR loops whose end value is at (or whose step passes) the limit of the control
//! variable's type. As in CODESYS, the increment is `i := i + BY` in the variable's
//! own type — it wraps — followed by the usual bound test, so `FOR b := 250 TO 255`
//! on a BYTE never ends: 255 + 1 is 0, which is still <= 255. The CODESYS help
//! (ST statement FOR) warns about exactly this: "If the end value of the counter is
//! equal to the upper limit of the data type of the counter, then an infinite loop
//! results."
//!
//! An endless loop cannot be run to completion, so each one counts its iterations
//! and EXITs at 300: reaching 300 proves it did not stop at the type limit, and the
//! control variable's value then proves it wrapped.

mod common;
use common::{compile_error, run, run_o3};

const SRC: &str = r#"
PROGRAM p
VAR
  i : INT; c1 : DINT; c2 : DINT; c3 : DINT; c4 : DINT; c5 : DINT; c6 : DINT;
  b : BYTE; si : SINT; st : INT := -2; ui : UINT; li : LINT; c7 : DINT; c8 : DINT; c9 : DINT;
  last1 : INT; lastb : BYTE; lastsi : SINT; lastui : UINT; lasti : INT; lastsi2 : SINT;
  lastli : LINT; lastb2 : BYTE;
  w2 : BOOL; w3 : BOOL; w5 : BOOL; w6 : BOOL; w7 : BOOL; w8 : BOOL; w9 : BOOL;
END_VAR
(* Loops that stay inside the type end normally. *)
FOR i := 10 TO 1 BY -3 DO c1 := c1 + 1; END_FOR;
last1 := i;
FOR i := 5 TO 1 BY st DO c4 := c4 + 1; END_FOR;

(* Loops that run into the type limit wrap and do not end. *)
FOR b := 250 TO 255 DO
  c2 := c2 + 1; IF b < 250 THEN w2 := TRUE; END_IF; IF c2 >= 300 THEN EXIT; END_IF;
END_FOR;
lastb := b;
FOR si := 120 TO 127 BY 2 DO
  c3 := c3 + 1; IF si < 120 THEN w3 := TRUE; END_IF; IF c3 >= 300 THEN EXIT; END_IF;
END_FOR;
lastsi := si;
FOR ui := 65530 TO 65535 DO
  c5 := c5 + 1; IF ui < 65530 THEN w5 := TRUE; END_IF; IF c5 >= 300 THEN EXIT; END_IF;
END_FOR;
lastui := ui;
FOR i := 32760 TO 32767 DO
  c6 := c6 + 1; IF i < 32760 THEN w6 := TRUE; END_IF; IF c6 >= 300 THEN EXIT; END_IF;
END_FOR;
lasti := i;
FOR si := -126 TO -128 BY -1 DO
  c7 := c7 + 1; IF si > -126 THEN w7 := TRUE; END_IF; IF c7 >= 300 THEN EXIT; END_IF;
END_FOR;
lastsi2 := si;
FOR li := LINT#9223372036854775806 TO LINT#9223372036854775807 DO
  c8 := c8 + 1; IF li < 0 THEN w8 := TRUE; END_IF; IF c8 >= 300 THEN EXIT; END_IF;
END_FOR;
lastli := li;
(* The step jumps over the end value and wraps: 250, 253, 0, 3, ..., 252, 255 —
   255 > 254 is the first value past the end, after 87 iterations. *)
FOR b := 250 TO 254 BY 3 DO
  c9 := c9 + 1; IF b < 250 THEN w9 := TRUE; END_IF; IF c9 >= 300 THEN EXIT; END_IF;
END_FOR;
lastb2 := b;
END_PROGRAM
"#;

#[test]
fn loops_inside_the_type_range_end() {
    for s in [run(SRC), run_o3(SRC)] {
        assert_eq!(s.i64("c1"), 4);
        assert_eq!(s.i64("last1"), -2);
        assert_eq!(s.i64("c4"), 3);
    }
}

#[test]
fn loops_at_type_limits_wrap_and_do_not_end() {
    for s in [run(SRC), run_o3(SRC)] {
        for (count, wrapped) in [
            ("c2", "w2"),
            ("c3", "w3"),
            ("c5", "w5"),
            ("c6", "w6"),
            ("c7", "w7"),
            ("c8", "w8"),
        ] {
            assert_eq!(s.i64(count), 300, "{count}: the loop must still be running at 300");
            assert!(s.bool(wrapped), "{wrapped}: the control variable must have wrapped");
        }
        // The 300th iteration's control value, i.e. start + 299 * step modulo 2^n.
        assert_eq!(s.u64("lastb"), (250 + 299) % 256);
        assert_eq!(s.i64("lastsi"), (120 + 299 * 2) as u8 as i8 as i64);
        assert_eq!(s.u64("lastui"), (65530 + 299) % 65536);
        assert_eq!(s.i64("lasti"), (32760 + 299) as u16 as i16 as i64);
        assert_eq!(s.i64("lastsi2"), (-126 - 299i64) as u8 as i8 as i64);
        assert_eq!(s.i64("lastli"), (i64::MAX - 1).wrapping_add(299));
        // Same rule when the step wraps past the end value: it ends when a
        // wrapped value first exceeds TO.
        assert_eq!(s.i64("c9"), 87);
        assert!(s.bool("w9"));
        assert_eq!(s.u64("lastb2"), 255);
    }
}

#[test]
fn a_constant_step_of_zero_is_an_error() {
    let e = compile_error(
        "PROGRAM p VAR i : INT; END_VAR FOR i := 1 TO 5 BY 0 DO END_FOR; END_PROGRAM",
    );
    assert!(e.contains("BY 0 never terminates"), "{e}");
}
