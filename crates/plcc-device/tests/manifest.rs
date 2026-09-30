// SPDX-License-Identifier: MPL-2.0

//! Manifest parsing, validation and expansion.
//!
//! Golden files: `tests/data/<name>.expanded.json` is the expansion of the
//! built-in manifests and of `tests/data/*.toml`; studio's TypeScript loader is
//! tested against the same files. Regenerate with `PLCC_UPDATE_GOLDEN=1`.

use plcc_device::{Severity, catalog, load};
use std::path::{Path, PathBuf};

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn update() -> bool {
    std::env::var_os("PLCC_UPDATE_GOLDEN").is_some()
}

fn golden(name: &str, actual: &str) {
    let path = data().join(format!("{name}.expanded.json"));
    if update() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} missing; run with PLCC_UPDATE_GOLDEN=1", path.display()));
    assert_eq!(expected, actual, "{} is stale; run with PLCC_UPDATE_GOLDEN=1", path.display());
}

fn expanded_json(source: &str, file: &str) -> String {
    let checked = load(source, Some(file));
    assert!(
        checked.diagnostics.is_empty(),
        "{file}: {:#?}",
        checked.diagnostics
    );
    let mut s = serde_json::to_string_pretty(&checked.device.unwrap()).unwrap();
    s.push('\n');
    s
}

#[test]
fn builtin_manifests_expand_to_their_golden_files() {
    for (name, text) in catalog::BUILTIN {
        let stem = name.trim_end_matches(".toml");
        golden(stem, &expanded_json(text, name));
    }
}

#[test]
fn test_manifests_expand_to_their_golden_files() {
    for e in std::fs::read_dir(data()).unwrap() {
        let path = e.unwrap().path();
        if path.extension().is_some_and(|x| x == "toml") {
            let text = std::fs::read_to_string(&path).unwrap();
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            golden(&stem, &expanded_json(&text, &stem));
        }
    }
}

#[test]
fn opta_manifest_matches_the_runtime() {
    let d = load(catalog::builtin_source("arduino-opta").unwrap(), None)
        .device
        .unwrap();
    assert_eq!((d.target.image.i, d.target.image.q, d.target.image.m), (18, 1, 64));
    assert_eq!(d.target.cpu.as_deref(), Some("cortex-m7"));
    let at = |id: &str| d.io.iter().find(|p| p.id == id).unwrap().address.clone();
    assert_eq!(at("I1"), "%IX0.0");
    assert_eq!(at("I8"), "%IX0.7");
    assert_eq!(at("BTN"), "%IX1.0");
    assert_eq!(at("A1"), "%IW1");
    assert_eq!(at("A8"), "%IW8");
    assert_eq!(at("R1"), "%QX0.0");
    assert_eq!(at("R4"), "%QX0.3");
    assert_eq!(at("LED"), "%QX0.4");
    assert_eq!(at("HR0"), "%MW0");
    assert_eq!(at("HR31"), "%MW31");
    assert_eq!(d.io.len(), 8 + 1 + 8 + 4 + 1 + 32);
    let f = d.flash.unwrap();
    assert_eq!((f.usb[0].vid, f.usb[0].pid, f.alt), (0x2341, 0x0364, 0));
    assert_eq!((f.address, f.max_size), (0x0804_0000, 0x1C_0000));
    let rtu = d.modbus.unwrap().rtu.unwrap();
    assert_eq!((rtu.unit, rtu.baud), (1, 19200));
    assert_eq!(d.console.unwrap().img_m_bytes, 64);
}

/// A small valid manifest to break in the tests below.
const BASE: &str = r#"[device]
id = "t"
name = "T"
vendor = "v"
version = 1
schema = 1

[target]
triple = "thumbv7em-none-eabi"
image = { I = 2, Q = 1, M = 4 }

[target.runtime]
kind = "k"
abi = 1

[[io]]
repeat = 4
id = "I{n}"
terminal = "I{n}"
label = "In {n}"
group = "g"
dir = "in"
kind = "digital"
type = "BOOL"
address = "%IX0.{n-1}"
"#;

