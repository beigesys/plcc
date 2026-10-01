// SPDX-License-Identifier: MPL-2.0

//! Parse, validate and expand a manifest.

use crate::diag::{Diagnostic, Locator, Path, Severity, line_col};
use crate::expr;
use crate::model::*;
use plcc_runtime::direct_address::{self, AddrSize, Area, DirectAddress, ParsedAddress};
use std::collections::{BTreeMap, HashMap};

/// The result of [`load`]: the parsed file, its expansion when it is valid, and
/// every diagnostic (warnings included).
#[derive(Clone, Debug)]
pub struct Checked {
    pub manifest: Option<Manifest>,
    pub device: Option<Device>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Checked {
    pub fn ok(&self) -> bool {
        self.device.is_some()
    }
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.is_error())
    }
}

/// Largest image area a manifest may declare (16 MiB).
pub const MAX_IMAGE_AREA: u32 = 16 << 20;

/// Parse the TOML text into a [`Manifest`] (types and unknown keys checked,
/// nothing else).
pub fn parse(source: &str, file: Option<&str>) -> Result<Manifest, Diagnostic> {
    toml::from_str::<Manifest>(source).map_err(|e| {
        let span = e.span();
        let (line, col) = match &span {
            Some(s) => {
                let (l, c) = line_col(source, s.start);
                (Some(l), Some(c))
            }
            None => (None, None),
        };
        Diagnostic {
            file: file.map(str::to_string),
            severity: Severity::Error,
            message: e.message().trim().to_string(),
            path: String::new(),
            line,
            col,
            span,
        }
    })
}

/// Parse, validate and expand.
pub fn load(source: &str, file: Option<&str>) -> Checked {
    match parse(source, file) {
        Err(d) => Checked {
            manifest: None,
            device: None,
            diagnostics: vec![d],
        },
        Ok(m) => {
            let (device, diagnostics) = validate(&m, Some(source), file);
            Checked {
                manifest: Some(m),
                device,
                diagnostics,
            }
        }
    }
}

/// Validate a parsed manifest and expand it. `source` (the TOML text the
/// manifest came from) gives diagnostics their line and column.
pub fn validate(m: &Manifest, source: Option<&str>, file: Option<&str>) -> (Option<Device>, Vec<Diagnostic>) {
    let loc = source.map(Locator::new);
    let mut v = Validator {
        loc: loc.as_ref(),
        file,
        diags: Vec::new(),
    };
    let io = v.check(m);
    let ok = !v.diags.iter().any(|d| d.is_error());
    let device = ok.then(|| Device {
        device: m.device.clone(),
        target: m.target.clone(),
        flash: m.flash.clone(),
        console: m.console.clone(),
        modbus: m.modbus.clone().map(|mut mb| {
            for e in &mut mb.map {
                if let Ok(ParsedAddress::Located(a)) = direct_address::parse(&e.address) {
                    e.address = a.to_string();
                }
            }
            mb
        }),
        io,
    });
    (device, v.diags)
}

struct Validator<'a, 'i> {
    loc: Option<&'a Locator<'i>>,
    file: Option<&'a str>,
    diags: Vec<Diagnostic>,
}

fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|p| {
            !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// IEC types that fit a size prefix.
pub fn types_for_size(size: AddrSize) -> &'static [&'static str] {
    match size {
        AddrSize::Bit => &["BOOL"],
        AddrSize::Byte => &["BYTE", "SINT", "USINT", "CHAR"],
        AddrSize::Word => &["WORD", "INT", "UINT", "WCHAR"],
        AddrSize::Dword => &["DWORD", "DINT", "UDINT", "REAL"],
        AddrSize::Lword => &["LWORD", "LINT", "ULINT", "LREAL"],
    }
}

impl Validator<'_, '_> {
    fn push(&mut self, severity: Severity, path: &Path, message: impl Into<String>) {
        let message = message.into();
        let d = match self.loc {
            Some(loc) => loc.diagnostic(self.file, severity, path, message),
            None => Diagnostic {
                file: self.file.map(str::to_string),
                severity,
                message,
                path: path.to_string(),
                line: None,
                col: None,
                span: None,
            },
        };
        self.diags.push(d);
    }

    fn error(&mut self, path: &Path, message: impl Into<String>) {
        self.push(Severity::Error, path, message);
    }

    fn warn(&mut self, path: &Path, message: impl Into<String>) {
        self.push(Severity::Warning, path, message);
    }

