// SPDX-License-Identifier: MPL-2.0
// Spike: plcc's LLVM code generator running as a WASI program.
//   webcc <input.st> <output.o|.ll> <triple> [symbols.json]
use std::collections::BTreeMap;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, output, triple) = (&args[1], &args[2], &args[3]);
    let text = std::fs::read_to_string(input).expect("read input");
    let name = input.rsplit('/').next().unwrap().to_string();
    let project = plcc_driver::Project {
        files: BTreeMap::from([(name.clone(), text)]),
        ..Default::default()
    };
    let checked = plcc_driver::check(&project);
    for d in &checked.diagnostics {
        eprintln!("{:?} {}: {}", d.severity, d.file.as_deref().unwrap_or("-"), d.message);
    }
    let parsed = checked.parsed.expect("parsed");
    if !checked.ok {
        std::process::exit(1);
    }
    let t0 = std::time::Instant::now();
    let ctx = inkwell::context::Context::create();
    let mut c = plcc_codegen::Compiler::new(&ctx, &name);
    for (area, bytes) in [
        (plcc_codegen::direct_address::Area::Input, 18),
        (plcc_codegen::direct_address::Area::Output, 1),
        (plcc_codegen::direct_address::Area::Memory, 64),
    ] {
        c.set_image_size(area, bytes);
    }
    // Which file each POU came from (fault sites).
    let mut pous: Vec<String> = Vec::new();
    for (d, o) in parsed.unit.declarations.iter().zip(&parsed.origins) {
        if !o.prelude {
            pous.extend(plcc_driver::declaration_name(d));
        }
    }
    c.add_source_file(&name, &project.files[&name], pous);
    c.compile(&parsed.unit).expect("codegen");
    c.optimize(triple, 2).expect("optimize");
    if output.ends_with(".ll") {
        std::fs::write(output, c.emit_ir()).unwrap();
    } else {
        c.emit_object(std::path::Path::new(output), triple).expect("emit");
    }
    if let Some(sym) = args.get(4) {
        let contract = c.runtime_contract(triple).expect("contract");
        std::fs::write(sym, plcc_codegen::header::symbols_json(&contract)).unwrap();
    }
    eprintln!("codegen+emit: {:?}", t0.elapsed());
}