/// Diagnostics of `BASE` with `from` replaced by `to`, as "line:col path: message".
fn errors_of(src: &str) -> Vec<String> {
    let c = load(src, Some("t.toml"));
    c.diagnostics
        .iter()
        .map(|d| {
            format!(
                "{}:{} {}: {}{}",
                d.line.unwrap_or(0),
                d.col.unwrap_or(0),
                d.path,
                d.message,
                if d.severity == Severity::Warning { " (warning)" } else { "" }
            )
        })
        .collect()
}

fn broken(from: &str, to: &str) -> Vec<String> {
    assert!(BASE.contains(from), "{from}");
    errors_of(&BASE.replacen(from, to, 1))
}

fn assert_one(errors: &[String], needle: &str) {
    assert!(
        errors.iter().any(|e| e.contains(needle)),
        "no diagnostic containing {needle:?} in {errors:#?}"
    );
}

#[test]
fn base_is_valid() {
    assert!(errors_of(BASE).is_empty());
}

#[test]
fn syntax_and_type_errors_have_positions() {
    let e = broken("abi = 1", "abi = \"one\"");
    assert_eq!(e.len(), 1);
    assert!(e[0].starts_with("14:7 "), "{e:?}");
    let e = broken("vendor = \"v\"", "vendor = \"v\"\ncolour = 1");
    assert_one(&e, "unknown field `colour`");
    assert!(e[0].starts_with("5:1 "), "{e:?}");
    let e = broken("[target]", "[target");
    assert_eq!(e.len(), 1);
    assert!(e[0].starts_with("8:"), "{e:?}");
}

#[test]
fn io_errors_point_at_the_entry() {
    let e = broken("%IX0.{n-1}", "%IX0.{n+4}");
    assert_one(&e, "25:11 io[0].address: `%IX0.8`: bit 8 does not exist");
    let e = broken("%IX0.{n-1}", "%QX0.{n-1}");
    assert_one(&e, "is in %Q, but dir = \"in\" points live in %I");
    let e = broken("%IX0.{n-1}", "%IX{n}.0");
    assert_one(&e, "`%IX2.0` (n = 2) ends at byte 3, past the 2-byte %I area");
    let e = broken("type = \"BOOL\"", "type = \"INT\"");
    assert_one(&e, "24:8 io[0].type: INT does not fit `%IX0.0`");
    let e = broken("id = \"I{n}\"", "id = \"I\"");
    assert_one(&e, "must contain `{n}`");
    let e = broken("repeat = 4\n", "");
    assert_one(&e, "uses `{…}` but has no `repeat`");
    let e = broken("{n-1}", "{n-}");
    assert_one(&e, "in `{n-}`: expression ends early");
    let e = broken("{n-1}", "{x}");
    assert_one(&e, "the only variable is `n`");
    let e = broken("repeat = 4", "repeat = 0");
    assert_one(&e, "io[0].repeat: must be at least 1");
    let e = broken("kind = \"digital\"", "kind = \"analog\"");
    assert_one(&e, "an analog point needs a byte, word or larger address");
    let dup = format!("{BASE}\n[[io]]\nid = \"I2\"\nterminal = \"X\"\nlabel = \"x\"\ngroup = \"g\"\ndir = \"in\"\nkind = \"digital\"\ntype = \"BOOL\"\naddress = \"%IX0.1\"\n");
    let e = errors_of(&dup);
    assert_one(&e, "io[1].id: id `I2` is already used by io[0]");
    assert_one(&e, "`I2` and `I2` share %IX0.1 (warning)");
}

#[test]
fn device_and_target_errors() {
    assert_one(&broken("id = \"t\"", "id = \"My Device\""), "is not a device id");
    assert_one(&broken("schema = 1", "schema = 2"), "manifest format 2 is not supported");
    assert_one(
        &broken("[target.runtime]", "float_abi = \"hard\"\n\n[target.runtime]"),
        "needs an `eabihf` triple",
    );
    assert_one(
        &broken("[target.runtime]", "features = [\"+a,+b\"]\n\n[target.runtime]"),
        "one per string",
    );
    assert_one(
        &broken("triple = \"thumbv7em-none-eabi\"", "triple = \"wasm32-unknown-unknown\"\nfloat_abi = \"soft\""),
        "a float ABI applies to ARM targets only",
    );
    assert_one(&broken("M = 4", "M = 99999999"), "target.image.M: 99999999 bytes");
}

