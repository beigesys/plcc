// SPDX-License-Identifier: MPL-2.0

//! The link report as JSON (`plcc image --json`, and the WebAssembly build's
//! report for `packages/plc-image`). Hand-written to keep the crate free of
//! dependencies.

pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The report for a linked image (also used by the CLI's `--json`).
pub fn report_json(img: &crate::Image) -> String {
    let h = &img.header;
    let sections = img
        .sections
        .iter()
        .map(|p| format!("{{\"name\":{},\"kind\":\"{}\",\"addr\":{},\"size\":{}}}", json_str(&p.name), p.kind, p.addr, p.size))
        .collect::<Vec<_>>()
        .join(",");
    let imports = img
        .imports
        .iter()
        .map(|(i, n, a)| format!("{{\"index\":{i},\"name\":{},\"veneer\":{a}}}", json_str(n)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        concat!(
            "{{\"ok\":true,\"address\":{},\"size\":{},\"buildId\":\"{}\",",
            "\"header\":{{\"format\":{},\"imageSize\":{},\"bodyCrc32\":{},\"headerCrc32\":{},\"targetId\":{},",
            "\"targetVersion\":{},\"abi\":{},\"services\":{},\"slotAddr\":{},\"slotSize\":{},\"ramAddr\":{},\"ramSize\":{},",
            "\"textAddr\":{},\"textSize\":{},\"dataLoad\":{},\"dataAddr\":{},\"dataSize\":{},\"bssAddr\":{},\"bssSize\":{},",
            "\"servicesSlot\":{},\"getApp\":{},\"flags\":{}}},\"sections\":[{}],\"imports\":[{}]}}"
        ),
        h.slot_addr,
        h.image_size,
        h.build_id_hex(),
        h.format,
        h.image_size,
        h.body_crc32,
        h.header_crc32,
        json_str(&h.target_id),
        h.target_version,
        h.abi,
        h.services,
        h.slot_addr,
        h.slot_size,
        h.ram_addr,
        h.ram_size,
        h.text_addr,
        h.text_size,
        h.data_load,
        h.data_addr,
        h.data_size,
        h.bss_addr,
        h.bss_size,
        h.services_slot,
        h.get_app,
        h.flags,
        sections,
        imports
    )
}

