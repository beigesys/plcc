// SPDX-License-Identifier: MPL-2.0

//! Logix arithmetic: promotion, truncation of oversized results, REAL→integer
//! rounding half to even, DIV/MOD by zero, logical zero-fill, status flags.

mod common;
use common::*;

#[test]
fn logix_math_semantics() {
    with_plc("math.L5X", |plc| {
        plc.scan();
        // SINT 100 + 100 is computed as DINT 200 and stored truncated: -56,
        // with S:V (1756-RM003 "Math status flags").
        assert_eq!(plc.get("SDst"), -56);
        assert!(plc.get_bool("Ovf1"));
        // Integer DIV truncates; a REAL destination divides in REAL.
        assert_eq!(plc.get("DivI"), 3);
        assert_eq!(plc.get("DivN"), -3);
        assert_eq!(plc.get_real("DivR"), 3.5);
        // DIV by zero: the result is Source A, S:V and a minor fault.
        assert_eq!(plc.get("DivZ"), 42);
        assert!(plc.get_bool("DivZV"));
        assert!(plc.get_bool("lx__S_MINOR"));
        // MOD: A - trunc(A / B) * B; by zero: 0.
        assert_eq!(plc.get("ModN"), -1);
        assert_eq!(plc.get("ModZ"), 0);
        // REAL → DINT rounds half to even.
        assert_eq!(plc.get("Rnd1"), 2);
        assert_eq!(plc.get("Rnd2"), 4);
        assert_eq!(plc.get("Rnd3"), -2);
        assert_eq!(plc.get("Rnd4"), 2);
        assert_eq!(plc.get("CptOut"), 7);
        // DINT overflow keeps the low 32 bits.
        assert_eq!(plc.get("MulOvf"), (10_000_000_000i64 as i32) as i64);
        assert!(plc.get_bool("MulV"));
        assert_eq!(plc.get("NegOut"), -5);
        assert!(plc.get_bool("NegFlag"), "S:N after a negative result");
        assert_eq!(plc.get("AbsOut"), 300);
        assert_eq!(plc.get_real("SqrOut"), 4.0);
        assert_eq!(plc.get("IntDst"), 70000 & 0xFFFF);
        assert!(plc.get_bool("IntV"));
        assert_eq!(plc.get("AndOut"), 0x0F00);
        assert_eq!(plc.get("OrOut"), 0xFFF0);
        assert_eq!(plc.get("XorOut"), 0xF0F0);
        assert_eq!(plc.get("NotOut"), -1);
        assert_eq!(plc.get("ClrMe"), 0);
        assert!(plc.get_bool("ClrZ"), "S:Z after CLR");
        assert_eq!(plc.get("BtdOut"), 0xF0);
        assert_eq!(plc.get("MvmOut"), 0x0FFF);
        // Math sign-extends an INT; logical instructions zero-fill it.
        assert_eq!(plc.get("SignExt"), -1);
        assert_eq!(plc.get("ZeroFill"), 0xFFFF);
        // REAL 3e10 → DINT: rounded, upper bits dropped, S:V.
        assert_eq!(plc.get("Wrapped"), (3.0e10f32 as i64 as i32) as i64);
        assert!(plc.get_bool("WrapV"));
        assert_eq!(plc.get_real("TrnOut"), -3.0);
        assert_eq!(plc.get("TodOut"), 0x1234);
        assert_eq!(plc.get("FrdOut"), 1234);
        assert_eq!(plc.get_real("RealOut"), 5.5);
        assert_eq!(plc.get("PowOut"), 1024);
        assert_eq!(plc.get("Counter"), 0);
    });
}
