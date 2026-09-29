// SPDX-License-Identifier: MPL-2.0

//! `--io-map`: module tags and controller tags bound to the process image, so
//! an L5X seal-in rung drives an Opta relay.

mod common;
use common::*;

fn opts() -> plcc_l5x::Options {
    let text = std::fs::read_to_string(fixture("opta_io.toml")).expect("map");
    plcc_l5x::Options {
        io_map: plcc_l5x::IoMap::parse(&text).expect("valid map"),
    }
}

#[test]
fn seal_in_on_process_image() {
    let src = read_fixture("opta_io.L5X");
    with_plc_src("opta_io.L5X", &src, &opts(), |plc| {
        plc.scan();
        assert_eq!(plc.output_byte(0) & 1, 0);
        // I1 (start) pressed.
        *plc.input_byte(0) |= 1;
        plc.scan();
        assert!(plc.get_bool("Motor"));
        assert_eq!(plc.output_byte(0) & 1, 1, "relay 1 on");
        assert_eq!(
            plc.get("Local__1__I.Data") & 1,
            1,
            "module tag follows %IX0.0"
        );
        *plc.input_byte(0) &= !1;
        plc.scan();
        assert_eq!(plc.output_byte(0) & 1, 1, "sealed in");
        // I2 (stop) pressed.
        *plc.input_byte(0) |= 2;
        plc.scan();
        assert_eq!(plc.output_byte(0) & 1, 0, "relay 1 off");
        *plc.input_byte(0) &= !2;
        // Analog I2 at %IW2 (bytes 4..5) above 2000 lights the LED.
        *plc.input_byte(4) = (2500u16 & 0xff) as u8;
        *plc.input_byte(5) = (2500u16 >> 8) as u8;
        plc.scan();
        assert_eq!(plc.get("Level"), 2500);
        assert_eq!(plc.output_byte(0) & 0x10, 0x10, "USER LED");
    });
}

#[test]
fn bad_map_entries_are_reported() {
    let src = read_fixture("opta_io.L5X");
    let map = plcc_l5x::IoMap::parse("\"Nope\" = \"%IX0.0\"\n\"Level\" = \"%IX0.1\"\n").unwrap();
    let (_, errs) = plcc_l5x::parse_with(&src, &plcc_l5x::Options { io_map: map });
    let msgs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("`Nope`") && m.contains("unknown tag")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("INT does not fit %IX0.1")),
        "{msgs:?}"
    );
}