    fn check(&mut self, m: &Manifest) -> Vec<IoPoint> {
        let root = Path::root();
        self.device(&m.device, &root.key("device"));
        self.target(&m.target, &root.key("target"));
        if let Some(f) = &m.flash {
            self.flash(f, &root.key("flash"));
        }
        if let Some(c) = &m.console {
            self.console(c, &m.target.image, &root.key("console"));
        }
        if let Some(mb) = &m.modbus {
            self.modbus(mb, &m.target.image, &root.key("modbus"));
        }
        self.io(&m.io, &m.target.image, &root.key("io"))
    }

    fn device(&mut self, d: &DeviceInfo, p: &Path) {
        if !is_slug(&d.id) {
            self.error(
                &p.key("id"),
                format!("`{}` is not a device id: use lowercase letters, digits and single `-` (`arduino-opta`)", d.id),
            );
        }
        if d.name.trim().is_empty() {
            self.error(&p.key("name"), "must not be empty");
        }
        if d.schema != SCHEMA_VERSION {
            self.error(
                &p.key("schema"),
                format!(
                    "manifest format {} is not supported; this plcc reads format {SCHEMA_VERSION}",
                    d.schema
                ),
            );
        }
        if d.version == 0 {
            self.error(&p.key("version"), "manifest versions start at 1");
        }
        if let Some(h) = &d.sha256 {
            if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                self.error(&p.key("sha256"), "must be 64 hex digits");
            }
        }
    }

    fn target(&mut self, t: &Target, p: &Path) {
        let triple = t.triple.trim();
        if triple.is_empty() || !triple.contains('-') || triple != t.triple {
            self.error(
                &p.key("triple"),
                format!("`{}` is not an LLVM target triple (`thumbv7em-none-eabi`)", t.triple),
            );
        }
        if let Some(cpu) = &t.cpu {
            if cpu.trim().is_empty() || cpu.contains(char::is_whitespace) {
                self.error(&p.key("cpu"), format!("`{cpu}` is not a CPU name"));
            }
        }
        for (i, f) in t.features.iter().enumerate() {
            let name = f.strip_prefix('+').or_else(|| f.strip_prefix('-'));
            if name.is_none_or(|n| n.is_empty() || n.contains([',', ' '])) {
                self.error(
                    &p.key("features").index(i),
                    format!("`{f}` is not a target feature: write `+name` or `-name`, one per string"),
                );
            }
        }
        if let Some(abi) = t.float_abi {
            if let Err(e) = abi.features_for(triple) {
                self.error(&p.key("float_abi"), e);
            }
        }
        if t.runtime.kind.trim().is_empty() {
            self.error(&p.key("runtime").key("kind"), "must not be empty");
        }
        if t.runtime.abi == 0 {
            self.error(&p.key("runtime").key("abi"), "runtime ABI versions start at 1");
        }
        for (name, size) in [("I", t.image.i), ("Q", t.image.q), ("M", t.image.m)] {
            if size > MAX_IMAGE_AREA {
                self.error(
                    &p.key("image").key(name),
                    format!("{size} bytes is more than the {MAX_IMAGE_AREA}-byte limit"),
                );
            }
        }
    }

    fn flash(&mut self, f: &Flash, p: &Path) {
        if f.max_size == 0 {
            self.error(&p.key("max_size"), "must be more than 0");
        }
        let end = f.address as u64 + f.max_size as u64;
        if end > 1 << 32 {
            self.error(
                &p.key("max_size"),
                format!("0x{:X} + 0x{:X} runs past the 32-bit address space", f.address, f.max_size),
            );
        }
        for (i, r) in f.protected.iter().enumerate() {
            if r.size == 0 {
                self.error(&p.key("protected").index(i).key("size"), "must be more than 0");
            }
            if (f.address as u64) < r.end() && (r.start as u64) < end {
                self.error(
                    &p.key("address"),
                    format!(
                        "the application area 0x{:08X}..0x{:08X} overlaps protected region 0x{:08X}..0x{:08X}{}",
                        f.address,
                        end,
                        r.start,
                        r.end(),
                        if r.reason.is_empty() { String::new() } else { format!(" ({})", r.reason) }
                    ),
                );
            }
        }
        if f.reboot == Reboot::Touch1200 && f.runtime_usb.is_empty() {
            self.error(
                &p.key("runtime_usb"),
                "`1200-baud-touch` needs the running application's USB ids (`runtime_usb`)",
            );
        }
        if f.usb.is_empty() {
            self.error(&p.key("usb"), "list the bootloader's USB ids");
        }
        if let Some(prog) = &f.program {
            self.program_slot(f, prog, end, &p.key("program"));
        }
        for (i, u) in f.usb.iter().chain(&f.runtime_usb).enumerate() {
            if u.vid == 0 {
                let key = if i < f.usb.len() { "usb" } else { "runtime_usb" };
                let idx = if i < f.usb.len() { i } else { i - f.usb.len() };
                self.error(&p.key(key).index(idx).key("vid"), "USB vendor id 0 is not valid");
            }
        }
    }

    fn program_slot(&mut self, f: &Flash, prog: &ProgramSlot, app_end: u64, p: &Path) {
        if prog.format != PROGRAM_IMAGE_FORMAT {
            self.error(
                &p.key("format"),
                format!("program image format {} is not one plcc writes ({PROGRAM_IMAGE_FORMAT})", prog.format),
            );
        }
        if prog.max_size < 256 {
            self.error(&p.key("max_size"), "must be at least 256 bytes (the image header is 128)");
        }
        let end = prog.address as u64 + prog.max_size as u64;
        if end > 1 << 32 {
            self.error(
                &p.key("max_size"),
                format!("0x{:X} + 0x{:X} runs past the 32-bit address space", prog.address, prog.max_size),
            );
        }
        if prog.address % 32 != 0 {
            self.error(&p.key("address"), "must be 32-byte aligned (a flash word)");
        }
        if (f.address as u64) < end && (prog.address as u64) < app_end {
            self.error(
                &p.key("address"),
                format!(
                    "the program slot 0x{:08X}..0x{:08X} overlaps the application (runtime) area 0x{:08X}..0x{:08X}",
                    prog.address, end, f.address, app_end
                ),
            );
        }
        for r in &f.protected {
            if (prog.address as u64) < r.end() && (r.start as u64) < end {
                self.error(
                    &p.key("address"),
                    format!(
                        "the program slot 0x{:08X}..0x{:08X} overlaps protected region 0x{:08X}..0x{:08X}{}",
                        prog.address,
                        end,
                        r.start,
                        r.end(),
                        if r.reason.is_empty() { String::new() } else { format!(" ({})", r.reason) }
                    ),
                );
            }
        }
        if prog.ram.size < 64 {
            self.error(&p.key("ram").key("size"), "must be at least 64 bytes");
        }
        if prog.ram.start as u64 + prog.ram.size as u64 > 1 << 32 {
            self.error(&p.key("ram").key("size"), "runs past the 32-bit address space");
        }
        if prog.ram.start % 8 != 0 {
            self.error(&p.key("ram").key("start"), "must be 8-byte aligned");
        }
        if prog.services < 3 {
            self.error(
                &p.key("services"),
                "a runtime provides at least plcc_monotonic_ns, plcc_print and plcc_fault (3 services)",
            );
        }
    }

    fn console(&mut self, c: &Console, image: &ImageSizes, p: &Path) {
        if c.baud == 0 {
            self.error(&p.key("baud"), "must be more than 0");
        }
        if c.commands.is_empty() {
            self.error(&p.key("commands"), "list the commands the runtime answers");
        }
        let mut seen = Vec::new();
        for (i, cmd) in c.commands.iter().enumerate() {
            if seen.contains(cmd) {
                self.error(&p.key("commands").index(i), format!("`{}` is listed twice", cmd.as_str()));
            }
            seen.push(*cmd);
        }
        if c.img_m_bytes > image.m {
            self.error(
                &p.key("img_m_bytes"),
                format!("{} bytes, but the %M area has only {}", c.img_m_bytes, image.m),
            );
        }
        if !c.commands.contains(&ConsoleCommand::Img) && c.img_m_bytes > 0 {
            self.warn(&p.key("img_m_bytes"), "set, but `img` is not among the commands");
        }
    }

    fn modbus(&mut self, mb: &Modbus, image: &ImageSizes, p: &Path) {
        if let Some(r) = &mb.rtu {
            let rp = p.key("rtu");
            if !(1..=247).contains(&r.unit) {
                self.error(&rp.key("unit"), format!("server address {} is outside 1..247", r.unit));
            }
            if r.baud == 0 {
                self.error(&rp.key("baud"), "must be more than 0");
            }
            if !(7..=8).contains(&r.data_bits) {
                self.error(&rp.key("data_bits"), "must be 7 or 8");
            }
            if !(1..=2).contains(&r.stop_bits) {
                self.error(&rp.key("stop_bits"), "must be 1 or 2");
            }
        }
        let mut ranges: Vec<(ModbusTable, u32, u32, usize)> = Vec::new();
        for (i, e) in mb.map.iter().enumerate() {
            let ep = p.key("map").index(i);
            if e.count == 0 {
                self.error(&ep.key("count"), "must be more than 0");
                continue;
            }
            let last = e.start as u32 + e.count as u32;
            if last > 65536 {
                self.error(&ep.key("count"), format!("items {}..{} run past 65535", e.start, last - 1));
            }
            for &(t, s, l, j) in &ranges {
                if t == e.table && (e.start as u32) < l && s < last {
                    self.error(
                        &ep.key("start"),
                        format!("overlaps map[{j}] ({} {}..{})", table_name(t), s, l - 1),
                    );
                }
            }
            ranges.push((e.table, e.start as u32, last, i));
            let Some(a) = self.address(&e.address, &ep.key("address")) else {
                continue;
            };
            let want = if e.table.is_bits() { AddrSize::Bit } else { AddrSize::Word };
            if a.size != want {
                self.error(
                    &ep.key("address"),
                    format!(
                        "{} are {}; start them at a %_{} address",
                        table_name(e.table),
                        if e.table.is_bits() { "bits" } else { "16-bit registers" },
                        want.letter()
                    ),
                );
                continue;
            }
            if !e.table.writable() && a.area != Area::Input {
                self.warn(
                    &ep.key("address"),
                    format!("{} are read-only to the master; they usually alias %I", table_name(e.table)),
                );
            }
            if e.table.writable() && a.area == Area::Input {
                self.error(
                    &ep.key("address"),
                    format!("{} are written by the master; %I is written only by the runtime", table_name(e.table)),
                );
            }
            // The last item's byte.
            let bytes = if e.table.is_bits() {
                (a.byte as u64 * 8 + a.bit.unwrap_or(0) as u64 + e.count as u64).div_ceil(8)
            } else {
                a.byte as u64 + 2 * e.count as u64
            };
            let size = area_size(image, a.area);
            if bytes > size as u64 {
                self.error(
                    &ep.key("count"),
                    format!(
                        "{} items from {} end at byte {}, past the {}-byte %{} area",
                        e.count,
                        a,
                        bytes,
                        size,
                        a.area.letter()
                    ),
                );
            }
        }
    }

    fn address(&mut self, text: &str, p: &Path) -> Option<DirectAddress> {
        match direct_address::parse(text) {
            Ok(ParsedAddress::Located(a)) => Some(a),
            Ok(ParsedAddress::Partial { .. }) => {
                self.error(p, format!("`{text}` is not a complete address"));
                None
            }
            Err(e) => {
                self.error(p, e);
                None
            }
        }
    }

    fn io(&mut self, entries: &[IoEntry], image: &ImageSizes, p: &Path) -> Vec<IoPoint> {
        let mut out = Vec::new();
        let mut ids: HashMap<String, usize> = HashMap::new();
        let mut addrs: BTreeMap<String, String> = BTreeMap::new();
        for (i, e) in entries.iter().enumerate() {
            let ep = p.index(i);
            let templated = [
                &e.id,
                &e.terminal,
                &e.label,
                &e.group,
                &e.address,
            ]
            .into_iter()
            .chain(e.description.as_ref())
            .chain(e.units.as_ref())
            .any(|s| expr::has_placeholder(s));
            let ns: Vec<i64> = match e.repeat {
                None => {
                    if templated {
                        self.error(&ep, "uses `{…}` but has no `repeat`");
                        continue;
                    }
                    if e.from.is_some() {
                        self.error(&ep.key("from"), "`from` needs `repeat`");
                    }
                    vec![e.from.unwrap_or(1)]
                }
                Some(0) => {
                    self.error(&ep.key("repeat"), "must be at least 1");
                    continue;
                }
                Some(r) if r > 4096 => {
                    self.error(&ep.key("repeat"), format!("{r} points is more than the 4096 one entry may expand to"));
                    continue;
                }
                Some(r) => {
                    if r > 1 && !expr::has_placeholder(&e.id) {
                        self.error(&ep.key("id"), "a repeated entry's id must contain `{n}` (or another expression) to be unique");
                        continue;
                    }
                    let from = e.from.unwrap_or(1);
                    (0..r as i64).map(|k| from + k).collect()
                }
            };
            for n in ns {
                let field = |this: &mut Self, key: &str, s: &str| -> Option<String> {
                    match expr::expand(s, n) {
                        Ok(v) => Some(v),
                        Err(msg) => {
                            this.error(&ep.key(key), msg);
                            None
                        }
                    }
                };
                let (Some(id), Some(terminal), Some(label), Some(group), Some(address)) = (
                    field(self, "id", &e.id),
                    field(self, "terminal", &e.terminal),
                    field(self, "label", &e.label),
                    field(self, "group", &e.group),
                    field(self, "address", &e.address),
                ) else {
                    break;
                };
                let description = match &e.description {
                    Some(d) => field(self, "description", d),
                    None => None,
                };
                let units = match &e.units {
                    Some(u) => field(self, "units", u),
                    None => None,
                };
                let at = if e.repeat.is_some() { format!(" (n = {n})") } else { String::new() };
                if id.trim().is_empty() {
                    self.error(&ep.key("id"), "must not be empty");
                }
                if terminal.trim().is_empty() {
                    self.error(&ep.key("terminal"), "must not be empty");
                }
                if let Some(first) = ids.insert(id.clone(), i) {
                    self.error(
                        &ep.key("id"),
                        format!("id `{id}`{at} is already used by io[{first}]"),
                    );
                }
                let ty = e.iec_type.trim().to_ascii_uppercase();
                let Some(a) = self.address(&address, &ep.key("address")) else {
                    continue;
                };
                let want = match e.dir {
                    Dir::In => Area::Input,
                    Dir::Out => Area::Output,
                    Dir::Mem => Area::Memory,
                };
                if a.area != want {
                    self.error(
                        &ep.key("address"),
                        format!(
                            "`{address}`{at} is in %{}, but dir = \"{}\" points live in %{}",
                            a.area.letter(),
                            dir_name(e.dir),
                            want.letter()
                        ),
                    );
                }
                let ok_types = types_for_size(a.size);
                if !ok_types.contains(&ty.as_str()) {
                    self.error(
                        &ep.key("type"),
                        format!("{ty} does not fit `{address}`{at}; a %_{} address holds {}", a.size.letter(), ok_types.join(" / ")),
                    );
                }
                match e.kind {
                    Kind::Digital if a.size != AddrSize::Bit => self.error(
                        &ep.key("kind"),
                        format!("a digital point needs a bit address (%_X), not `{address}`"),
                    ),
                    Kind::Analog if a.size == AddrSize::Bit => self.error(
                        &ep.key("kind"),
                        format!("an analog point needs a byte, word or larger address, not `{address}`"),
                    ),
                    _ => {}
                }
                let size = area_size(image, a.area);
                if a.end() > size as u64 {
                    self.error(
                        &ep.key("address"),
                        format!(
                            "`{address}`{at} ends at byte {}, past the {}-byte %{} area (target.image.{})",
                            a.end(),
                            size,
                            a.area.letter(),
                            a.area.letter()
                        ),
                    );
                }
                if let Some([lo, hi]) = e.range {
                    if lo >= hi {
                        self.error(&ep.key("range"), format!("[{lo}, {hi}]: the minimum must be below the maximum"));
                    }
                }
                if e.eng.is_some() && e.range.is_none() {
                    self.error(&ep.key("eng"), "`eng` scales the raw `range`; give `range` too");
                }
                if let Some([lo, hi]) = e.eng {
                    if !lo.is_finite() || !hi.is_finite() || lo == hi {
                        self.error(&ep.key("eng"), "must be two different finite numbers");
                    }
                }
                let canon = a.to_string();
                if let Some(other) = addrs.insert(canon.clone(), id.clone()) {
                    self.warn(
                        &ep.key("address"),
                        format!("`{id}` and `{other}` share {canon}"),
                    );
                }
                out.push(IoPoint {
                    id,
                    terminal,
                    label,
                    group,
                    dir: e.dir,
                    kind: e.kind,
                    iec_type: ty,
                    address: canon,
                    range: e.range,
                    eng: e.eng,
                    units,
                    description,
                });
            }
        }
        out
    }
}

fn area_size(image: &ImageSizes, area: Area) -> u32 {
    match area {
        Area::Input => image.i,
        Area::Output => image.q,
        Area::Memory => image.m,
    }
}

fn dir_name(d: Dir) -> &'static str {
    match d {
        Dir::In => "in",
        Dir::Out => "out",
        Dir::Mem => "mem",
    }
}

fn table_name(t: ModbusTable) -> &'static str {
    match t {
        ModbusTable::Coils => "coils",
        ModbusTable::Discrete => "discrete inputs",
        ModbusTable::Input => "input registers",
        ModbusTable::Holding => "holding registers",
    }
}