#[test]
fn flash_may_not_cover_a_protected_region() {
    let flash = "\n[flash]\nmethod = \"dfuse\"\nusb = [{ vid = 0x2341, pid = 0x0364 }]\naddress = 0x08000000\nmax_size = 0x1C0000\nprotected = [{ start = 0x08000000, size = 0x40000, reason = \"bootloader\" }]\n";
    let e = errors_of(&format!("{BASE}{flash}"));
    assert_one(&e, "flash.address: the application area 0x08000000..0x081C0000 overlaps protected region 0x08000000..0x08040000 (bootloader)");
    let ok = flash.replace("address = 0x08000000", "address = 0x08040000");
    assert!(errors_of(&format!("{BASE}{ok}")).is_empty());
    let touch = ok.replace("max_size", "reboot = \"1200-baud-touch\"\nmax_size");
    assert_one(&errors_of(&format!("{BASE}{touch}")), "needs the running application's USB ids");
    let over = ok.replace("max_size = 0x1C0000", "max_size = 0xFFFFFFFF");
    assert_one(&errors_of(&format!("{BASE}{over}")), "runs past the 32-bit address space");
}

#[test]
fn console_and_modbus_errors() {
    let console = "\n[console]\ntransport = \"webserial\"\nbaud = 115200\ncommands = [\"img\", \"img\"]\nimg_format = \"hex-areas\"\nimg_m_bytes = 8\n";
    let e = errors_of(&format!("{BASE}{console}"));
    assert_one(&e, "console.commands[1]: `img` is listed twice");
    assert_one(&e, "console.img_m_bytes: 8 bytes, but the %M area has only 4");
    let modbus = "\n[modbus.rtu]\nunit = 0\nbaud = 19200\nparity = \"even\"\n\n[[modbus.map]]\ntable = \"holding\"\ncount = 3\naddress = \"%MW0\"\n\n[[modbus.map]]\ntable = \"holding\"\nstart = 2\ncount = 1\naddress = \"%MX0.0\"\n\n[[modbus.map]]\ntable = \"coils\"\ncount = 1\naddress = \"%IX0.0\"\n";
    let e = errors_of(&format!("{BASE}{modbus}"));
    assert_one(&e, "modbus.rtu.unit: server address 0 is outside 1..247");
    assert_one(&e, "modbus.map[0].count: 3 items from %MW0 end at byte 6, past the 4-byte %M area");
    assert_one(&e, "modbus.map[1].start: overlaps map[0] (holding registers 0..2)");
    assert_one(&e, "holding registers are 16-bit registers; start them at a %_W address");
    assert_one(&e, "coils are written by the master; %I is written only by the runtime");
}

#[test]
fn provenance_fields() {
    let with = "version = 1\nsource = \"https://example.com/t.toml\"\nsha256 = \"abc\"";
    assert_one(&broken("version = 1", with), "device.sha256: must be 64 hex digits");
    let good = with.replace("\"abc\"", &format!("\"{}\"", "a".repeat(64)));
    assert!(broken("version = 1", &good).is_empty());
}

/// docs/device-manifest.schema.json must be what `json_schema()` produces.
#[test]
fn json_schema_is_up_to_date() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/device-manifest.schema.json");
    let actual = plcc_device::json_schema();
    if update() {
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(expected, actual, "docs/device-manifest.schema.json is stale; run with PLCC_UPDATE_GOLDEN=1");
}

/// The repository catalog (`devices/`, a git submodule) and the built-in copies
/// must agree. Skipped when the submodule is not checked out.
#[test]
fn builtin_copies_match_the_catalog() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices");
    let entries = catalog::load_dir(&dir).unwrap();
    if entries.is_empty() {
        eprintln!("skipped: {} has no manifests (submodule not checked out?)", dir.display());
        return;
    }
    for e in &entries {
        let c = e.load();
        assert!(c.diagnostics.is_empty(), "{}: {:#?}", e.display_name(), c.diagnostics);
    }
    for (name, text) in catalog::BUILTIN {
        let Some(e) = entries.iter().find(|e| e.file_name == *name) else {
            panic!("devices/{name} is missing; the built-in copy has no catalog original");
        };
        assert_eq!(
            e.source, *text,
            "crates/plcc-device/builtin/{name} differs from devices/{name}; copy the catalog file over"
        );
    }
}
