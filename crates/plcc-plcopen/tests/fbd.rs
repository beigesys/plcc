// SPDX-License-Identifier: MPL-2.0

//! Function Block Diagram bodies: operators, generic and user function calls,
//! connectors, negated pins, FB instances, executionOrderId.

mod common;
use common::*;

#[test]
fn fbd_arithmetic_runs() {
    with_plc("fbd_arith.xml", |plc| {
        let g = |n: &str| plc.get(&format!("Calc.{n}"));
        let set = |n: &str, v: i64| plc.set(&format!("Calc.{n}"), v);
        for (a, b, c) in [(3, 4, 5), (20, 7, 10), (-2, -9, 3)] {
            set("a", a);
            set("b", b);
            set("c", c);
            plc.scan();
            let y = (a + b) * c;
            assert_eq!(g("y"), y);
            assert_eq!(g("s"), a + b, "connector/continuation");
            assert_eq!(g("m"), a.max(b), "MAX call");
            assert_eq!(g("lim"), y.clamp(0, 100), "LIMIT reads y after it was written");
            assert_eq!(g("sc"), a * 2 + 1, "user FUNCTION with an FBD body");
            assert_eq!(g("agtb"), (a > b) as i64);
        }
        for bits in 0..4 {
            let (f, o) = (bits & 1, (bits >> 1) & 1);
            set("flag", f);
            set("other", o);
            plc.scan();
            assert_eq!(g("both"), (f == 1 && o == 0) as i64, "negated input pin");
        }
    });
}

#[test]
fn fbd_fb_instance_accumulates() {
    with_plc("fbd_arith.xml", |plc| {
        plc.set("Calc.a", 5);
        for i in 1..=4 {
            plc.scan();
            assert_eq!(plc.get("Calc.tot"), 5 * i);
        }
    });
}

#[test]
fn fbd_type_checks() {
    let errs = check_messages("fbd_arith.xml");
    assert!(errs.is_empty(), "{errs:#?}");
}
