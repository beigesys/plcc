// SPDX-License-Identifier: MPL-2.0

//! The image linker on hand-written objects (tests/data, see regen.sh):
//! every supported relocation against ld.lld, objects it must refuse, the
//! header checks, and robustness against malformed input.
//!
//! Golden files: `relocs.lld.{text,data}.bin` are ld.lld's output for
//! relocs.o at the same addresses. Regenerate with `PLCC_UPDATE_GOLDEN=1`
//! (needs ld.lld and llvm-objcopy; `PLCC_LLD`/`PLCC_OBJCOPY` or LLVM 21's
//! default paths). When ld.lld is present the comparison also runs live.

use plcc_image::{Error, Image, Layout, Options, check, header, link};
use std::path::{Path, PathBuf};
use std::process::Command;

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(data(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The Opta's layout, from the built-in manifest.
fn opta() -> Layout {
    let d = plcc_device::load(plcc_device::catalog::builtin_source("arduino-opta").unwrap(), None)
        .device
        .unwrap();
    let p = d.flash.as_ref().unwrap().program.as_ref().unwrap();
    Layout {
        target_id: d.device.id.clone(),
        target_version: d.device.version,
        abi: d.target.runtime.abi,
        slot_addr: p.address,
        slot_size: p.max_size,
        ram_addr: p.ram.start,
        ram_size: p.ram.size,
        services: p.services,
        hard_float: false,
    }
}

fn link_ok(name: &str) -> Image {
    link(&read(name), &opta(), &Options::default()).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn link_err(name: &str) -> String {
    match link(&read(name), &opta(), &Options::default()) {
        Ok(_) => panic!("{name}: linked, but should have been refused"),
        Err(Error { message }) => message,
    }
}

fn tool(env: &str, default: &str) -> Option<PathBuf> {
    let p = std::env::var_os(env).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(default));
    Command::new(&p).arg("--version").output().ok().filter(|o| o.status.success()).map(|_| p)
}

/// ld.lld's text and data for `obj` at the addresses `img` used.
fn lld(obj: &Path, img: &Image) -> Option<(Vec<u8>, Vec<u8>)> {
    let lld = tool("PLCC_LLD", "/usr/lib/llvm-21/bin/ld.lld")?;
    let objcopy = tool("PLCC_OBJCOPY", "/usr/lib/llvm-21/bin/llvm-objcopy")?;
    let h = &img.header;
    let dir = std::env::temp_dir().join(format!("plcc-image-lld-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = format!(
        "SECTIONS {{\n  . = {:#x};\n  .text : {{ *(.text*) *(.rodata*) }}\n  . = {:#x};\n  .data : AT({:#x}) {{ *(.data*) }}\n  .bss : {{ *(.bss*) *(COMMON) }}\n  /DISCARD/ : {{ *(.ARM.exidx*) *(.ARM.extab*) *(.ARM.attributes) *(.comment) *(.note*) }}\n}}\n",
        h.text_addr, h.data_addr, h.data_load
    );
    std::fs::write(dir.join("l.ld"), script).unwrap();
    let mut cmd = Command::new(lld);
    cmd.arg("-O0").arg("-T").arg(dir.join("l.ld")).arg("-o").arg(dir.join("ref.elf")).arg(obj);
    for (_, name, veneer) in &img.imports {
        cmd.arg(format!("--defsym={name}={:#x}", veneer | 1));
    }
    let o = cmd.output().unwrap();
    assert!(o.status.success(), "ld.lld: {}", String::from_utf8_lossy(&o.stderr));
    let section = |s: &str| {
        let out = dir.join(format!("{s}.bin"));
        let st = Command::new(&objcopy)
            .args(["-O", "binary", "-j", s])
            .arg(dir.join("ref.elf"))
            .arg(&out)
            .status()
            .unwrap();
        assert!(st.success());
        std::fs::read(out).unwrap_or_default()
    };
    let r = (section(".text"), section(".data"));
    let _ = std::fs::remove_dir_all(&dir);
    Some(r)
}

fn text_of(img: &Image, len: usize) -> &[u8] {
    &img.bytes[128..128 + len]
}

fn data_of(img: &Image) -> &[u8] {
    let h = &img.header;
    let off = (h.data_load - h.slot_addr) as usize;
    &img.bytes[off..off + h.data_size as usize]
}

#[test]
fn every_relocation_matches_lld() {
    let img = link_ok("relocs.o");
    let (gt, gd) = ("relocs.lld.text.bin", "relocs.lld.data.bin");
    if std::env::var_os("PLCC_UPDATE_GOLDEN").is_some() {
        let (t, d) = lld(&data("relocs.o"), &img).expect("PLCC_UPDATE_GOLDEN needs ld.lld and llvm-objcopy");
        std::fs::write(data(gt), t).unwrap();
        std::fs::write(data(gd), d).unwrap();
    }
    let (t, d) = (read(gt), read(gd));
    assert!(!t.is_empty() && !d.is_empty());
    assert_eq!(text_of(&img, t.len()), &t[..], "text differs from ld.lld (golden)");
    assert_eq!(data_of(&img), &d[..], ".data differs from ld.lld (golden)");
    match lld(&data("relocs.o"), &img) {
        Some((lt, ld)) => {
            assert_eq!(text_of(&img, lt.len()), &lt[..], "text differs from ld.lld (live)");
            assert_eq!(data_of(&img), &ld[..], ".data differs from ld.lld (live)");
        }
        None => eprintln!("ld.lld not found: compared with the golden files only"),
    }
}

#[test]
fn relocs_layout_and_services() {
    let img = link_ok("relocs.o");
    let h = &img.header;
    // memcpy (3), memset (5) and the weak plcc_fault (2) go to the runtime.
    let names: Vec<&str> = img.imports.iter().map(|(_, n, _)| n.as_str()).collect();
    assert_eq!(names, ["plcc_fault", "memcpy", "memset"]);
    assert_eq!(h.services, 6);
    // Veneers follow text, in index order, 16 bytes each.
    let v0 = img.imports[0].2;
    assert_eq!(img.imports[1].2, v0 + 16);
    assert_eq!(h.text_addr + h.text_size, v0 + 48);
    // A veneer for service k loads fn[k] from the table at the window's first word.
    let off = (img.imports[1].2 - h.slot_addr) as usize;
    assert_eq!(&img.bytes[off..off + 16], &plcc_image::arm::veneer(h.ram_addr, 3));
    // .data at the window start + 8, .bss and COMMON after it, all inside the window.
    assert_eq!(h.data_addr, h.ram_addr + 8);
    assert_eq!(h.data_size, 12);
    assert!(h.bss_addr >= h.data_addr + h.data_size);
    assert_eq!(h.bss_size, 8 + 4 + 12, "counter (8), padding to COMMON's alignment, shared (12)");
    assert_eq!(img.symbols["ptrs"], h.data_addr);
    // The image is padded to whole flash words and passes the loader's check.
    assert_eq!(img.bytes.len() % 32, 0);
    assert_eq!(img.bytes.len() as u32, h.image_size);
    check(&img.bytes, &opta(), 2).unwrap();
    assert!(img.map().contains("plcc_get_app"));
}

#[test]
fn refuses_what_it_cannot_link() {
    let e = link_err("errors-1.o");
    assert!(e.contains("undefined symbols `not_a_service`, `printf`"), "{e}");
    assert!(e.contains("not a runtime service"), "{e}");
    let e = link_err("errors-2.o");
    assert!(e.contains(".init_array") && e.contains("constructors"), "{e}");
    let e = link_err("errors-3.o");
    assert!(e.contains(".tbss") && e.contains("thread-local"), "{e}");
    let e = link_err("errors-4.o");
    assert!(e.contains("R_ARM_CALL at .text.arm+0x0") && e.contains("not supported"), "{e}");
    let e = link_err("errors-5.o");
    assert!(e.contains(".text.rw") && e.contains("writable code"), "{e}");
    let mut small = opta();
    small.slot_size = 256; // relocs.o needs 128 + ~160 bytes
    let e = link(&read("relocs.o"), &small, &Options::default()).unwrap_err().message;
    assert!(e.contains("does not fit") && e.contains("slot 0x08180000, 256 bytes"), "{e}");
    let e = link_err("errors-6.o");
    assert!(e.contains("does not fit") && e.contains(".data and .bss need 70008 bytes"), "{e}");
    let e = link_err("errors-7.o");
    assert!(e.contains("R_ARM_THM_JUMP8") && e.contains("does not fit in 9 signed bits"), "{e}");
    let e = link_err("errors-8.o");
    assert!(e.contains("R_ARM_GOT_PREL") && e.contains("not supported"), "{e}");
    let e = link_err("errors-9.o");
    assert!(e.contains("does not define plcc_get_app"), "{e}");
    let e = link_err("errors-10.o");
    assert!(e.contains("hard-float"), "{e}");
    let mut hard = opta();
    hard.hard_float = true;
    link(&read("errors-10.o"), &hard, &Options::default()).unwrap();
    let e = link(&read("relocs.o"), &hard, &Options::default()).unwrap_err().message;
    assert!(e.contains("soft-float"), "{e}");
}

#[test]
fn refuses_services_the_runtime_lacks() {
    let mut old = opta();
    old.services = 4; // a runtime with services 0..3: memset (5) is missing
    let e = link(&read("relocs.o"), &old, &Options::default()).unwrap_err().message;
    assert!(e.contains("`memset` (runtime service 5)") && e.contains("services 0..3"), "{e}");
}

#[test]
fn build_id_defaults_to_the_object_hash() {
    let a = link_ok("relocs.o");
    let b = link_ok("relocs.o");
    assert_eq!(a.bytes, b.bytes, "linking is deterministic");
    let id = [7u8; 16];
    let c = link(&read("relocs.o"), &opta(), &Options { build_id: Some(id) }).unwrap();
    assert_eq!(c.header.build_id, id);
    assert_ne!(a.header.build_id, id);
    assert_eq!(&c.bytes[128..], &a.bytes[128..], "the build id only changes the header");
}

/// Every check the loader makes, each provoked by one change to a good image.
#[test]
fn header_checks_mirror_the_loader() {
    let layout = opta();
    let good = link_ok("relocs.o").bytes;
    check(&good, &layout, 2).unwrap();
    let reseal = |b: &mut Vec<u8>| {
        let crc = header::crc32(&b[..124]);
        b[124..128].copy_from_slice(&crc.to_le_bytes());
    };
    let field = |off: usize, v: u32, reseal_it: bool| {
        let mut b = good.clone();
        b[off..off + 4].copy_from_slice(&v.to_le_bytes());
        if reseal_it {
            reseal(&mut b);
        }
        check(&b, &layout, 2).unwrap_err().message
    };
    assert_eq!(field(0, 0xFFFF_FFFF, false), "empty slot");
    assert_eq!(field(0, 0x1234_5678, false), "no program image (bad magic)");
    assert_eq!(field(4, 2 | 128 << 16, true), "unknown image format");
    assert_eq!(field(8, 4096, false), "header CRC mismatch");
    assert_eq!(field(40, 3, true), "linked for another version of this device's manifest");
    assert_eq!(field(40, 1, true), "linked for another version of this device's manifest");
    assert_eq!(field(44, 2, true), "runtime ABI mismatch");
    assert_eq!(field(48, 186, true), "needs services this runtime does not have");
    assert_eq!(field(52, 0x0808_0000, true), "linked for another program slot");
    assert_eq!(field(60, 0x2400_0000, true), "linked for another RAM window");
    assert_eq!(field(104, 1, true), "unknown flags");
    assert_eq!(field(8, 0x10_0000, true), "bad image size");
    assert_eq!(field(68, 0x0818_0100, true), "text does not follow the header");
    assert_eq!(field(72, 0x10_0000, true), "text outside the image");
    assert_eq!(field(84, 0x10_0000, true), ".data image outside the image");
    assert_eq!(field(96, 0x2001_0004, true), "services slot is not the window's first word");
    assert_eq!(field(80, 0x2000_0000, true), ".data outside the RAM window");
    assert_eq!(field(92, 0x10_0000, true), ".bss outside the RAM window");
    assert_eq!(field(100, 0x0818_0080, true), "plcc_get_app is not Thumb code inside the image");
    assert_eq!(field(100, 0x0800_0001, true), "plcc_get_app is not Thumb code inside the image");
    let mut b = good.clone();
    b[16..40].copy_from_slice(&[b'x'; 24]);
    reseal(&mut b);
    assert_eq!(check(&b, &layout, 2).unwrap_err().message, "target id not terminated");
    let mut b = good.clone();
    b[16] = b'b';
    reseal(&mut b);
    assert_eq!(check(&b, &layout, 2).unwrap_err().message, "linked for another device");
    let mut b = good.clone();
    let last = b.len() - 1;
    b[last] ^= 1;
    assert_eq!(check(&b, &layout, 2).unwrap_err().message, "body CRC mismatch");
    let mut b = good.clone();
    b[200] ^= 0x80;
    assert_eq!(check(&b, &layout, 2).unwrap_err().message, "body CRC mismatch");
}

/// Malformed objects are errors, never panics: every truncation and a sweep
/// of single-byte corruptions of a real object.
#[test]
fn malformed_objects_do_not_panic() {
    let layout = opta();
    for name in ["relocs.o", "errors-7.o"] {
        let obj = read(name);
        for len in 0..obj.len() {
            let _ = link(&obj[..len], &layout, &Options::default());
        }
        for i in 0..obj.len() {
            for x in [0x01u8, 0x80, 0xff] {
                let mut b = obj.clone();
                b[i] ^= x;
                let _ = link(&b, &layout, &Options::default());
            }
        }
    }
    assert!(link(b"not an object", &layout, &Options::default()).unwrap_err().message.contains("not an ELF"));
}

/// The service list in Rust and the runtime's C table are the same list.
#[test]
fn services_match_the_runtime_table() {
    let h = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtimes/arduino-opta/loader/plcc_services.h");
    let text = std::fs::read_to_string(&h).unwrap();
    let mut c: Vec<(usize, String)> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("PLCC_SERVICE(") {
            let inner = rest.split(')').next().unwrap();
            let (i, n) = inner.split_once(',').unwrap();
            c.push((i.trim().parse().unwrap(), n.trim().to_string()));
        }
    }
    let rust: Vec<(usize, String)> =
        plcc_image::services::SERVICES.iter().enumerate().map(|(i, s)| (i, s.to_string())).collect();
    assert_eq!(c, rust);
    assert!(text.contains(&format!("#define PLCC_SERVICE_COUNT {}u", rust.len())));
    assert_eq!(opta().services as usize, rust.len(), "the manifest's [flash.program] services");
}
