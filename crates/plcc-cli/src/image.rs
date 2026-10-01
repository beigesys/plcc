// SPDX-License-Identifier: MPL-2.0

//! `plcc image`: link a compiled object into a program image for a device's
//! program slot, or check an image (docs/program-image.md).

use crate::device::{Loaded, resolve};
use miette::{IntoDiagnostic, Result};
use std::path::Path;

/// The program layout (`[flash.program]` and identity) of a device.
pub fn program_layout(loaded: &Loaded) -> Result<plcc_image::Layout> {
    let d = &loaded.device;
    let prog = d.flash.as_ref().and_then(|f| f.program.as_ref()).ok_or_else(|| {
        miette::miette!(
            "{}: `{}` has no program slot ([flash.program]); its runtime does not load program images",
            loaded.name,
            d.device.id
        )
    })?;
    Ok(plcc_image::Layout {
        target_id: d.device.id.clone(),
        target_version: d.device.version,
        abi: d.target.runtime.abi,
        slot_addr: prog.address,
        slot_size: prog.max_size,
        ram_addr: prog.ram.start,
        ram_size: prog.ram.size,
        services: prog.services,
        hard_float: d.target.float_abi == Some(plcc_device::FloatAbi::Hard),
    })
}

fn parse_build_id(hex: &str) -> Result<[u8; 16]> {
    let hex = hex.trim();
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(miette::miette!("--build-id: `{hex}` is not 32 hex digits"));
    }
    let mut b = [0u8; 16];
    for (i, x) in b.iter_mut().enumerate() {
        *x = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).into_diagnostic()?;
    }
    Ok(b)
}

fn print_header(input: &Path, h: &plcc_image::Header) {
    println!("{}: program image format {}, {} bytes", input.display(), h.format, h.image_size);
    println!("  target   {} v{} (ABI {}), {} service(s)", h.target_id, h.target_version, h.abi, h.services);
    println!(
        "  slot     {:#010x} ({} bytes), RAM window {:#010x} ({} bytes)",
        h.slot_addr, h.slot_size, h.ram_addr, h.ram_size
    );
    println!("  text     {:#010x} {}", h.text_addr, h.text_size);
    println!("  .data    {:#010x} {} (from {:#010x})", h.data_addr, h.data_size, h.data_load);
    println!("  .bss     {:#010x} {}", h.bss_addr, h.bss_size);
    println!("  entry    plcc_get_app {:#010x}", h.get_app);
    println!("  build    {}", h.build_id_hex());
    println!("  crc32    header {:08x}, body {:08x}", h.header_crc32, h.body_crc32);
}

/// `plcc image OBJECT --device D -o IMAGE` and `plcc image --info IMAGE --device D`.
pub fn run(
    input: &Path,
    device: &str,
    output: Option<&Path>,
    build_id: Option<&str>,
    map: bool,
    json: bool,
    info: bool,
) -> Result<()> {
    let loaded = resolve(device)?;
    let layout = program_layout(&loaded)?;
    let bytes = std::fs::read(input).map_err(|e| miette::miette!("{}: {e}", input.display()))?;
    if info {
        let h = plcc_image::Header::parse(&bytes).map_err(|e| miette::miette!("{}: {e}", input.display()))?;
        print_header(input, &h);
        return match plcc_image::check(&bytes, &layout, 1) {
            Ok(_) => {
                println!("valid for {} v{} ({})", layout.target_id, layout.target_version, loaded.name);
                Ok(())
            }
            Err(e) => Err(miette::miette!("{}: not loadable on {}: {e}", input.display(), layout.target_id)),
        };
    }
    let opts = plcc_image::Options { build_id: build_id.map(parse_build_id).transpose()? };
    let img = plcc_image::link(&bytes, &layout, &opts).map_err(|e| miette::miette!("{}: {e}", input.display()))?;
    // Never write an image the runtime would refuse.
    plcc_image::check(&img.bytes, &layout, layout.target_version)
        .map_err(|e| miette::miette!("internal error: the linked image fails the loader's check: {e}"))?;
    let out = output.ok_or_else(|| miette::miette!("-o is required"))?;
    std::fs::write(out, &img.bytes).map_err(|e| miette::miette!("{}: {e}", out.display()))?;
    if json {
        println!("{}", plcc_image::json::report_json(&img));
        return Ok(());
    }
    if map {
        print!("{}", img.map());
    }
    let h = &img.header;
    eprintln!(
        "Linked {} for {} v{}: {} bytes at {:#010x} (text {}, .data {}, .bss {}), {} service(s), build {}",
        out.display(),
        h.target_id,
        h.target_version,
        h.image_size,
        h.slot_addr,
        h.text_size,
        h.data_size,
        h.bss_size,
        img.imports.len(),
        h.build_id_hex()
    );
    Ok(())
}
