// SPDX-License-Identifier: MPL-2.0

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::intrinsics::Intrinsic;
use inkwell::module::Module;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, StructType};
use inkwell::values::{BasicValue, BasicValueEnum, FunctionValue, GlobalValue, PointerValue};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate, OptimizationLevel};
use plcc_hir::check::TypeChecker;
use plcc_hir::types::{IecType, TypeRegistry};
use plcc_st::ast::*;
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

pub mod contract;
mod bits;
mod convert;
mod enums;
mod fault;
mod fbinit;
mod image;
mod interfaces;
mod ondemand;
mod oop;
mod refs;
mod shortcircuit;
mod stdfns;
mod strings;
pub use contract::{DeviceStamp, RuntimeContract, TaskOptions};

/// Name of the generated function that initializes VAR_GLOBAL FB instances.
const GLOBALS_INIT_FN: &str = "plcc_globals_init";

/// How one operand of a binary operator reads its bits.
///
/// `Adaptive` is a value with no static IEC type — a bare integer literal, or a call
/// whose result type codegen cannot name. It has no signedness of its own and takes
/// the other operand's; see [`Compiler::promote_int_operands`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Signedness {
    Signed,
    Unsigned,
    Adaptive,
}

/// Which way a FOR loop's control variable walks.
///
/// `Runtime` carries an i1 that is true when the step is negative — the only honest
/// answer for `BY st` where `st` is a variable.
enum StepDir<'ctx> {
    Up,
    Down,
    Runtime(inkwell::values::IntValue<'ctx>),
}

/// Parse a TIME literal string (e.g., "T#100ms", "T#1s500ms", "T#1h30m") into nanoseconds.
/// The characters of a STRING (`wide == false`) or WSTRING literal body, with the
/// IEC 61131-3 `$` escapes (Table 6) decoded: `$$`, `$'`, `$"`, `$L`/`$N` (line
/// feed), `$P` (form feed), `$R` (carriage return), `$T` (tab), and `$hh` (STRING)
/// or `$hhhh` (WSTRING) character codes in hex. A STRING's other characters are
/// its UTF-8 bytes; a WSTRING's are UTF-16 code units. An unrecognized escape is
/// kept as written.
fn decode_iec_string(raw: &str, wide: bool) -> Vec<u32> {
    let mut out = Vec::with_capacity(raw.len());
    let push_str = |out: &mut Vec<u32>, c: char| {
        if wide {
            let mut buf = [0u16; 2];
            out.extend(c.encode_utf16(&mut buf).iter().map(|&u| u as u32));
        } else {
            let mut buf = [0u8; 4];
            out.extend(c.encode_utf8(&mut buf).bytes().map(u32::from));
        }
    };
    let hex_len = if wide { 4 } else { 2 };
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '$' || i + 1 >= chars.len() {
            push_str(&mut out, c);
            i += 1;
            continue;
        }
        let e = chars[i + 1];
        let simple = match e.to_ascii_uppercase() {
            '$' => Some(0x24),
            '\'' => Some(0x27),
            '"' => Some(0x22),
            'L' | 'N' => Some(0x0A),
            'P' => Some(0x0C),
            'R' => Some(0x0D),
            'T' => Some(0x09),
            _ => None,
        };
        if let Some(code) = simple {
            out.push(code);
            i += 2;
            continue;
        }
        let hex: String = chars[i + 1..].iter().take(hex_len).collect();
        if hex.len() == hex_len && hex.chars().all(|h| h.is_ascii_hexdigit()) {
            if let Ok(code) = u32::from_str_radix(&hex, 16) {
                out.push(code);
                i += 1 + hex_len;
                continue;
            }
        }
        push_str(&mut out, c);
        i += 1;
    }
    out
}

/// Days from 1970-01-01 to the proleptic-Gregorian date `y-m-d` (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Nanoseconds since midnight for `hh:mm[:ss[.frac]]`.
fn parse_tod_ns(s: &str) -> Option<i64> {
    let mut parts = s.split(':');
    let h: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let (sec, frac_ns) = match parts.next() {
        None => (0, 0),
        Some(sec) => {
            let (whole, frac) = sec.split_once('.').unwrap_or((sec, ""));
            let whole: i64 = whole.parse().ok()?;
            let digits: String = frac.chars().take(9).collect();
            let frac_ns = if digits.is_empty() {
                0
            } else {
                digits.parse::<i64>().ok()? * 10i64.pow(9 - digits.len() as u32)
            };
            (whole, frac_ns)
        }
    };
    if parts.next().is_some() || h > 23 || m > 59 || sec > 59 {
        return None;
    }
    Some(((h * 60 + m) * 60 + sec) * 1_000_000_000 + frac_ns)
}

/// Nanoseconds since 1970-01-01 at midnight for `yyyy-mm-dd`.
fn parse_date_ns(s: &str) -> Option<i64> {
    let mut parts = s.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    days_from_civil(y, m, d).checked_mul(86_400_000_000_000)
}

/// The value of a DATE / TIME_OF_DAY / DATE_AND_TIME literal in plcc's storage
/// representation: i64 nanoseconds — since 1970-01-01T00:00 for DATE and DT, since
/// midnight for TOD. `None` for a malformed literal.
fn parse_date_time_literal_ns(kind: &ExpressionKind) -> Option<i64> {
    let body = |raw: &str| raw.split_once('#').map(|(_, b)| b.trim().to_string());
    match kind {
        ExpressionKind::DateLiteral(raw) => parse_date_ns(&body(raw)?),
        ExpressionKind::TodLiteral(raw) => parse_tod_ns(&body(raw)?),
        ExpressionKind::DtLiteral(raw) => {
            let b = body(raw)?;
            // yyyy-mm-dd-hh:mm[:ss[.f]]: the date is the first three '-' fields.
            let mut it = b.splitn(4, '-');
            let date = format!("{}-{}-{}", it.next()?, it.next()?, it.next()?);
            parse_date_ns(&date)?.checked_add(parse_tod_ns(it.next()?)?)
        }
        _ => None,
    }
}

/// `digits` (`213503`, `1.5`) of a unit worth `multiplier` ns, in ns. The whole
/// part is exact (f64 is not, above 2^53 ns: `LTIME#213503d...` came out a
/// minute short); the fraction is rounded, not truncated (`1.1 * 1e9` is
/// 1100000000.0000002 and `0.3 * 1e3` is 299.99999999999994). `None` when
/// it is far out of range.
fn duration_part(digits: &str, multiplier: i128) -> Option<i128> {
    let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let whole: i128 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
    let whole = whole.checked_mul(multiplier).filter(|v| *v <= u64::MAX as i128 * 2)?;
    let frac = if frac.is_empty() {
        0
    } else {
        let f: f64 = format!("0.{frac}").parse().ok()?;
        (f * multiplier as f64).round() as i128
    };
    Some(whole + frac)
}

/// Nanoseconds of a TIME/LTIME literal, or why it is malformed.
fn parse_time_literal_ns(s: &str) -> Result<i64, String> {
    let s = s.trim();
    let long = ["LTIME#", "LT#"]
        .iter()
        .any(|p| s.len() > p.len() && s[..p.len()].eq_ignore_ascii_case(p));
    // Strip the duration prefix. IEC 61131-3 Annex A B.1.2.3 allows T#, LT#,
    // TIME# and LTIME#; the lexer accepts all four, so all four must be
    // stripped here or the prefix letters would be misread as unit suffixes.
    // Longest first, so `LTIME#`/`TIME#` are not truncated to `LT#`/`T#`.
    let s = ["LTIME#", "TIME#", "LT#", "T#"]
        .iter()
        .find_map(|p| {
            (s.len() > p.len() && s[..p.len()].eq_ignore_ascii_case(p)).then(|| &s[p.len()..])
        })
        .unwrap_or(s);
    // `T#-14ms`: the sign applies to the whole duration.
    let (negative, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    const TOO_LONG: &str = "the duration does not fit in 64-bit nanoseconds (about 106751 days)";
    let mut ns: i128 = 0;
    let mut num_buf = String::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() || c == '.' || c == '_' {
            if c != '_' {
                num_buf.push(c);
            }
            chars.next();
        } else {
            let digits = std::mem::take(&mut num_buf);
            // Read unit suffix
            let mut unit = String::new();
            while let Some(&u) = chars.peek() {
                if u.is_ascii_alphabetic() {
                    unit.push(u);
                    chars.next();
                } else {
                    break;
                }
            }
            if unit.is_empty() {
                return Err(format!("unexpected `{c}` in a duration"));
            }
            // An unknown unit was read as 0 — `T#5x` was a silent T#0s.
            let multiplier: i128 = match unit.to_lowercase().as_str() {
                "d" => 86_400_000_000_000,
                "h" => 3_600_000_000_000,
                "m" => 60_000_000_000,
                "s" => 1_000_000_000,
                "ms" => 1_000_000,
                "us" => 1_000,
                "ns" => 1,
                other => return Err(format!("`{other}` is not a duration unit (d, h, m, s, ms, us, ns)")),
            };
            ns += duration_part(&digits, multiplier).ok_or(TOO_LONG)?;
        }
    }
    // Handle trailing number with no unit (assume ms for bare numbers)
    if !num_buf.is_empty() {
        // default ms
        ns += duration_part(&num_buf, 1_000_000).ok_or(TOO_LONG)?;
    }
    let ns = if negative { -ns } else { ns };
    match i64::try_from(ns) {
        Ok(v) => Ok(v),
        // CODESYS/TwinCAT LTIME is an unsigned 64-bit count of nanoseconds, up to
        // LTIME#213503d23h34m33s709ms551us615ns. plcc holds durations signed:
        // keep the bit pattern, so such a value is stored and compared for
        // equality exactly (ordering against it is signed).
        Err(_) if long && (0..=u64::MAX as i128).contains(&ns) => Ok(ns as u64 as i64),
        Err(_) => Err(TOO_LONG.into()),
    }
}

#[derive(Debug, Error)]
pub enum CodegenError {
    #[error("unsupported type: {0}")]
    UnsupportedType(String),
    #[error("undefined variable: {0}")]
    UndefinedVariable(String),
    #[error("LLVM error: {0}")]
    LlvmError(String),
    #[error("target error: {0}")]
    TargetError(String),
    /// A variable was declared with a type name that resolves to nothing: not an
    /// elementary type, not a user TYPE, and not a FUNCTION_BLOCK/CLASS in scope.
    ///
    /// This used to be swallowed — the declaration silently became an `i32` slot and
    /// every statement touching it was dropped from the generated code, so a program
    /// using `TON` without a `TON` definition compiled to an empty `scan()`. It is now
    /// a hard error.
    #[error(
        "unknown type `{type_name}` for variable `{var_name}` in {pou_kind} `{pou_name}` \
         (no elementary type, TYPE declaration, FUNCTION_BLOCK or CLASS with that name is \
         in scope; standard function blocks come from the bundled stdlib — check --stdlib)"
    )]
    UnknownType {
        pou_kind: &'static str,
        pou_name: String,
        var_name: String,
        type_name: String,
    },
    /// An expression that must produce a value (a builtin's argument, an IF
    /// condition, a FOR bound, an array index, ...) produced none.
    ///
    /// This used to be reported as `failed to compile argument for MIN`, which blamed
    /// the enclosing builtin for what was really an unknown identifier or an undefined
    /// nested function inside the argument. `what` names the slot, `expr` the
    /// expression in it, and `reason` the leaf that actually failed.
    #[error("{what} (`{expr}`) produced no value: {reason} (source offset {offset})")]
    NoValue {
        what: String,
        expr: String,
        reason: String,
        offset: usize,
    },
    /// A `TYPE` declaration never becomes layout-complete: a cycle, or a name that
    /// resolves to nothing.
    #[error(
        "TYPE `{type_name}` cannot be laid out: `{unresolved}` never resolves \
         (a cyclic TYPE definition, or a name that is not declared anywhere; \
         a type that refers to itself must go through POINTER TO)"
    )]
    CyclicType {
        type_name: String,
        unresolved: String,
    },
    /// Two `TYPE` declarations share a name. ST identifiers are case-insensitive, so
    /// `Foo` and `foo` are the same type — the second silently replaced the first and
    /// the program failed much later with something unrelated, typically
    /// `undefined variable: a` from a field that only the shadowed declaration had.
    #[error(
        "TYPE `{second}` is already declared as `{first}` \
         (ST identifiers are case-insensitive, so these are the same name)"
    )]
    DuplicateType { first: String, second: String },
    /// A call's arguments cannot be bound to the callee's declared parameters.
    ///
    /// IEC 61131-3 allows a call to name its arguments (`F(b := 3, a := 10)`), and the
    /// names — not the positions — decide the binding. Silently ignoring the names and
    /// binding positionally is a wrong answer with no diagnostic, so anything that
    /// cannot be bound unambiguously is an error here.
    #[error("in the call to `{callee}`: {problem}")]
    ArgumentBinding { callee: String, problem: String },
    /// The generated module failed LLVM's own verifier.
    ///
    /// Structural IR defects (a terminator in the middle of a block, a block with no
    /// terminator, a type mismatch) are invisible at `OptimizationLevel::None` — the
    /// JIT the execution tests use will happily run past them — and only surface as a
    /// hang or a miscompile once an optimizing backend gets hold of the module. So the
    /// verifier runs at the end of every `compile()`, not just before object emission.
    #[error("generated LLVM module failed verification: {0}")]
    InvalidModule(String),
    /// A diagnostic tied to a source location (a bad direct address, an AT on an
    /// incompatible type, a CONFIGURATION error). See [`CodegenError::span`].
    #[error("{message}")]
    Located {
        message: String,
        span: plcc_st::span::Span,
    },
}

/// A declared formal parameter of a FUNCTION or METHOD.
#[derive(Clone, Debug)]
struct Param {
    name: String,
    /// The declared type — what the parameter *is* inside the callee.
    ty: IecType,
    /// VAR_IN_OUT. IEC 61131-3 defines these as pass-by-reference, so the LLVM
    /// parameter is a pointer to the caller's variable rather than a copy of it.
    is_in_out: bool,
    /// VAR_OUTPUT of a FUNCTION or METHOD. Passed as a pointer to a caller-owned
    /// temporary; the caller copies it to the `=>` target after the call.
    is_output: bool,
}

/// Information about a compiled method on an FB/Class.
#[derive(Clone, Debug)]
struct MethodInfo {
    /// The LLVM function name for this method (e.g. "counter_add").
    fn_name: String,
    /// Declared parameters in order (excludes the implicit instance pointer).
    params: Vec<Param>,
    /// Return type (Void if the method doesn't return a value).
    return_type: IecType,
}

/// One field of a POU's state struct.
#[derive(Clone, Debug)]
struct PouField {
    name: String,
    /// The declared type — what the variable *is* inside the POU.
    declared: IecType,
    /// How the state struct stores it. Same as `declared`, except a VAR_IN_OUT is
    /// stored as `Pointer(declared)`: the struct holds the caller's address, because
    /// IEC 61131-3 defines VAR_IN_OUT as pass-by-reference.
    stored: IecType,
    is_in_out: bool,
    /// The variable block the field was declared in; `None` for a STRUCT member.
    /// Decides which of `:=` / `=>` a call may bind it with.
    block: Option<VarBlockKind>,
    /// `AT %..`: the variable lives at this process-image location. Its slot in the
    /// state struct is kept (so field indices do not shift) but never used.
    at: Option<crate::direct_address::DirectAddress>,
}

impl PouField {
    /// A field that is stored exactly as declared.
    fn plain(name: String, ty: IecType) -> Self {
        Self {
            name,
            declared: ty.clone(),
            stored: ty,
            is_in_out: false,
            block: None,
            at: None,
        }
    }
}

/// Layout information for a compiled function block.
#[derive(Clone, Debug)]
struct FbLayout<'ctx> {
    struct_type: StructType<'ctx>,
    scan_fn_name: String,
    /// Name of the generated `<pou>_init` function that applies declared initial
    /// values to an instance (and recursively initializes nested FB instances).
    init_fn_name: String,
    /// Ordered state-struct fields (inputs, outputs, in-outs, locals — all in
    /// declaration order).
    fields: Vec<PouField>,
    /// Compiled methods, keyed by uppercase method name.
    methods: HashMap<String, MethodInfo>,
}

/// Something that has to be initialized at a fixed position inside a value.
///
/// The position is a chain of constant GEP indices from the start of the value, so
/// one flat list covers a STRUCT field, a field of a nested STRUCT, and an element of
/// an array of STRUCTs alike.
#[derive(Clone, Debug)]
enum FieldInit {
    /// Store a declared default: `TYPE S : STRUCT a : DINT := 5; ...`.
    Store(Expression, IecType),
    /// Call a nested FB instance's `<fb>_init`, for an FB held in a STRUCT field.
    InitFb(String),
}

pub struct Compiler<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    variables: HashMap<String, (PointerValue<'ctx>, IecType)>,
    type_registry: TypeRegistry,
    type_checker: TypeChecker,
    /// Target block for EXIT statements inside loops.
    loop_exit_bb: Option<inkwell::basic_block::BasicBlock<'ctx>>,
    /// Target block for CONTINUE statements inside loops.
    ///
    /// This is the block that resumes the *next* iteration, which is not the same as
    /// the loop's condition test: a FOR loop has to run its increment first, or
    /// `CONTINUE` would spin on the same control value forever.
    loop_continue_bb: Option<inkwell::basic_block::BasicBlock<'ctx>>,
    /// Target block for RETURN: the POU's single exit, where the epilogue (load the
    /// result, `ret`) is emitted. `None` outside a POU body.
    return_bb: Option<inkwell::basic_block::BasicBlock<'ctx>>,
    /// Uppercase name of the result variable of the FUNCTION/METHOD being compiled,
    /// so `RETURN expr;` (a common vendor extension) can store into it.
    return_result_var: Option<String>,
    /// Global variables: (global LLVM value, struct type, field names+types).
    global_var: Option<(GlobalValue<'ctx>, StructType<'ctx>, Vec<(String, IecType)>)>,
    /// Compiled FB layouts, keyed by uppercase FB type name.
    compiled_fbs: HashMap<String, FbLayout<'ctx>>,
    /// The `TypeSpec` of every user `TYPE` declaration, keyed by uppercase name.
    ///
    /// [`IecType`] carries no initializers — `IecType::Struct` is just an ordered
    /// `(name, type)` list — so a STRUCT field's declared default is not recoverable
    /// from the resolved type. It has to be read back off the AST, which is what this
    /// is for. Duplicate `TYPE` names are rejected before this is populated, so the
    /// mapping is unambiguous.
    type_specs: HashMap<String, TypeSpec>,
    /// Counter for the synthetic names `copy_output` binds an output slot to.
    output_copy_seq: u32,
    /// Declared parameters of every user FUNCTION, keyed by lowercased name.
    ///
    /// Recorded before any body is compiled so a call site can bind its arguments by
    /// name and coerce them to the declared parameter types. Without this an `INT`
    /// literal passed to a `DINT` parameter reached `build_call` unconverted and
    /// LLVM's verifier rejected the module — `F(5)` against
    /// `FUNCTION F : DINT VAR_INPUT x : DINT`.
    fn_signatures: HashMap<String, Vec<Param>>,
    /// Declared result type of every user FUNCTION that has one, keyed by lowercased
    /// name, so a call's static IEC type is known (see [`Self::rvalue_iec_type`]).
    fn_return_types: HashMap<String, IecType>,
    /// The parent struct type for the current POU being compiled (needed for GEP on FB instances).
    current_struct_type: Option<StructType<'ctx>>,
    /// The state pointer for the current POU being compiled.
    current_state_ptr: Option<PointerValue<'ctx>>,
    /// Why the most recent `compile_expression` leaf returned `Ok(None)`, and the
    /// span of that leaf. `Ok(None)` propagates outward through operators and calls
    /// carrying no information; the site that finally needs a value reads this back
    /// (see [`Self::no_value_error`]) so it can name the innermost culprit instead of
    /// itself.
    no_value_cause: Option<(plcc_st::span::Span, String)>,
    /// Process image, AT bindings, tasks: the runtime contract (see `image.rs`,
    /// `contract.rs`).
    rt: image::ContractState<'ctx>,
    /// Enumerators of every enumeration in the unit (see `enums.rs`).
    enums: enums::EnumTable,
    /// `EXTENDS`: bases and inherited-method origins (see `oop.rs`).
    hierarchy: oop::Hierarchy,
    /// Without a CONFIGURATION: each PROGRAM's single instance, by uppercase name,
    /// so another POU can call it (`Sub();`) and read it (`Sub.out`).
    program_instances: HashMap<String, GlobalValue<'ctx>>,
    /// Programs some POU calls: they run when called, not on their own.
    called_programs: std::collections::HashSet<String>,
    /// FB_init parameters' declared initial values, per FB (see `fbinit`).
    fb_init_defaults: fbinit::Defaults,
    /// INTERFACE method tables and IMPLEMENTS sets (see `interfaces.rs`).
    interfaces: interfaces::InterfaceTable,
    /// (uppercase FB/CLASS, uppercase input) for every `REFERENCE TO` input: a
    /// call binds it to its argument's address (see refs.rs).
    reference_inputs: std::collections::HashSet<(String, String)>,
    /// Uppercase name of the FB/CLASS whose body or method is being compiled, for
    /// calls to its methods written without `THIS^.`.
    current_pou: Option<String>,
    /// Runtime fault sites (see `fault.rs`).
    fault: fault::FaultState<'ctx>,
    /// LLVM CPU and feature string for every target machine (`--cpu`, `--features`).
    machine: MachineOptions,
}

/// The CPU and target features code is generated for, beyond the triple.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineOptions {
    /// LLVM CPU name; `generic` is the triple's baseline.
    pub cpu: String,
    /// Comma-separated LLVM features (`+fp-armv8d16,-neon`).
    pub features: String,
}

impl Default for MachineOptions {
    fn default() -> Self {
        MachineOptions {
            cpu: "generic".into(),
            features: String::new(),
        }
    }
}

impl<'ctx> Compiler<'ctx> {
    pub fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        let builder = context.create_builder();
        Self {
            context,
            module,
            builder,
            variables: HashMap::new(),
            type_registry: TypeRegistry::new(),
            type_checker: TypeChecker::new(),
            loop_exit_bb: None,
            loop_continue_bb: None,
            return_bb: None,
            return_result_var: None,
            global_var: None,
            compiled_fbs: HashMap::new(),
            type_specs: HashMap::new(),
            fn_signatures: HashMap::new(),
            output_copy_seq: 0,
            fn_return_types: HashMap::new(),
            current_struct_type: None,
            current_state_ptr: None,
            no_value_cause: None,
            rt: image::ContractState::default(),
            enums: enums::EnumTable::default(),
            hierarchy: oop::Hierarchy::default(),
            program_instances: HashMap::new(),
            called_programs: std::collections::HashSet::new(),
            fb_init_defaults: Default::default(),
            interfaces: interfaces::InterfaceTable::default(),
            reference_inputs: std::collections::HashSet::new(),
            current_pou: None,
            fault: fault::FaultState::default(),
            machine: MachineOptions::default(),
        }
    }

    /// Generate code for this CPU and these target features (LLVM names, e.g.
    /// `cortex-m7` and `+fp-armv8d16`). Applies to every target machine the
    /// compiler creates and is recorded on each function as `target-cpu` /
    /// `target-features`, so an `.ll`/`.bc` optimized or compiled later keeps it.
    pub fn set_machine_options(&mut self, opts: MachineOptions) {
        self.machine = opts;
    }

    pub fn machine_options(&self) -> &MachineOptions {
        &self.machine
    }

    /// Register LLVM intrinsic declarations for standard math functions.
    fn register_standard_functions(&self) {
        let f32_ty: BasicTypeEnum = self.context.f32_type().into();
        let f64_ty: BasicTypeEnum = self.context.f64_type().into();

        let intrinsics_f32_f64 = [
            "llvm.fabs",
            "llvm.sqrt",
            "llvm.sin",
            "llvm.cos",
            "llvm.exp",
            "llvm.log",
            "llvm.pow",
            "llvm.floor",
            "llvm.ceil",
            "llvm.trunc",
        ];

        for name in &intrinsics_f32_f64 {
            if let Some(intr) = Intrinsic::find(name) {
                intr.get_declaration(&self.module, &[f32_ty]);
                intr.get_declaration(&self.module, &[f64_ty]);
            }
        }

        let i16_ty: BasicTypeEnum = self.context.i16_type().into();
        let i32_ty: BasicTypeEnum = self.context.i32_type().into();
        for name in &["llvm.fshl", "llvm.fshr"] {
            if let Some(intr) = Intrinsic::find(name) {
                intr.get_declaration(&self.module, &[i16_ty]);
                intr.get_declaration(&self.module, &[i32_ty]);
            }
        }
    }

    /// Operands for SHL / SHR / ROL / ROR: the value stays at **its own** width and
    /// the distance is brought to it.
    ///
    /// Widening the value to the distance's width instead was silently wrong twice
    /// over. It sign-extended: `SHR(b, 1)` with `b : BYTE := 254` widened 254 to
    /// i16 as -2, and the logical shift then answered 32767 instead of 127. And it
    /// changed the rotation width, so `ROL(b, 1)` rotated within 16 bits, moving in
    /// bits that were never part of the BYTE. IEC 61131-3 defines all four on the
    /// type of IN, with N only a count.
    fn shift_operands(
        &self,
        arg_vals: &[BasicValueEnum<'ctx>],
        arg_tys: &[Option<IecType>],
    ) -> Result<
        (
            inkwell::values::IntValue<'ctx>,
            inkwell::values::IntValue<'ctx>,
        ),
        CodegenError,
    > {
        let (Some(&val), Some(&n)) = (arg_vals.first(), arg_vals.get(1)) else {
            return Err(CodegenError::LlvmError(format!(
                "a shift or rotate expects 2 arguments, got {}",
                arg_vals.len()
            )));
        };
        let val = self.int_operand(val, "IN of a shift or rotate")?;
        // IN of a narrow type computed at native width (`ROL(b + 1, 1)`: integer
        // temporaries are at least 32 bits) shifts and rotates at its type's
        // width, not the temporary's.
        let val = match arg_tys
            .first()
            .and_then(|t| t.as_ref())
            .and_then(|t| t.base().bit_size())
        {
            Some(bits) if bits > 1 && bits < val.get_type().get_bit_width() => self
                .builder
                .build_int_truncate(val, self.context.custom_width_int_type(bits), "shift_in")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
            _ => val,
        };
        let n = self.int_operand(n, "N of a shift or rotate")?;
        let n = if n.get_type().get_bit_width() < 8 {
            self.widen_to(n, Signedness::Unsigned, self.context.i8_type())?
        } else {
            n
        };
        let n_sign = Self::signedness_of(arg_tys.get(1).and_then(|t| t.as_ref()));
        let target = val.get_type();
        let n = match n.get_type().get_bit_width().cmp(&target.get_bit_width()) {
            std::cmp::Ordering::Equal => n,
            std::cmp::Ordering::Less => self.widen_to(n, n_sign, target)?,
            std::cmp::Ordering::Greater => self
                .builder
                .build_int_truncate(n, target, "shiftn")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
        };
        Ok((val, n))
    }

    /// `SHL(IN, N)` / `SHR(IN, N)`: zero-filling shifts. Shifting by the width of IN
    /// or more (or by a negative N) yields 0 — every bit has been shifted out.
    /// LLVM's `shl`/`lshr` by the width or more is poison; x86 masks the count, so
    /// `SHL(dint, 32)` returned the input unchanged, and an N wider than IN was
    /// truncated first, so `SHL(byte, 256)` was a shift by 0.
    fn checked_shift(
        &self,
        arg_vals: &[BasicValueEnum<'ctx>],
        arg_tys: &[Option<IecType>],
        left: bool,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let raw_n = arg_vals
            .get(1)
            .map(|v| self.int_operand(*v, "N of a shift"))
            .transpose()?;
        let (val, n) = self.shift_operands(arg_vals, arg_tys)?;
        let width = val.get_type().get_bit_width() as u64;
        // Compare the count before it was narrowed to IN's width, unsigned: a
        // negative N is huge.
        let raw_n = raw_n.unwrap_or(n);
        let raw_n = if raw_n.get_type().get_bit_width() < 8 {
            self.widen_to(raw_n, Signedness::Unsigned, self.context.i8_type())?
        } else {
            raw_n
        };
        let too_far = self
            .builder
            .build_int_compare(
                IntPredicate::UGE,
                raw_n,
                raw_n.get_type().const_int(width, false),
                "shift_out",
            )
            .map_err(err)?;
        let zero = val.get_type().const_zero();
        let safe_n = self.builder.build_select(too_far, zero, n, "shift_n").map_err(err)?;
        let shifted = if left {
            self.builder
                .build_left_shift(val, safe_n.into_int_value(), "shl")
                .map_err(err)?
        } else {
            self.builder
                .build_right_shift(val, safe_n.into_int_value(), false, "shr")
                .map_err(err)?
        };
        Ok(self
            .builder
            .build_select(too_far, zero, shifted, "shift")
            .map_err(err)?
            .into_int_value())
    }

    /// Try to compile a call to a standard library function.
    /// Returns `Ok(Some(val))` if handled, `Ok(None)` if not a known stdlib function.
    fn compile_stdlib_call(
        &mut self,
        name: &str,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let uname = name.to_uppercase();

        // `STRING_TO_<int>`: parse the text.
        if Self::string_to_target(&uname).is_some() {
            return self.compile_string_to_int(&uname, args, function);
        }
        // `<SRC>_TO_<DST>` / `TO_<DST>` between any two elementary types.
        if Self::parse_conversion(&uname).is_some() {
            return self.compile_conversion_call(&uname, args, function);
        }

        // The arguments' static IEC types, for the builtins whose lowering depends on
        // signedness (the shift and rotate family).
        let arg_tys: Vec<Option<IecType>> = args
            .iter()
            .map(|arg| self.rvalue_iec_type(&arg.value))
            .collect();

        let mut arg_vals: Vec<BasicValueEnum<'ctx>> = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            if let Some(val) = self.compile_expression(&arg.value, function)? {
                arg_vals.push(val);
            } else {
                // Blame the argument, not the builtin: `MIN(x, LANGUAGE.LMAX)` fails
                // because `LANGUAGE` is unknown, not because MIN is missing.
                let slot = match &arg.name {
                    Some(n) => format!("argument `{}` of `{uname}`", n.name),
                    None => format!("argument {} of `{uname}`", i + 1),
                };
                return Err(self.no_value_error(slot, &arg.value));
            }
        }

        match uname.as_str() {
            // The platform clock, exposed to ST. Returns LINT/TIME nanoseconds.
            // `MONOTONIC_NS` is the ST-facing spelling; `PLCC_MONOTONIC_NS` matches the
            // symbol name for anyone who prefers to be explicit about the import.
            "MONOTONIC_NS" | "PLCC_MONOTONIC_NS" => {
                if !arg_vals.is_empty() {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} takes no arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let clock_fn = self.get_or_declare_monotonic_ns();
                let call = self
                    .builder
                    .build_call(clock_fn, &[], "monons")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Ok(Some(v)),
                    inkwell::values::ValueKind::Instruction(_) => Err(CodegenError::LlvmError(
                        "plcc_monotonic_ns returned no value".into(),
                    )),
                }
            }
            "SQRT" | "SIN" | "COS" | "EXP" | "LN" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument, got {}",
                        arg_vals.len()
                    )));
                }
                let arg = self.ensure_float(arg_vals[0], arg_tys.get(0).and_then(Option::as_ref))?;
                let intrinsic_name = match uname.as_str() {
                    "SQRT" => "llvm.sqrt",
                    "SIN" => "llvm.sin",
                    "COS" => "llvm.cos",
                    "EXP" => "llvm.exp",
                    "LN" => "llvm.log",
                    _ => unreachable!(),
                };
                let fty = arg.get_type();
                let intr = Intrinsic::find(intrinsic_name).ok_or_else(|| {
                    CodegenError::LlvmError(format!("intrinsic {intrinsic_name} not found"))
                })?;
                let fn_val = intr
                    .get_declaration(&self.module, &[fty.into()])
                    .ok_or_else(|| {
                        CodegenError::LlvmError(format!(
                            "failed to get declaration for {intrinsic_name}"
                        ))
                    })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[arg.into()], &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from intrinsic".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }

            "EXPT" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "EXPT expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let result = self.compile_expt(
                    arg_vals[0],
                    arg_tys.first().and_then(|t| t.as_ref()),
                    arg_vals[1],
                    arg_tys.get(1).and_then(|t| t.as_ref()),
                )?;
                Ok(Some(result))
            }

            "ABS" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "ABS expects 1 argument, got {}",
                        arg_vals.len()
                    )));
                }
                let arg = arg_vals[0];
                if arg.is_float_value() {
                    let fv = arg.into_float_value();
                    let fty = fv.get_type();
                    let intr = Intrinsic::find("llvm.fabs").ok_or_else(|| {
                        CodegenError::LlvmError("intrinsic llvm.fabs not found".into())
                    })?;
                    let fn_val = intr
                        .get_declaration(&self.module, &[fty.into()])
                        .ok_or_else(|| {
                            CodegenError::LlvmError("failed to get llvm.fabs declaration".into())
                        })?;
                    let call_result = self
                        .builder
                        .build_call(fn_val, &[fv.into()], "fabs")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .try_as_basic_value();
                    let result = match call_result {
                        inkwell::values::ValueKind::Basic(v) => v,
                        _ => {
                            return Err(CodegenError::LlvmError(
                                "expected return value from fabs intrinsic".into(),
                            ));
                        }
                    };
                    Ok(Some(result))
                } else {
                    let iv = self.int_operand(arg, &uname)?;
                    // An ANY_BIT or ANY_UNSIGNED value is already its own magnitude.
                    // Negating it read `b : BYTE := 200` as -56 and answered 56.
                    if Self::signedness_of(arg_tys[0].as_ref()) == Signedness::Unsigned {
                        return Ok(Some(iv.into()));
                    }
                    let zero = iv.get_type().const_zero();
                    let is_neg = self
                        .builder
                        .build_int_compare(IntPredicate::SLT, iv, zero, "is_neg")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let neg_val = self
                        .builder
                        .build_int_neg(iv, "neg")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_select(is_neg, neg_val, iv, "abs")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(result.into()))
                }
            }

            "MIN" | "MAX" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let a = arg_vals[0];
                let b = arg_vals[1];
                let is_max = uname == "MAX";

                if a.is_float_value() || b.is_float_value() {
                    let fa = self.ensure_float(a, arg_tys.get(0).and_then(Option::as_ref))?;
                    let fb = self.ensure_float(b, arg_tys.get(1).and_then(Option::as_ref))?;
                    let (fa, fb) = self.match_float_widths(fa, fb)?;
                    let pred = if is_max {
                        FloatPredicate::OGT
                    } else {
                        FloatPredicate::OLT
                    };
                    let cmp = self
                        .builder
                        .build_float_compare(pred, fa, fb, "cmp")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_select(cmp, fa, fb, &uname.to_lowercase())
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(result.into()))
                } else {
                    // Same rule as a relational operator: two unsigned operands
                    // compare unsigned, or `MAX(b, 100)` with `b : BYTE := 200`
                    // answers 100.
                    let (ia, ib, unsigned) = self.prepare_int_operands(
                        self.int_operand(a, &uname)?,
                        arg_tys[0].as_ref(),
                        self.int_operand(b, &uname)?,
                        arg_tys[1].as_ref(),
                    )?;
                    let pred = match (is_max, unsigned) {
                        (true, true) => IntPredicate::UGT,
                        (true, false) => IntPredicate::SGT,
                        (false, true) => IntPredicate::ULT,
                        (false, false) => IntPredicate::SLT,
                    };
                    let cmp = self
                        .builder
                        .build_int_compare(pred, ia, ib, "cmp")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_select(cmp, ia, ib, &uname.to_lowercase())
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(result.into()))
                }
            }

            "LIMIT" => {
                if arg_vals.len() != 3 {
                    return Err(CodegenError::LlvmError(format!(
                        "LIMIT expects 3 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let mn = arg_vals[0];
                let val = arg_vals[1];
                let mx = arg_vals[2];

                if val.is_float_value() || mn.is_float_value() || mx.is_float_value() {
                    let fmn = self.ensure_float(mn, arg_tys.get(0).and_then(Option::as_ref))?;
                    let fval = self.ensure_float(val, arg_tys.get(1).and_then(Option::as_ref))?;
                    let fmx = self.ensure_float(mx, arg_tys.get(2).and_then(Option::as_ref))?;
                    // One float width for all three, or a REAL bound against an LREAL
                    // input builds a mistyped compare.
                    let (fmn, fval) = self.match_float_widths(fmn, fval)?;
                    let (fval, fmx) = self.match_float_widths(fval, fmx)?;
                    let (fmn, fval) = self.match_float_widths(fmn, fval)?;
                    let cmp_hi = self
                        .builder
                        .build_float_compare(FloatPredicate::OLT, fval, fmx, "cmp_hi")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let clamped_hi = self
                        .builder
                        .build_select(cmp_hi, fval, fmx, "clamp_hi")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into_float_value();
                    let cmp_lo = self
                        .builder
                        .build_float_compare(FloatPredicate::OGT, clamped_hi, fmn, "cmp_lo")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_select(cmp_lo, clamped_hi, fmn, "limit")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(result.into()))
                } else {
                    // LIMIT clamps all three operands in one shared representation, so
                    // the width and the signedness are settled once over the triple
                    // rather than pairwise — two pairwise widenings would leave MN and
                    // MX at different widths from IN. Comparing an unsigned bound with
                    // SLT clamped `LIMIT(v, 200, 200)` on BYTEs down to 100.
                    let ([imn, ival, imx], unsigned) = self.prepare_int_triple(
                        [
                            self.int_operand(mn, &uname)?,
                            self.int_operand(val, &uname)?,
                            self.int_operand(mx, &uname)?,
                        ],
                        [
                            arg_tys[0].as_ref(),
                            arg_tys[1].as_ref(),
                            arg_tys[2].as_ref(),
                        ],
                    )?;
                    let cmp_hi = self
                        .builder
                        .build_int_compare(
                            if unsigned {
                                IntPredicate::ULT
                            } else {
                                IntPredicate::SLT
                            },
                            ival,
                            imx,
                            "cmp_hi",
                        )
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let clamped_hi = self
                        .builder
                        .build_select(cmp_hi, ival, imx, "clamp_hi")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into_int_value();
                    let cmp_lo = self
                        .builder
                        .build_int_compare(
                            if unsigned {
                                IntPredicate::UGT
                            } else {
                                IntPredicate::SGT
                            },
                            clamped_hi,
                            imn,
                            "cmp_lo",
                        )
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_select(cmp_lo, clamped_hi, imn, "limit")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(result.into()))
                }
            }

            "SEL" => {
                if arg_vals.len() != 3 {
                    return Err(CodegenError::LlvmError(format!(
                        "SEL expects 3 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let g = self.int_operand(arg_vals[0], &uname)?;
                let (mut in0, mut in1) = (arg_vals[1], arg_vals[2]);
                // IN0 and IN1 of different types (`SEL(g, int_var, dint_var)`) are
                // brought to their common type first; selecting between an i16 and
                // an i32 is invalid IR.
                if in0.get_type() != in1.get_type() {
                    let common = Self::arith_result_type(arg_tys[1].clone(), arg_tys[2].clone())
                        .or_else(|| {
                            if in0.is_float_value() || in1.is_float_value() {
                                Some(IecType::Lreal)
                            } else {
                                let w = in0
                                    .into_int_value()
                                    .get_type()
                                    .get_bit_width()
                                    .max(in1.into_int_value().get_type().get_bit_width());
                                Some(match w {
                                    0..=8 => IecType::Sint,
                                    9..=16 => IecType::Int,
                                    17..=32 => IecType::Dint,
                                    _ => IecType::Lint,
                                })
                            }
                        })
                        .ok_or_else(|| {
                            CodegenError::UnsupportedType(
                                "SEL: IN0 and IN1 have no common type".into(),
                            )
                        })?;
                    in0 = self.coerce_value(in0, arg_tys[1].as_ref(), &common)?;
                    in1 = self.coerce_value(in1, arg_tys[2].as_ref(), &common)?;
                }
                let cond = self.to_i1(g)?;
                let result = self
                    .builder
                    .build_select(cond, in1, in0, "sel")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(result.into()))
            }

            "SHL" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "SHL expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                Ok(Some(self.checked_shift(&arg_vals, &arg_tys, true)?.into()))
            }
            "SHR" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "SHR expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                // IEC 61131-3 SHR is defined on ANY_BIT: bits vacated at the top are
                // filled with zeros, never with a sign. There is no arithmetic-shift
                // spelling in the standard — a signed right shift is written by
                // dividing — so it is a logical shift for every input type.
                Ok(Some(self.checked_shift(&arg_vals, &arg_tys, false)?.into()))
            }

            "ROL" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "ROL expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let (val, n) = self.shift_operands(&arg_vals, &arg_tys)?;
                let ity: BasicTypeEnum = val.get_type().into();
                let intr = Intrinsic::find("llvm.fshl").ok_or_else(|| {
                    CodegenError::LlvmError("intrinsic llvm.fshl not found".into())
                })?;
                let fn_val = intr.get_declaration(&self.module, &[ity]).ok_or_else(|| {
                    CodegenError::LlvmError("failed to get llvm.fshl declaration".into())
                })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[val.into(), val.into(), n.into()], "rol")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from intrinsic".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }
            "ROR" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "ROR expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let (val, n) = self.shift_operands(&arg_vals, &arg_tys)?;
                let ity: BasicTypeEnum = val.get_type().into();
                let intr = Intrinsic::find("llvm.fshr").ok_or_else(|| {
                    CodegenError::LlvmError("intrinsic llvm.fshr not found".into())
                })?;
                let fn_val = intr.get_declaration(&self.module, &[ity]).ok_or_else(|| {
                    CodegenError::LlvmError("failed to get llvm.fshr declaration".into())
                })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[val.into(), val.into(), n.into()], "ror")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from intrinsic".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }

            // IEC 61131-3 TRUNC: ANY_REAL -> ANY_INT, truncating toward zero. The
            // result is a DINT, as in CODESYS; OSCAT's RANGE_TO_BYTE writes
            // `INT_TO_BYTE(TRUNC(x))`. It was lowered to `llvm.trunc`, which yields a
            // REAL, and the integer conversion wrapped around it then panicked on
            // the float operand.
            "TRUNC" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError("TRUNC expects 1 argument".into()));
                }
                let fv = self.ensure_float(arg_vals[0], arg_tys.get(0).and_then(Option::as_ref))?;
                let result = self.float_to_int(fv, &IecType::Dint, false)?;
                Ok(Some(result.into()))
            }

            // --- Trig functions (extern C library) ---
            "TAN" | "ASIN" | "ACOS" | "ATAN" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument, got {}",
                        arg_vals.len()
                    )));
                }
                let arg = self.ensure_float(arg_vals[0], arg_tys.get(0).and_then(Option::as_ref))?;
                let fty = arg.get_type();
                let is_f64 = fty == self.context.f64_type();
                let c_name = match uname.as_str() {
                    "TAN" => {
                        if is_f64 {
                            "tan"
                        } else {
                            "tanf"
                        }
                    }
                    "ASIN" => {
                        if is_f64 {
                            "asin"
                        } else {
                            "asinf"
                        }
                    }
                    "ACOS" => {
                        if is_f64 {
                            "acos"
                        } else {
                            "acosf"
                        }
                    }
                    "ATAN" => {
                        if is_f64 {
                            "atan"
                        } else {
                            "atanf"
                        }
                    }
                    _ => unreachable!(),
                };
                let fn_type = fty.fn_type(&[fty.into()], false);
                let ext_fn = self.module.get_function(c_name).unwrap_or_else(|| {
                    self.module.add_function(
                        c_name,
                        fn_type,
                        Some(inkwell::module::Linkage::External),
                    )
                });
                let result = self
                    .builder
                    .build_call(ext_fn, &[arg.into()], &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from extern trig fn".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }

            "ATAN2" => {
                if arg_vals.len() != 2 {
                    return Err(CodegenError::LlvmError(format!(
                        "ATAN2 expects 2 arguments, got {}",
                        arg_vals.len()
                    )));
                }
                let y = self.ensure_float(arg_vals[0], arg_tys.get(0).and_then(Option::as_ref))?;
                let x = self.ensure_float(arg_vals[1], arg_tys.get(1).and_then(Option::as_ref))?;
                let (y, x) = self.match_float_widths(y, x)?;
                let fty = y.get_type();
                let is_f64 = fty == self.context.f64_type();
                let c_name = if is_f64 { "atan2" } else { "atan2f" };
                let fn_type = fty.fn_type(&[fty.into(), fty.into()], false);
                let ext_fn = self.module.get_function(c_name).unwrap_or_else(|| {
                    self.module.add_function(
                        c_name,
                        fn_type,
                        Some(inkwell::module::Linkage::External),
                    )
                });
                let result = self
                    .builder
                    .build_call(ext_fn, &[y.into(), x.into()], "atan2")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from atan2".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }

            "LOG" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "LOG expects 1 argument, got {}",
                        arg_vals.len()
                    )));
                }
                let arg = self.ensure_float(arg_vals[0], arg_tys.get(0).and_then(Option::as_ref))?;
                let fty = arg.get_type();
                let intr = Intrinsic::find("llvm.log10").ok_or_else(|| {
                    CodegenError::LlvmError("intrinsic llvm.log10 not found".into())
                })?;
                let fn_val = intr
                    .get_declaration(&self.module, &[fty.into()])
                    .ok_or_else(|| {
                        CodegenError::LlvmError("failed to get llvm.log10 declaration".into())
                    })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[arg.into()], "log")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from log10 intrinsic".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }

            // --- Rounding functions (LLVM intrinsics) ---
            "FLOOR" | "CEIL" | "ROUND" => {
                if arg_vals.len() != 1 {
                    return Err(CodegenError::LlvmError(format!(
                        "{uname} expects 1 argument, got {}",
                        arg_vals.len()
                    )));
                }
                let arg = self.ensure_float(arg_vals[0], arg_tys.get(0).and_then(Option::as_ref))?;
                let fty = arg.get_type();
                let intrinsic_name = match uname.as_str() {
                    "FLOOR" => "llvm.floor",
                    "CEIL" => "llvm.ceil",
                    "ROUND" => "llvm.round",
                    _ => unreachable!(),
                };
                let intr = Intrinsic::find(intrinsic_name).ok_or_else(|| {
                    CodegenError::LlvmError(format!("intrinsic {intrinsic_name} not found"))
                })?;
                let fn_val = intr
                    .get_declaration(&self.module, &[fty.into()])
                    .ok_or_else(|| {
                        CodegenError::LlvmError(format!(
                            "failed to get declaration for {intrinsic_name}"
                        ))
                    })?;
                let result = self
                    .builder
                    .build_call(fn_val, &[arg.into()], &uname.to_lowercase())
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                let result = match result {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => {
                        return Err(CodegenError::LlvmError(
                            "expected return value from rounding intrinsic".into(),
                        ));
                    }
                };
                Ok(Some(result))
            }

            "LEN" => {
                if args.len() != 1 {
                    return Err(CodegenError::LlvmError("LEN expects 1 argument".into()));
                }
                // A pointer to the bytes, not the loaded value; any STRING
                // expression (`LEN('abc')`, `LEN(CONCAT(a, b))`) has one. Anything
                // else is a diagnostic — it used to be a silent 0.
                let (str_ptr, _) = self.string_operand(&args[0].value, "LEN", function)?;
                let strlen_fn = self.get_or_create_strlen_fn();
                let result = self
                    .builder
                    .build_call(strlen_fn, &[str_ptr.into()], "len_result")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                match result {
                    inkwell::values::ValueKind::Basic(v) => Ok(Some(v)),
                    _ => Err(CodegenError::LlvmError("LEN: expected return value".into())),
                }
            }

            "FIND" => {
                if args.len() != 2 {
                    return Err(CodegenError::LlvmError("FIND expects 2 arguments".into()));
                }
                // FIND(s1, s2) — 1-based position of s2 in s1, 0 if not found.
                let (s1_ptr, _) = self.string_operand(&args[0].value, "FIND", function)?;
                let (s2_ptr, _) = self.string_operand(&args[1].value, "FIND", function)?;
                let find_fn = self.get_or_create_find_fn();
                let result = self
                    .builder
                    .build_call(find_fn, &[s1_ptr.into(), s2_ptr.into()], "find_result")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .try_as_basic_value();
                match result {
                    inkwell::values::ValueKind::Basic(v) => Ok(Some(v)),
                    _ => Err(CodegenError::LlvmError("FIND: expected return value".into())),
                }
            }

            // IEC 61131-3 Table 30 date/time functions, CONCAT_DATE_TOD, TIME(), ...
            n if Self::is_datetime_function(n) => {
                self.compile_datetime_call(n, &arg_vals, &arg_tys)
            }

            // String functions that return STRING are handled as special cases
            // in compile_statement (Assignment), not here. CONCAT, LEFT, RIGHT, MID
            // need a destination pointer which is only available at the assignment level.
            "CONCAT" | "LEFT" | "RIGHT" | "MID" => {
                // Return None here — handled in compile_string_assignment
                Ok(None)
            }

            _ => Ok(None),
        }
    }

    /// Get or declare the extern `plcc_print(i8*)` function.
    /// This is provided by the firmware runtime (writes to debug UART).
    fn get_or_declare_plcc_print(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_print") {
            return f;
        }
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let void_ty = self.context.void_type();
        let fn_type = void_ty.fn_type(&[ptr_ty.into()], false);
        self.module.add_function(
            "plcc_print",
            fn_type,
            Some(inkwell::module::Linkage::External),
        )
    }

    /// Get or declare the extern `plcc_monotonic_ns() -> i64` function.
    ///
    /// This is the one time source the compiler needs and the language cannot
    /// provide. The platform supplies it, exactly like `plcc_print`:
    ///   * `plcc sim` / the JIT map it onto a host `std::time::Instant` clock;
    ///   * bare-metal integrators must export a `plcc_monotonic_ns` symbol
    ///     (e.g. from a cycle counter or SysTick).
    ///
    /// Signed i64, not u64: TIME and LTIME are already laid out as i64 nanoseconds
    /// by `iec_to_llvm_type`, and every arithmetic and comparison path in codegen
    /// treats those as signed. Returning u64 would mean the very first thing any
    /// timer did with the value was a signedness reinterpretation. i64 nanoseconds
    /// still spans ~292 years of uptime, which is not a real constraint.
    ///
    /// The value is nanoseconds since an arbitrary, fixed epoch. Only differences
    /// are meaningful. It must never go backwards.
    pub const MONOTONIC_NS_SYMBOL: &'static str = "plcc_monotonic_ns";

    fn get_or_declare_monotonic_ns(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function(Self::MONOTONIC_NS_SYMBOL) {
            return f;
        }
        let fn_type = self.context.i64_type().fn_type(&[], false);
        self.module.add_function(
            Self::MONOTONIC_NS_SYMBOL,
            fn_type,
            Some(inkwell::module::Linkage::External),
        )
    }

    /// Compile a PRINT('literal') or PRINT(string_var) call.
    /// Emits: call void @plcc_print(ptr)
    fn compile_print_call(
        &mut self,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        if args.len() != 1 {
            return Err(CodegenError::LlvmError(format!(
                "PRINT expects 1 argument, got {}",
                args.len()
            )));
        }

        let print_fn = self.get_or_declare_plcc_print();
        let arg_expr = &args[0].value;

        // Check if the argument is a string literal
        let str_ptr = match &arg_expr.kind {
            ExpressionKind::StringLiteral(s) | ExpressionKind::WstringLiteral(s) => {
                // Create a global constant string and get a pointer to it
                let bytes: Vec<u8> = decode_iec_string(s, false)
                    .into_iter()
                    .map(|c| c as u8)
                    .collect();
                let i8_ty = self.context.i8_type();
                let arr_ty = i8_ty.array_type((bytes.len() + 1) as u32);
                let mut vals: Vec<inkwell::values::IntValue> = bytes
                    .iter()
                    .map(|&b| i8_ty.const_int(b as u64, false))
                    .collect();
                vals.push(i8_ty.const_zero()); // null terminator
                let const_arr = i8_ty.const_array(&vals);
                let global = self.module.add_global(arr_ty, None, "print_str");
                global.set_initializer(&const_arr);
                global.set_constant(true);
                global.as_pointer_value()
            }
            _ => {
                // Assume it's a STRING variable — get its pointer via lvalue
                self.compile_lvalue_with_fn(arg_expr, function)?
                    .ok_or_else(|| {
                        CodegenError::LlvmError(
                            "PRINT: argument must be a string literal or STRING variable".into(),
                        )
                    })?
            }
        };

        self.builder
            .build_call(print_fn, &[str_ptr.into()], "")
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        Ok(())
    }

    /// Get or create a `plcc_strlen` helper function that counts non-null bytes.
    /// Signature: i16 plcc_strlen(ptr) -- scans bytes until null, returns count as i16.
    fn get_or_create_strlen_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_strlen") {
            return f;
        }

        let i16_ty = self.context.i16_type();
        let i8_ty = self.context.i8_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = i16_ty.fn_type(&[ptr_ty.into()], false);
        let function = self.module.add_function("plcc_strlen", fn_type, Some(inkwell::module::Linkage::Internal));

        let saved_block = self.builder.get_insert_block();

        let entry = self.context.append_basic_block(function, "entry");
        let loop_bb = self.context.append_basic_block(function, "loop");
        let inc_bb = self.context.append_basic_block(function, "inc");
        let done_bb = self.context.append_basic_block(function, "done");

        self.builder.position_at_end(entry);
        let counter = self.builder.build_alloca(i16_ty, "counter").unwrap();
        self.builder
            .build_store(counter, i16_ty.const_zero())
            .unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(loop_bb);
        let str_ptr = function.get_nth_param(0).unwrap().into_pointer_value();
        let idx = self
            .builder
            .build_load(i16_ty, counter, "idx")
            .unwrap()
            .into_int_value();
        let idx_i64 = self
            .builder
            .build_int_s_extend(idx, self.context.i64_type(), "idx64")
            .unwrap();
        let char_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, str_ptr, &[idx_i64], "char_ptr")
                .unwrap()
        };
        let ch = self
            .builder
            .build_load(i8_ty, char_ptr, "ch")
            .unwrap()
            .into_int_value();
        let is_null = self
            .builder
            .build_int_compare(IntPredicate::EQ, ch, i8_ty.const_zero(), "is_null")
            .unwrap();
        self.builder
            .build_conditional_branch(is_null, done_bb, inc_bb)
            .unwrap();

        self.builder.position_at_end(inc_bb);
        let cur = self
            .builder
            .build_load(i16_ty, counter, "cur")
            .unwrap()
            .into_int_value();
        let next = self
            .builder
            .build_int_add(cur, i16_ty.const_int(1, false), "next")
            .unwrap();
        self.builder.build_store(counter, next).unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(done_bb);
        let result = self.builder.build_load(i16_ty, counter, "result").unwrap();
        self.builder.build_return(Some(&result)).unwrap();

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }

        function
    }

    /// Get or create a `plcc_find` helper function.
    /// Signature: i16 plcc_find(ptr haystack, ptr needle) — returns 1-based position, 0 if not found.
    fn get_or_create_find_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_find") {
            return f;
        }

        let i16_ty = self.context.i16_type();
        let i8_ty = self.context.i8_type();
        let i64_ty = self.context.i64_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = i16_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false);
        let function = self.module.add_function("plcc_find", fn_type, Some(inkwell::module::Linkage::Internal));

        let saved_block = self.builder.get_insert_block();

        let entry = self.context.append_basic_block(function, "entry");
        let outer_loop = self.context.append_basic_block(function, "outer_loop");
        let inner_loop = self.context.append_basic_block(function, "inner_loop");
        let inner_check = self.context.append_basic_block(function, "inner_check");
        let found_bb = self.context.append_basic_block(function, "found");
        let next_bb = self.context.append_basic_block(function, "next");
        let not_found_bb = self.context.append_basic_block(function, "not_found");

        let haystack = function.get_nth_param(0).unwrap().into_pointer_value();
        let needle = function.get_nth_param(1).unwrap().into_pointer_value();

        // entry: alloca i, j
        self.builder.position_at_end(entry);
        let i_ptr = self.builder.build_alloca(i16_ty, "i").unwrap();
        let j_ptr = self.builder.build_alloca(i16_ty, "j").unwrap();
        self.builder
            .build_store(i_ptr, i16_ty.const_zero())
            .unwrap();
        self.builder.build_unconditional_branch(outer_loop).unwrap();

        // outer_loop: check haystack[i] != 0
        self.builder.position_at_end(outer_loop);
        let i_val = self
            .builder
            .build_load(i16_ty, i_ptr, "i")
            .unwrap()
            .into_int_value();
        let i_64 = self
            .builder
            .build_int_s_extend(i_val, i64_ty, "i64")
            .unwrap();
        let h_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, haystack, &[i_64], "hp")
                .unwrap()
        };
        let h_ch = self
            .builder
            .build_load(i8_ty, h_ptr, "hch")
            .unwrap()
            .into_int_value();
        let h_null = self
            .builder
            .build_int_compare(IntPredicate::EQ, h_ch, i8_ty.const_zero(), "hnull")
            .unwrap();
        self.builder
            .build_conditional_branch(h_null, not_found_bb, inner_loop)
            .unwrap();

        // inner_loop: reset j=0, start matching
        self.builder.position_at_end(inner_loop);
        self.builder
            .build_store(j_ptr, i16_ty.const_zero())
            .unwrap();
        self.builder
            .build_unconditional_branch(inner_check)
            .unwrap();

        // inner_check: compare haystack[i+j] == needle[j]
        self.builder.position_at_end(inner_check);
        let i2 = self
            .builder
            .build_load(i16_ty, i_ptr, "i2")
            .unwrap()
            .into_int_value();
        let j2 = self
            .builder
            .build_load(i16_ty, j_ptr, "j2")
            .unwrap()
            .into_int_value();
        // Check needle[j] == 0 => found
        let j2_64 = self.builder.build_int_s_extend(j2, i64_ty, "j64").unwrap();
        let n_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, needle, &[j2_64], "np")
                .unwrap()
        };
        let n_ch = self
            .builder
            .build_load(i8_ty, n_ptr, "nch")
            .unwrap()
            .into_int_value();
        let n_null = self
            .builder
            .build_int_compare(IntPredicate::EQ, n_ch, i8_ty.const_zero(), "nnull")
            .unwrap();
        self.builder
            .build_conditional_branch(n_null, found_bb, next_bb)
            .unwrap();

        // next: compare chars, if mismatch goto outer_loop i++, else j++ and inner_check
        self.builder.position_at_end(next_bb);
        let ij = self.builder.build_int_add(i2, j2, "ij").unwrap();
        let ij_64 = self.builder.build_int_s_extend(ij, i64_ty, "ij64").unwrap();
        let hp2 = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, haystack, &[ij_64], "hp2")
                .unwrap()
        };
        let hc2 = self
            .builder
            .build_load(i8_ty, hp2, "hc2")
            .unwrap()
            .into_int_value();
        let match_cmp = self
            .builder
            .build_int_compare(IntPredicate::EQ, hc2, n_ch, "mcmp")
            .unwrap();

        let j_inc_bb = self.context.append_basic_block(function, "j_inc");
        let i_inc_bb = self.context.append_basic_block(function, "i_inc");
        self.builder
            .build_conditional_branch(match_cmp, j_inc_bb, i_inc_bb)
            .unwrap();

        // j_inc
        self.builder.position_at_end(j_inc_bb);
        let j3 = self
            .builder
            .build_load(i16_ty, j_ptr, "j3")
            .unwrap()
            .into_int_value();
        let j_next = self
            .builder
            .build_int_add(j3, i16_ty.const_int(1, false), "jnext")
            .unwrap();
        self.builder.build_store(j_ptr, j_next).unwrap();
        self.builder
            .build_unconditional_branch(inner_check)
            .unwrap();

        // i_inc
        self.builder.position_at_end(i_inc_bb);
        let i3 = self
            .builder
            .build_load(i16_ty, i_ptr, "i3")
            .unwrap()
            .into_int_value();
        let i_next = self
            .builder
            .build_int_add(i3, i16_ty.const_int(1, false), "inext")
            .unwrap();
        self.builder.build_store(i_ptr, i_next).unwrap();
        self.builder.build_unconditional_branch(outer_loop).unwrap();

        // found: return i + 1 (1-based)
        self.builder.position_at_end(found_bb);
        let i_final = self
            .builder
            .build_load(i16_ty, i_ptr, "ifinal")
            .unwrap()
            .into_int_value();
        let pos = self
            .builder
            .build_int_add(i_final, i16_ty.const_int(1, false), "pos")
            .unwrap();
        self.builder.build_return(Some(&pos)).unwrap();

        // not_found: return 0
        self.builder.position_at_end(not_found_bb);
        self.builder
            .build_return(Some(&i16_ty.const_zero()))
            .unwrap();

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }

        function
    }

    /// Get or create `plcc_left(dest: *i8, src: *i8, n: i32, max_len: i32)`.
    /// Copies first min(n, strlen(src)) characters from src to dest.
    fn get_or_create_left_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_left") {
            return f;
        }

        let void_ty = self.context.void_type();
        let i8_ty = self.context.i8_type();
        let i32_ty = self.context.i32_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = void_ty.fn_type(
            &[ptr_ty.into(), ptr_ty.into(), i32_ty.into(), i32_ty.into()],
            false,
        );
        let function = self.module.add_function("plcc_left", fn_type, Some(inkwell::module::Linkage::Internal));

        let saved_block = self.builder.get_insert_block();

        let entry = self.context.append_basic_block(function, "entry");
        let loop_bb = self.context.append_basic_block(function, "loop_bb");
        let body_bb = self.context.append_basic_block(function, "body_bb");
        let done = self.context.append_basic_block(function, "done");

        let dest = function.get_nth_param(0).unwrap().into_pointer_value();
        let src = function.get_nth_param(1).unwrap().into_pointer_value();
        let n = function.get_nth_param(2).unwrap().into_int_value();
        let max_len = function.get_nth_param(3).unwrap().into_int_value();

        self.builder.position_at_end(entry);
        let idx = self.builder.build_alloca(i32_ty, "idx").unwrap();
        self.builder.build_store(idx, i32_ty.const_zero()).unwrap();
        let max_minus1 = self
            .builder
            .build_int_sub(max_len, i32_ty.const_int(1, false), "max_m1")
            .unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // loop: i < n && i < max_len-1 && src[i] != 0
        self.builder.position_at_end(loop_bb);
        let i = self
            .builder
            .build_load(i32_ty, idx, "i")
            .unwrap()
            .into_int_value();
        let lt_n = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, n, "lt_n")
            .unwrap();
        let lt_max = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, max_minus1, "lt_max")
            .unwrap();
        let i_i64 = self
            .builder
            .build_int_s_extend(i, self.context.i64_type(), "i64")
            .unwrap();
        let src_ch_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, src, &[i_i64], "src_ch")
                .unwrap()
        };
        let ch = self
            .builder
            .build_load(i8_ty, src_ch_ptr, "ch")
            .unwrap()
            .into_int_value();
        let not_null = self
            .builder
            .build_int_compare(IntPredicate::NE, ch, i8_ty.const_zero(), "not_null")
            .unwrap();
        let c1 = self.builder.build_and(lt_n, lt_max, "c1").unwrap();
        let cont = self.builder.build_and(c1, not_null, "cont").unwrap();
        self.builder
            .build_conditional_branch(cont, body_bb, done)
            .unwrap();

        // body: dest[i] = src[i]; i++
        self.builder.position_at_end(body_bb);
        let dest_ch_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[i_i64], "dest_ch")
                .unwrap()
        };
        self.builder.build_store(dest_ch_ptr, ch).unwrap();
        let i_next = self
            .builder
            .build_int_add(i, i32_ty.const_int(1, false), "i_next")
            .unwrap();
        self.builder.build_store(idx, i_next).unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // done: null-terminate
        self.builder.position_at_end(done);
        let final_i = self
            .builder
            .build_load(i32_ty, idx, "final_i")
            .unwrap()
            .into_int_value();
        let fi_i64 = self
            .builder
            .build_int_s_extend(final_i, self.context.i64_type(), "fi64")
            .unwrap();
        let null_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[fi_i64], "null_ptr")
                .unwrap()
        };
        self.builder
            .build_store(null_ptr, i8_ty.const_zero())
            .unwrap();
        self.builder.build_return(None).unwrap();

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }
        function
    }

    /// Get or create `plcc_right(dest: *i8, src: *i8, n: i32, max_len: i32)`.
    /// Copies last n characters of src to dest.
    fn get_or_create_right_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_right") {
            return f;
        }

        let void_ty = self.context.void_type();
        let i8_ty = self.context.i8_type();
        let i32_ty = self.context.i32_type();
        let i16_ty = self.context.i16_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = void_ty.fn_type(
            &[ptr_ty.into(), ptr_ty.into(), i32_ty.into(), i32_ty.into()],
            false,
        );
        let function = self.module.add_function("plcc_right", fn_type, Some(inkwell::module::Linkage::Internal));

        let saved_block = self.builder.get_insert_block();

        let entry = self.context.append_basic_block(function, "entry");
        let loop_bb = self.context.append_basic_block(function, "loop_bb");
        let body_bb = self.context.append_basic_block(function, "body_bb");
        let done = self.context.append_basic_block(function, "done");

        let dest = function.get_nth_param(0).unwrap().into_pointer_value();
        let src = function.get_nth_param(1).unwrap().into_pointer_value();
        let n = function.get_nth_param(2).unwrap().into_int_value();
        let max_len = function.get_nth_param(3).unwrap().into_int_value();

        self.builder.position_at_end(entry);
        // Get strlen(src) via call to plcc_strlen
        let strlen_fn = self.get_or_create_strlen_fn();
        let slen_result = self
            .builder
            .build_call(strlen_fn, &[src.into()], "slen")
            .unwrap()
            .try_as_basic_value();
        let slen_i16 = match slen_result {
            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
            _ => i16_ty.const_zero(),
        };
        let slen = self
            .builder
            .build_int_s_extend(slen_i16, i32_ty, "slen32")
            .unwrap();
        // start = max(0, slen - n)
        let diff = self.builder.build_int_sub(slen, n, "diff").unwrap();
        let is_neg = self
            .builder
            .build_int_compare(IntPredicate::SLT, diff, i32_ty.const_zero(), "is_neg")
            .unwrap();
        let start = self
            .builder
            .build_select(is_neg, i32_ty.const_zero(), diff, "start")
            .unwrap()
            .into_int_value();

        let dest_idx = self.builder.build_alloca(i32_ty, "dest_idx").unwrap();
        let src_idx = self.builder.build_alloca(i32_ty, "src_idx").unwrap();
        self.builder
            .build_store(dest_idx, i32_ty.const_zero())
            .unwrap();
        self.builder.build_store(src_idx, start).unwrap();
        let max_minus1 = self
            .builder
            .build_int_sub(max_len, i32_ty.const_int(1, false), "max_m1")
            .unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // loop: src_idx < slen && dest_idx < max_len-1
        self.builder.position_at_end(loop_bb);
        let si = self
            .builder
            .build_load(i32_ty, src_idx, "si")
            .unwrap()
            .into_int_value();
        let di = self
            .builder
            .build_load(i32_ty, dest_idx, "di")
            .unwrap()
            .into_int_value();
        let lt_slen = self
            .builder
            .build_int_compare(IntPredicate::SLT, si, slen, "lt_slen")
            .unwrap();
        let lt_max = self
            .builder
            .build_int_compare(IntPredicate::SLT, di, max_minus1, "lt_max")
            .unwrap();
        let cont = self.builder.build_and(lt_slen, lt_max, "cont").unwrap();
        self.builder
            .build_conditional_branch(cont, body_bb, done)
            .unwrap();

        // body: dest[dest_idx] = src[src_idx]; both++
        self.builder.position_at_end(body_bb);
        let si_i64 = self
            .builder
            .build_int_s_extend(si, self.context.i64_type(), "si64")
            .unwrap();
        let di_i64 = self
            .builder
            .build_int_s_extend(di, self.context.i64_type(), "di64")
            .unwrap();
        let src_ch = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, src, &[si_i64], "src_ch")
                .unwrap()
        };
        let ch = self.builder.build_load(i8_ty, src_ch, "ch").unwrap();
        let dest_ch = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[di_i64], "dest_ch")
                .unwrap()
        };
        self.builder.build_store(dest_ch, ch).unwrap();
        let si_next = self
            .builder
            .build_int_add(si, i32_ty.const_int(1, false), "si_next")
            .unwrap();
        let di_next = self
            .builder
            .build_int_add(di, i32_ty.const_int(1, false), "di_next")
            .unwrap();
        self.builder.build_store(src_idx, si_next).unwrap();
        self.builder.build_store(dest_idx, di_next).unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // done: null-terminate
        self.builder.position_at_end(done);
        let final_di = self
            .builder
            .build_load(i32_ty, dest_idx, "final_di")
            .unwrap()
            .into_int_value();
        let fdi_i64 = self
            .builder
            .build_int_s_extend(final_di, self.context.i64_type(), "fdi64")
            .unwrap();
        let null_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[fdi_i64], "null_ptr")
                .unwrap()
        };
        self.builder
            .build_store(null_ptr, i8_ty.const_zero())
            .unwrap();
        self.builder.build_return(None).unwrap();

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }
        function
    }

    /// Get or create `plcc_mid(dest: *i8, src: *i8, len: i32, pos: i32, max_len: i32)`.
    /// Copies `len` characters starting at 1-based position `pos` from src to dest.
    fn get_or_create_mid_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("plcc_mid") {
            return f;
        }

        let void_ty = self.context.void_type();
        let i8_ty = self.context.i8_type();
        let i32_ty = self.context.i32_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = void_ty.fn_type(
            &[
                ptr_ty.into(),
                ptr_ty.into(),
                i32_ty.into(),
                i32_ty.into(),
                i32_ty.into(),
            ],
            false,
        );
        let function = self.module.add_function("plcc_mid", fn_type, Some(inkwell::module::Linkage::Internal));

        let saved_block = self.builder.get_insert_block();

        let entry = self.context.append_basic_block(function, "entry");
        let loop_bb = self.context.append_basic_block(function, "loop_bb");
        let body_bb = self.context.append_basic_block(function, "body_bb");
        let done = self.context.append_basic_block(function, "done");

        let dest = function.get_nth_param(0).unwrap().into_pointer_value();
        let src = function.get_nth_param(1).unwrap().into_pointer_value();
        let len_param = function.get_nth_param(2).unwrap().into_int_value();
        let pos = function.get_nth_param(3).unwrap().into_int_value();
        let max_len = function.get_nth_param(4).unwrap().into_int_value();

        self.builder.position_at_end(entry);
        // offset = pos - 1 (convert 1-based to 0-based)
        let offset = self
            .builder
            .build_int_sub(pos, i32_ty.const_int(1, false), "offset")
            .unwrap();
        let dest_idx = self.builder.build_alloca(i32_ty, "dest_idx").unwrap();
        let src_idx = self.builder.build_alloca(i32_ty, "src_idx").unwrap();
        let count = self.builder.build_alloca(i32_ty, "count").unwrap();
        self.builder
            .build_store(dest_idx, i32_ty.const_zero())
            .unwrap();
        self.builder.build_store(src_idx, offset).unwrap();
        self.builder
            .build_store(count, i32_ty.const_zero())
            .unwrap();
        let max_minus1 = self
            .builder
            .build_int_sub(max_len, i32_ty.const_int(1, false), "max_m1")
            .unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // loop: count < len && dest_idx < max_len-1 && src[src_idx] != 0
        self.builder.position_at_end(loop_bb);
        let c = self
            .builder
            .build_load(i32_ty, count, "c")
            .unwrap()
            .into_int_value();
        let di = self
            .builder
            .build_load(i32_ty, dest_idx, "di")
            .unwrap()
            .into_int_value();
        let si = self
            .builder
            .build_load(i32_ty, src_idx, "si")
            .unwrap()
            .into_int_value();
        let lt_len = self
            .builder
            .build_int_compare(IntPredicate::SLT, c, len_param, "lt_len")
            .unwrap();
        let lt_max = self
            .builder
            .build_int_compare(IntPredicate::SLT, di, max_minus1, "lt_max")
            .unwrap();
        let si_i64 = self
            .builder
            .build_int_s_extend(si, self.context.i64_type(), "si64")
            .unwrap();
        let src_ch_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, src, &[si_i64], "src_ch")
                .unwrap()
        };
        let ch = self
            .builder
            .build_load(i8_ty, src_ch_ptr, "ch")
            .unwrap()
            .into_int_value();
        let not_null = self
            .builder
            .build_int_compare(IntPredicate::NE, ch, i8_ty.const_zero(), "not_null")
            .unwrap();
        let c1 = self.builder.build_and(lt_len, lt_max, "c1").unwrap();
        let cont = self.builder.build_and(c1, not_null, "cont").unwrap();
        self.builder
            .build_conditional_branch(cont, body_bb, done)
            .unwrap();

        // body: dest[dest_idx] = ch; all++
        self.builder.position_at_end(body_bb);
        let di_i64 = self
            .builder
            .build_int_s_extend(di, self.context.i64_type(), "di64")
            .unwrap();
        let dest_ch_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[di_i64], "dest_ch")
                .unwrap()
        };
        self.builder.build_store(dest_ch_ptr, ch).unwrap();
        let di_next = self
            .builder
            .build_int_add(di, i32_ty.const_int(1, false), "di_next")
            .unwrap();
        let si_next = self
            .builder
            .build_int_add(si, i32_ty.const_int(1, false), "si_next")
            .unwrap();
        let c_next = self
            .builder
            .build_int_add(c, i32_ty.const_int(1, false), "c_next")
            .unwrap();
        self.builder.build_store(dest_idx, di_next).unwrap();
        self.builder.build_store(src_idx, si_next).unwrap();
        self.builder.build_store(count, c_next).unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // done: null-terminate
        self.builder.position_at_end(done);
        let final_di = self
            .builder
            .build_load(i32_ty, dest_idx, "final_di")
            .unwrap()
            .into_int_value();
        let fdi_i64 = self
            .builder
            .build_int_s_extend(final_di, self.context.i64_type(), "fdi64")
            .unwrap();
        let null_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, dest, &[fdi_i64], "null_ptr")
                .unwrap()
        };
        self.builder
            .build_store(null_ptr, i8_ty.const_zero())
            .unwrap();
        self.builder.build_return(None).unwrap();

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }
        function
    }

    /// Bring an integer to `target`'s width: extend by `signed`, else truncate.
    fn resize_int(
        &self,
        iv: inkwell::values::IntValue<'ctx>,
        target: inkwell::types::IntType<'ctx>,
        signed: bool,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let (sw, tw) = (iv.get_type().get_bit_width(), target.get_bit_width());
        let r = if sw == tw {
            return Ok(iv);
        } else if sw > tw {
            self.builder.build_int_truncate(iv, target, "rsz")
        } else if signed {
            self.builder.build_int_s_extend(iv, target, "rsz")
        } else {
            self.builder.build_int_z_extend(iv, target, "rsz")
        };
        r.map_err(|e| CodegenError::LlvmError(e.to_string()))
    }

    fn widens_unsigned_opt(ty: Option<&IecType>) -> bool {
        ty.is_some_and(Self::widens_unsigned)
    }

    /// Emit, into the current block, a loop copying `src[start..end)` onto
    /// `buf[*k..]`, stopping early at a NUL in `src` or when `*k` reaches `limit`.
    /// Leaves the builder at the loop's exit.
    #[allow(clippy::too_many_arguments)]
    fn emit_bounded_copy(
        &self,
        function: FunctionValue<'ctx>,
        buf: PointerValue<'ctx>,
        k_ptr: PointerValue<'ctx>,
        i_ptr: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        start: inkwell::values::IntValue<'ctx>,
        end: inkwell::values::IntValue<'ctx>,
        limit: inkwell::values::IntValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let i8_ty = self.context.i8_type();
        let i32_ty = self.context.i32_type();
        let i64_ty = self.context.i64_type();
        let head = self.context.append_basic_block(function, "copy_head");
        let body = self.context.append_basic_block(function, "copy_body");
        let exit = self.context.append_basic_block(function, "copy_exit");

        self.builder.build_store(i_ptr, start).map_err(err)?;
        self.builder.build_unconditional_branch(head).map_err(err)?;

        self.builder.position_at_end(head);
        let i = self
            .builder
            .build_load(i32_ty, i_ptr, "i")
            .map_err(err)?
            .into_int_value();
        let k = self
            .builder
            .build_load(i32_ty, k_ptr, "k")
            .map_err(err)?
            .into_int_value();
        let in_range = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, end, "in_range")
            .map_err(err)?;
        let has_room = self
            .builder
            .build_int_compare(IntPredicate::SLT, k, limit, "has_room")
            .map_err(err)?;
        // `i` never exceeds the source's length here — `end` is clamped to it, or
        // the NUL stops the loop — so this load stays inside the string.
        let i64v = self
            .builder
            .build_int_s_extend(i, i64_ty, "i64")
            .map_err(err)?;
        let src_ch = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, src, &[i64v], "src_ch")
        }
        .map_err(err)?;
        let ch = self
            .builder
            .build_load(i8_ty, src_ch, "ch")
            .map_err(err)?
            .into_int_value();
        let not_nul = self
            .builder
            .build_int_compare(IntPredicate::NE, ch, i8_ty.const_zero(), "not_nul")
            .map_err(err)?;
        let go = self
            .builder
            .build_and(in_range, has_room, "go")
            .map_err(err)?;
        let go = self.builder.build_and(go, not_nul, "go2").map_err(err)?;
        self.builder
            .build_conditional_branch(go, body, exit)
            .map_err(err)?;

        self.builder.position_at_end(body);
        let k64 = self
            .builder
            .build_int_s_extend(k, i64_ty, "k64")
            .map_err(err)?;
        let dst_ch = unsafe {
            self.builder
                .build_in_bounds_gep(i8_ty, buf, &[k64], "dst_ch")
        }
        .map_err(err)?;
        self.builder.build_store(dst_ch, ch).map_err(err)?;
        let one = i32_ty.const_int(1, false);
        let k1 = self.builder.build_int_add(k, one, "k1").map_err(err)?;
        let i1 = self.builder.build_int_add(i, one, "i1").map_err(err)?;
        self.builder.build_store(k_ptr, k1).map_err(err)?;
        self.builder.build_store(i_ptr, i1).map_err(err)?;
        self.builder.build_unconditional_branch(head).map_err(err)?;

        self.builder.position_at_end(exit);
        Ok(())
    }

    /// Get or create `plcc_replace(dest, in1, in2, l: i32, p: i32, max_len: i32)`:
    /// IEC `REPLACE(IN1, IN2, L, P)` — IN1 with the L characters starting at 1-based
    /// position P replaced by IN2.
    ///
    /// P is clamped into `1..=LEN(IN1)+1` and L to what IN1 has left, so a position
    /// past the end appends and nothing ever reads beyond IN1's terminator. The
    /// result is built in a scratch buffer and copied to `dest` last, because the
    /// idiomatic call writes over its own input: `s := REPLACE(s, '', 1, pos)`.
    fn get_or_create_replace_fn(&self) -> Result<FunctionValue<'ctx>, CodegenError> {
        if let Some(f) = self.module.get_function("plcc_replace") {
            return Ok(f);
        }
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let i8_ty = self.context.i8_type();
        let i32_ty = self.context.i32_type();
        let i64_ty = self.context.i64_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = self.context.void_type().fn_type(
            &[
                ptr_ty.into(),
                ptr_ty.into(),
                ptr_ty.into(),
                i32_ty.into(),
                i32_ty.into(),
                i32_ty.into(),
            ],
            false,
        );
        let function = self.module.add_function("plcc_replace", fn_type, Some(inkwell::module::Linkage::Internal));
        let saved_block = self.builder.get_insert_block();

        let param = |n: u32| {
            function
                .get_nth_param(n)
                .ok_or_else(|| CodegenError::LlvmError("plcc_replace: missing param".into()))
        };
        let dest = param(0)?.into_pointer_value();
        let in1 = param(1)?.into_pointer_value();
        let in2 = param(2)?.into_pointer_value();
        let l = param(3)?.into_int_value();
        let p = param(4)?.into_int_value();
        let max_len = param(5)?.into_int_value();

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);
        let buf = self
            .builder
            .build_array_alloca(i8_ty, max_len, "buf")
            .map_err(err)?;
        let k_ptr = self.builder.build_alloca(i32_ty, "k").map_err(err)?;
        let i_ptr = self.builder.build_alloca(i32_ty, "i").map_err(err)?;
        self.builder
            .build_store(k_ptr, i32_ty.const_zero())
            .map_err(err)?;

        let strlen_fn = self.get_or_create_strlen_fn();
        let n1 = match self
            .builder
            .build_call(strlen_fn, &[in1.into()], "n1")
            .map_err(err)?
            .try_as_basic_value()
        {
            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
            _ => {
                return Err(CodegenError::LlvmError(
                    "plcc_strlen returned no value".into(),
                ));
            }
        };
        let n1 = self
            .builder
            .build_int_z_extend(n1, i32_ty, "n1_32")
            .map_err(err)?;

        let smax = |a, b, name: &str| -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
            let c = self
                .builder
                .build_int_compare(IntPredicate::SGT, a, b, name)
                .map_err(err)?;
            Ok(self
                .builder
                .build_select(c, a, b, name)
                .map_err(err)?
                .into_int_value())
        };
        let smin = |a, b, name: &str| -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
            let c = self
                .builder
                .build_int_compare(IntPredicate::SLT, a, b, name)
                .map_err(err)?;
            Ok(self
                .builder
                .build_select(c, a, b, name)
                .map_err(err)?
                .into_int_value())
        };
        let zero = i32_ty.const_zero();
        let one = i32_ty.const_int(1, false);
        let p0 = self.builder.build_int_sub(p, one, "p0").map_err(err)?;
        let p0 = smax(p0, zero, "p0_lo")?;
        let p0 = smin(p0, n1, "p0_hi")?;
        let lc = smax(l, zero, "l_lo")?;
        let tail = self.builder.build_int_add(p0, lc, "tail").map_err(err)?;
        let tail = smin(tail, n1, "tail_hi")?;
        let limit = self
            .builder
            .build_int_sub(max_len, one, "limit")
            .map_err(err)?;
        let no_end = i32_ty.const_int(i32::MAX as u64, false);

        self.emit_bounded_copy(function, buf, k_ptr, i_ptr, in1, zero, p0, limit)?;
        self.emit_bounded_copy(function, buf, k_ptr, i_ptr, in2, zero, no_end, limit)?;
        self.emit_bounded_copy(function, buf, k_ptr, i_ptr, in1, tail, n1, limit)?;

        let k = self
            .builder
            .build_load(i32_ty, k_ptr, "k_end")
            .map_err(err)?
            .into_int_value();
        let k64 = self
            .builder
            .build_int_s_extend(k, i64_ty, "k64")
            .map_err(err)?;
        let nul =
            unsafe { self.builder.build_in_bounds_gep(i8_ty, buf, &[k64], "nul") }.map_err(err)?;
        self.builder
            .build_store(nul, i8_ty.const_zero())
            .map_err(err)?;
        let size = self
            .builder
            .build_int_add(k64, i64_ty.const_int(1, false), "size")
            .map_err(err)?;
        self.builder
            .build_memmove(dest, 1, buf, 1, size)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        self.builder.build_return(None).map_err(err)?;

        if let Some(bb) = saved_block {
            self.builder.position_at_end(bb);
        }
        Ok(function)
    }

    /// The one REAL/LREAL → integer conversion. Every `REAL_TO_<int>`,
    /// `LREAL_TO_<int>`, `TRUNC`, `REAL_TO_TIME` and implicit REAL → integer store
    /// lowers here. Out-of-range values saturate and NaN is 0 (see below).
    ///
    /// `round`: round to nearest, halves away from zero (`llvm.round`), which is what
    /// CODESYS documents for REAL_TO_<type> — "for 1 to 4 after the decimal point,
    /// the number is rounded down; for 5 to 9 it is rounded up", with
    /// `REAL_TO_INT(-1.5) = -2`. IEC 61131-3 (Table 22) also requires round-to-nearest
    /// but, by reference to IEC 60559, breaks ties to even (2.5 → 2); CODESYS and
    /// TwinCAT give 3. plcc follows CODESYS here — see docs/codesys-compatibility.md.
    /// Switching is `llvm.roundeven`. Without `round` the value is truncated toward
    /// zero, which is TRUNC.
    fn float_to_int(
        &self,
        fv: inkwell::values::FloatValue<'ctx>,
        target: &IecType,
        round: bool,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let llvm_err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let fv = if round {
            let intr = Intrinsic::find("llvm.round")
                .ok_or_else(|| CodegenError::LlvmError("intrinsic llvm.round not found".into()))?;
            let decl = intr
                .get_declaration(&self.module, &[fv.get_type().into()])
                .ok_or_else(|| CodegenError::LlvmError("llvm.round declaration".into()))?;
            match self
                .builder
                .build_call(decl, &[fv.into()], "round")
                .map_err(llvm_err)?
                .try_as_basic_value()
            {
                inkwell::values::ValueKind::Basic(v) => v.into_float_value(),
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err(CodegenError::LlvmError("llvm.round returned no value".into()));
                }
            }
        } else {
            fv
        };
        // Saturating: an out-of-range value clamps to the target's range and NaN
        // becomes 0 (`llvm.fpto[su]i.sat`). A plain fptosi/fptoui is poison out of
        // range — undefined behaviour once the optimizer sees it — and CODESYS only
        // promises "an undefined, target system-dependent value", so any defined
        // answer conforms; saturation is the one that is the same on every target.
        let it = self.iec_to_llvm_type(target).into_int_type();
        let name = if Self::widens_unsigned(target) {
            "llvm.fptoui.sat"
        } else {
            "llvm.fptosi.sat"
        };
        let intr = Intrinsic::find(name)
            .ok_or_else(|| CodegenError::LlvmError(format!("intrinsic {name} not found")))?;
        let decl = intr
            .get_declaration(&self.module, &[it.into(), fv.get_type().into()])
            .ok_or_else(|| CodegenError::LlvmError(format!("{name} declaration")))?;
        match self
            .builder
            .build_call(decl, &[fv.into()], "f_to_int_sat")
            .map_err(llvm_err)?
            .try_as_basic_value()
        {
            inkwell::values::ValueKind::Basic(v) => Ok(v.into_int_value()),
            inkwell::values::ValueKind::Instruction(_) => Err(CodegenError::LlvmError(format!(
                "{name} returned no value"
            ))),
        }
    }

    fn ensure_float(
        &self,
        val: BasicValueEnum<'ctx>,
        ty: Option<&IecType>,
    ) -> Result<inkwell::values::FloatValue<'ctx>, CodegenError> {
        if val.is_float_value() {
            Ok(val.into_float_value())
        } else {
            let iv = self.int_operand(val, "a floating-point operand")?;
            self.int_to_float(iv, ty, self.context.f32_type())
        }
    }

    /// The integer type an address is converted to when a POINTER is used as a
    /// number (`pt + 1`, `DWORD_TO_X(pt)`, comparing two pointers). 64 bits holds an
    /// address on every target plcc emits for; narrower uses truncate from there.
    fn addr_int_type(&self) -> inkwell::types::IntType<'ctx> {
        self.context.i64_type()
    }

    /// An operand that must be an integer, with no `into_int_value` panic behind it.
    ///
    /// * An integer is returned as is.
    /// * A POINTER is its address, as an integer — CODESYS treats a pointer as an
    ///   address word, and OSCAT relies on it (`pt := pt + 1`, `ADR(x) + n`).
    /// * A REAL/LREAL is a type error: IEC has no implicit REAL→integer conversion,
    ///   so it is reported, naming `what` (the operator or builtin), rather than
    ///   silently truncated.
    /// * An aggregate (ARRAY/STRUCT) value is reported the same way.
    fn int_operand(
        &self,
        val: BasicValueEnum<'ctx>,
        what: &str,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        match val {
            BasicValueEnum::IntValue(iv) => Ok(iv),
            BasicValueEnum::PointerValue(pv) => self
                .builder
                .build_ptr_to_int(pv, self.addr_int_type(), "addr")
                .map_err(|e| CodegenError::LlvmError(e.to_string())),
            BasicValueEnum::FloatValue(_) => Err(CodegenError::UnsupportedType(format!(
                "{what} needs an integer operand but got a REAL/LREAL value; IEC 61131-3 \
                 has no implicit REAL-to-integer conversion — use an explicit one such \
                 as REAL_TO_DINT or TRUNC"
            ))),
            BasicValueEnum::ArrayValue(_) => Err(CodegenError::UnsupportedType(format!(
                "{what} needs an integer operand but got an ARRAY value"
            ))),
            BasicValueEnum::StructValue(_) => Err(CodegenError::UnsupportedType(format!(
                "{what} needs an integer operand but got a STRUCT or FB instance value"
            ))),
            _ => Err(CodegenError::UnsupportedType(format!(
                "{what} needs an integer operand but got a vector value"
            ))),
        }
    }

    /// Match two float values to the same (wider) type.
    fn match_float_widths(
        &self,
        a: inkwell::values::FloatValue<'ctx>,
        b: inkwell::values::FloatValue<'ctx>,
    ) -> Result<
        (
            inkwell::values::FloatValue<'ctx>,
            inkwell::values::FloatValue<'ctx>,
        ),
        CodegenError,
    > {
        let aty = a.get_type();
        let bty = b.get_type();
        if aty == bty {
            Ok((a, b))
        } else if aty == self.context.f64_type() {
            let b_ext = self
                .builder
                .build_float_ext(b, self.context.f64_type(), "fext")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            Ok((a, b_ext))
        } else {
            let a_ext = self
                .builder
                .build_float_ext(a, self.context.f64_type(), "fext")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            Ok((a_ext, b))
        }
    }

    /// The REAL type an EXPT operand of IEC type `ty` is evaluated in.
    ///
    /// IEC 61131-3 (3rd ed., Table 23 no. 7 and Table 29) defines `**` as EXPT:
    /// `IN1 : ANY_REAL`, `IN2 : ANY_NUM`, result the type of IN1. An integer base
    /// reaches IN1 through the implicit conversions of Table 11: SINT/INT/USINT/UINT
    /// (and the 8/16-bit ANY_BIT) to REAL, the 32/64-bit integers to LREAL. A bare
    /// integer literal has no type of its own here, so it takes LREAL, which is exact
    /// for every integer result up to 2**53.
    fn expt_float_type(&self, ty: Option<&IecType>) -> inkwell::types::FloatType<'ctx> {
        match ty {
            Some(IecType::Real) => self.context.f32_type(),
            Some(t) if t.bit_size().is_some_and(|b| b <= 16) => self.context.f32_type(),
            _ => self.context.f64_type(),
        }
    }

    /// Integer to floating point, honouring the source's signedness: a BYTE holding
    /// 200 is 200.0, not -56.0.
    fn int_to_float(
        &self,
        iv: inkwell::values::IntValue<'ctx>,
        ty: Option<&IecType>,
        fty: inkwell::types::FloatType<'ctx>,
    ) -> Result<inkwell::values::FloatValue<'ctx>, CodegenError> {
        if iv.get_type().get_bit_width() == 1 || Self::widens_unsigned_opt(ty) {
            self.builder.build_unsigned_int_to_float(iv, fty, "uitof")
        } else {
            self.builder.build_signed_int_to_float(iv, fty, "itof")
        }
        .map_err(|e| CodegenError::LlvmError(e.to_string()))
    }

    /// `base ** exp` and `EXPT(base, exp)`: always a REAL/LREAL result, per the
    /// standard (see [`Self::expt_float_type`]).
    ///
    /// Integer operands are converted, not truncated to an integer power: `2 ** -1`
    /// is 0.5, `0 ** 0` is 1.0, `0 ** -1` is +inf (C `pow` semantics). Assigning the
    /// result to an integer variable converts it back, exactly for every integer
    /// result an LREAL holds exactly.
    fn compile_expt(
        &self,
        base: BasicValueEnum<'ctx>,
        base_ty: Option<&IecType>,
        exp: BasicValueEnum<'ctx>,
        exp_ty: Option<&IecType>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let base = if base.is_float_value() {
            base.into_float_value()
        } else {
            let iv = self.int_operand(base, "the base of EXPT / **")?;
            self.int_to_float(iv, base_ty, self.expt_float_type(base_ty))?
        };
        let exp = if exp.is_float_value() {
            exp.into_float_value()
        } else {
            let iv = self.int_operand(exp, "the exponent of EXPT / **")?;
            self.int_to_float(iv, exp_ty, base.get_type())?
        };
        let (base, exp) = self.match_float_widths(base, exp)?;
        let fty = base.get_type();
        let intr = Intrinsic::find("llvm.pow")
            .ok_or_else(|| CodegenError::LlvmError("intrinsic llvm.pow not found".into()))?;
        let fn_val = intr
            .get_declaration(&self.module, &[fty.into()])
            .ok_or_else(|| CodegenError::LlvmError("failed to get llvm.pow declaration".into()))?;
        match self
            .builder
            .build_call(fn_val, &[base.into(), exp.into()], "expt")
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            .try_as_basic_value()
        {
            inkwell::values::ValueKind::Basic(v) => Ok(v),
            _ => Err(CodegenError::LlvmError(
                "expected return value from llvm.pow".into(),
            )),
        }
    }

    /// Static IEC type of `l ** r`, mirroring what [`Self::compile_expt`] produces.
    fn expt_result_type(&self, l: &Expression, r: &Expression) -> IecType {
        let float_of = |e: &Expression| -> Option<IecType> {
            match self.rvalue_iec_type(e) {
                Some(t @ (IecType::Real | IecType::Lreal)) => Some(t),
                _ if Self::is_real_literal(e) => Some(IecType::Real),
                _ => None,
            }
        };
        let base = float_of(l).unwrap_or_else(|| {
            if self.expt_float_type(self.rvalue_iec_type(l).as_ref()) == self.context.f32_type()
            {
                IecType::Real
            } else {
                IecType::Lreal
            }
        });
        match (base, float_of(r)) {
            (IecType::Lreal, _) | (_, Some(IecType::Lreal)) => IecType::Lreal,
            _ => IecType::Real,
        }
    }

    fn is_real_literal(e: &Expression) -> bool {
        match &e.kind {
            ExpressionKind::RealLiteral(_) => true,
            ExpressionKind::Parenthesized(inner) => Self::is_real_literal(inner),
            ExpressionKind::UnaryOp { operand, .. } => Self::is_real_literal(operand),
            _ => false,
        }
    }

    pub fn compile(&mut self, unit: &CompilationUnit) -> Result<(), CodegenError> {
        // PROPERTY / ACTION / VAR_STAT / qualified names become methods, globals
        // and plain names (the type checker runs the same pass).
        let (desugared, sugar_errors) = plcc_hir::desugar::desugar(unit);
        if let Some((_, span, message)) = sugar_errors.into_iter().next() {
            return Err(CodegenError::Located { message, span });
        }
        let unit = desugared.as_ref().unwrap_or(unit);
        // `ARRAY[1..N]`, `STRING(LEN)` with named constants: fold to literals. A
        // bound that is not a constant used to become a one-element array.
        let (folded, unresolved) = plcc_hir::consts::fold_type_constants(unit);
        if let Some((_, span, message)) = unresolved.first() {
            return Err(CodegenError::UnsupportedType(format!(
                "{message} (source offset {})",
                span.start
            )));
        }
        let unit = folded.as_ref().unwrap_or(unit);
        // CONFIGURATION / RESOURCE VAR_GLOBAL blocks become ordinary VAR_GLOBALs.
        let hoisted = contract::hoist_configuration_globals(unit);
        let unit = hoisted.as_ref().unwrap_or(unit);
        // EXTENDS: every derived FB/CLASS gets its base's variables and methods.
        // VAR_INST method variables become hidden instance members.
        let lifted = oop::lift_var_inst(unit);
        let unit = lifted.as_ref().unwrap_or(unit);
        let (flattened, hierarchy) = oop::flatten_inheritance(unit)?;
        self.hierarchy = hierarchy;
        let unit = flattened.as_ref().unwrap_or(unit);
        // REFERENCE TO: every use of a reference means the referenced value.
        let dereffed = refs::desugar_references(unit);
        let unit = dereffed.as_ref().unwrap_or(unit);
        self.reference_inputs = refs::reference_inputs(unit);
        self.fb_init_defaults = fbinit::defaults(unit);
        // REAL/LREAL <-> STRING, written in ST, only when the unit uses them.
        let with_helpers = ondemand::add_on_demand_helpers(unit)?;
        let unit = with_helpers.as_ref().map(|(u, _)| u).unwrap_or(unit);
        let helper_fns: Vec<String> = with_helpers
            .as_ref()
            .map(|(_, names)| names.clone())
            .unwrap_or_default();
        self.register_standard_functions();

        // Register types and POUs
        for decl in &unit.declarations {
            let (name, fb_type) = match decl {
                Declaration::FunctionBlock(fb) => (
                    fb.name.name.clone(),
                    IecType::FbInstance(fb.name.name.clone()),
                ),
                Declaration::Class(cls) => (
                    cls.name.name.clone(),
                    IecType::FbInstance(cls.name.name.clone()),
                ),
                // An INTERFACE-typed variable is valid 3rd-edition ST (IEC 61131-3
                // §6.6.4). It holds a *reference* to an object implementing the
                // interface, not an inline instance, so it gets a pointer slot —
                // registering it as an FbInstance would find no layout and silently
                // fall back to an i32 field, which is exactly the miscompile the
                // unknown-type error exists to prevent. Skipping it entirely made the
                // declaration a hard error.
                Declaration::Interface(iface) => (
                    iface.name.name.clone(),
                    Self::interface_type(&iface.name.name),
                ),
                _ => continue,
            };
            // TypeRegistry::register case-folds the key, so one registration covers
            // every spelling a program might use.
            self.type_registry.register(name.clone(), fb_type.clone());
            self.type_checker.types.register(name, fb_type);
        }

        // Register user TYPE declarations (STRUCT / ENUM / subrange / alias) so that a
        // `v : MyStruct;` declaration resolves instead of falling through to the
        // "unknown type" fallback.
        //
        // A TYPE may name another TYPE declared later in the file, so this iterates to
        // a fixed point. A fixed *count* of passes silently truncates: two passes
        // resolve a two-deep forward chain and reject a three-deep one, which is not a
        // property of the language, only of the loop bound. Each round re-resolves from
        // the AST, so a round that resolves one name lets the next round resolve
        // anything that referenced it.
        self.register_type_declarations(unit)?;

        // Every declared type must resolve to something real *before* we lay out any
        // struct. Previously an unknown type name (`t : TON;` with no TON in scope)
        // silently became an i32 field and every statement referring to it was dropped
        // from the generated code — a silent miscompile with no diagnostic at all.
        self.validate_declared_types(unit)?;

        // Lay out every FB/CLASS state struct *before* anything that needs to know
        // their size. This makes FB instance fields work regardless of declaration
        // order (an FB may contain an instance of an FB declared later in the file).
        self.layout_pou_structs(unit);

        // Record every METHOD signature up front for the same reason: a method call
        // resolves through the owner's recorded `methods` map.
        self.layout_pou_methods(unit);
        self.layout_callable_programs(unit);
        self.layout_interfaces(unit);

        // And every FUNCTION signature, for the same reason one level over: a call site
        // has to coerce its arguments to the declared parameter types, and it may be
        // compiled before the callee's own body is.
        self.layout_function_signatures(unit);

        // Declare the `<pou>_init` prototypes up front so init bodies can call each
        // other without regard to declaration order.
        self.declare_init_prototypes(unit);

        // Validate every AT address and record the process-image bindings before any
        // body refers to them.
        self.plan_process_image(unit)?;

        // Scan for VAR_GLOBAL declarations and create a global struct
        let mut global_fields = Vec::new();
        let mut global_names = Vec::new();
        for decl in &unit.declarations {
            if let Declaration::GlobalVarDecl(block) = decl {
                for var in &block.declarations {
                    let ty = self.resolve_type_spec(&var.type_spec);
                    global_fields.push(self.iec_to_llvm_type(&ty));
                    global_names.push((var.name.name.clone(), ty, var.initializer.clone()));
                }
            }
        }
        if !global_fields.is_empty() {
            let global_struct = self.context.struct_type(&global_fields, false);
            let global_val = self.module.add_global(global_struct, None, "plcc_globals");
            // Build initializer with constant values
            let mut const_vals: Vec<inkwell::values::BasicValueEnum<'ctx>> = Vec::new();
            for (i, (_name, ty, init)) in global_names.iter().enumerate() {
                if let Some(init_expr) = init {
                    // Try to evaluate constant initializer
                    if let Some(val) = self.eval_const_initializer(init_expr, ty) {
                        // At the field's width: `g : DINT := BYTE#5` (or a plain
                        // literal narrower than its slot) must not mistype the
                        // aggregate. Aggregates are already built at their type.
                        let val = self.const_coerce(val, ty).unwrap_or(val);
                        const_vals.push(val);
                    } else {
                        const_vals.push(global_fields[i].const_zero());
                    }
                } else {
                    const_vals.push(global_fields[i].const_zero());
                }
            }
            let init = global_struct.const_named_struct(&const_vals);
            global_val.set_initializer(&init);

            let names_types: Vec<(String, IecType)> = global_names
                .into_iter()
                .map(|(name, ty, _)| (name, ty))
                .collect();
            self.global_var = Some((global_val, global_struct, names_types));
        }

        // Compile FBs, classes, and functions first so they're available when programs reference them
        for decl in &unit.declarations {
            match decl {
                Declaration::Function(f) => self.compile_function(f)?,
                Declaration::FunctionBlock(fb) => self.compile_function_block(fb)?,
                Declaration::Class(cls) => self.compile_class(cls)?,
                _ => {}
            }
        }
        // Now that every `<fb>_init` has a body, emit the global initializer that
        // initializes VAR_GLOBAL FB instances. Programs' `_init` will call it.
        self.emit_globals_init(unit)?;

        // Then compile programs (which may instantiate FBs)
        for decl in &unit.declarations {
            if let Declaration::Program(p) = decl {
                self.compile_program(p)?;
            }
        }

        // Program instances, task table, `plcc_init`/`plcc_run_task`, and the sized
        // process image: the runtime contract (docs/process-image.md).
        self.finish_runtime_contract(unit)?;

        for name in &helper_fns {
            if let Some(f) = self.module.get_function(name) {
                f.set_linkage(inkwell::module::Linkage::Internal);
            }
        }

        // Structurally malformed IR must never escape this function. `emit_object`
        // runs LLVM at OptimizationLevel::Default; the execution tests JIT at
        // OptimizationLevel::None. Anything the verifier rejects is a codegen bug that
        // the JIT path would otherwise hide until someone ran the real `plcc compile`.
        if let Err(e) = self.module.verify() {
            // Name the functions at fault: the verifier's text does not.
            let bad: Vec<String> = self
                .module
                .get_functions()
                .filter(|f| f.count_basic_blocks() > 0 && !f.verify(false))
                .map(|f| f.get_name().to_string_lossy().into_owned())
                .collect();
            let mut msg = e.to_string();
            if !bad.is_empty() {
                msg = format!("in {}: {msg}", bad.join(", "));
            }
            return Err(CodegenError::InvalidModule(msg));
        }
        Ok(())
    }

    /// Resolve every `TYPE` declaration to a fixed point.
    ///
    /// Returns an error if a declaration never becomes layout-complete: a genuine
    /// cycle (`A : ARRAY OF B; B : ARRAY OF A;`) or a name that resolves to nothing.
    /// Without that check the loop would either stop early and leave an `Unresolved`
    /// in a struct layout, or spin forever.
    fn register_type_declarations(&mut self, unit: &CompilationUnit) -> Result<(), CodegenError> {
        let mut pending: Vec<&TypeDeclaration> = unit
            .declarations
            .iter()
            .filter_map(|d| match d {
                Declaration::TypeDecl(td) => Some(td),
                _ => None,
            })
            .collect();

        // Reject two declarations of the same name before resolving anything. The
        // registry case-folds its keys, so `TYPE Foo` followed by `TYPE foo` was a
        // silent last-wins overwrite, and the program failed later somewhere else
        // entirely — `undefined variable: a` from a field only the first one had.
        let mut seen: HashMap<String, String> = HashMap::new();
        for td in &pending {
            if let Some(first) = seen.insert(td.name.name.to_uppercase(), td.name.name.clone()) {
                return Err(CodegenError::DuplicateType {
                    first,
                    second: td.name.name.clone(),
                });
            }
        }

        // Keep the declarations themselves: STRUCT field defaults live on the AST and
        // are erased by type resolution. See `type_specs`.
        for td in &pending {
            self.type_specs
                .insert(td.name.name.to_uppercase(), td.type_spec.clone());
        }

        while !pending.is_empty() {
            let before = pending.len();
            let mut still = Vec::with_capacity(before);
            for td in pending {
                let mut ty = self.resolve_type_spec(&td.type_spec);
                if let IecType::Enum { name, .. } = &mut ty {
                    *name = td.name.name.clone();
                }
                self.register_enum(&td.name.name, &ty);
                let incomplete = Self::first_unresolved_in_layout(&ty);
                // Register even a partially resolved type: it is what lets the *next*
                // round make progress on whatever referenced it.
                self.type_registry
                    .register(td.name.name.clone(), ty.clone());
                self.type_checker.types.register(td.name.name.clone(), ty);
                if let Some(name) = incomplete {
                    still.push((td, name));
                }
            }
            if still.is_empty() {
                return Ok(());
            }
            if still.len() == before {
                // A whole round with nothing resolved: no later round can do better.
                let (td, unresolved) = &still[0];
                return Err(CodegenError::CyclicType {
                    type_name: td.name.name.clone(),
                    unresolved: unresolved.clone(),
                });
            }
            pending = still.into_iter().map(|(td, _)| td).collect();
        }
        Ok(())
    }

    /// First unresolved type name that would affect `ty`'s memory layout.
    ///
    /// Recursion stops at a POINTER: it is one machine word whatever it points at, so
    /// `TYPE Node : STRUCT next : POINTER TO Node; END_STRUCT; END_TYPE` is complete
    /// after a single round rather than regressing forever.
    fn first_unresolved_in_layout(ty: &IecType) -> Option<String> {
        match ty {
            IecType::Unresolved(name) => Some(name.clone()),
            IecType::Array { element_type, .. } => Self::first_unresolved_in_layout(element_type),
            IecType::Alias { base_type, .. } | IecType::Subrange { base_type, .. } => {
                Self::first_unresolved_in_layout(base_type)
            }
            IecType::Struct { fields, .. } => fields
                .iter()
                .find_map(|(_, t)| Self::first_unresolved_in_layout(t)),
            IecType::Pointer(_) => None,
            _ => None,
        }
    }

    /// First unresolved type name reachable from `ty`, if any.
    ///
    /// Walks through the aggregate types so `ARRAY [1..4] OF TON` and
    /// `STRUCT t : TON; END_STRUCT` are caught too, not just a bare `t : TON;`.
    fn first_unresolved(&self, ty: &IecType) -> Option<String> {
        match ty {
            IecType::Unresolved(name) => Some(name.clone()),
            IecType::Array { element_type, .. } => self.first_unresolved(element_type),
            IecType::Alias {
                base_type: inner, ..
            } => self.first_unresolved(inner),
            // A pointee is not part of this type's layout, and it may legitimately be
            // the type currently being defined: `TYPE Node : STRUCT next : POINTER TO
            // Node; END_STRUCT` snapshots `Node` as unresolved inside itself. So the
            // pointee is checked by *name* against the finished registry instead of
            // structurally — a POINTER TO something that genuinely does not exist is
            // still rejected.
            IecType::Pointer(inner) => match inner.as_ref() {
                IecType::Unresolved(name) if self.type_registry.resolve(name).is_some() => None,
                other => self.first_unresolved(other),
            },
            IecType::Struct { fields, .. } => {
                fields.iter().find_map(|(_, t)| self.first_unresolved(t))
            }
            _ => None,
        }
    }

    /// Reject any declaration whose type name does not resolve.
    ///
    /// This is deliberately in codegen rather than in `plcc-hir`: `plcc compile` and
    /// `plcc sim` never run the HIR checker, so a HIR-only diagnostic would leave the
    /// actual miscompile in place. Codegen is the layer that would otherwise emit the
    /// wrong code, so codegen is where the guarantee has to hold. (A HIR diagnostic on
    /// top of this would be a nicer *message* — with a source span — but it cannot be
    /// the enforcement point.)
    fn validate_declared_types(&mut self, unit: &CompilationUnit) -> Result<(), CodegenError> {
        let check = |this: &mut Self,
                         pou_kind: &'static str,
                         pou_name: &str,
                         blocks: &[VarBlock]|
         -> Result<(), CodegenError> {
            for block in blocks {
                for decl in &block.declarations {
                    let ty = this.resolve_type_spec(&decl.type_spec);
                    if let Some(type_name) = this.first_unresolved(&ty) {
                        return Err(CodegenError::UnknownType {
                            pou_kind,
                            pou_name: pou_name.to_string(),
                            var_name: decl.name.name.clone(),
                            type_name,
                        });
                    }
                }
            }
            Ok(())
        };

        for decl in &unit.declarations {
            match decl {
                Declaration::Program(p) => {
                    check(self, "PROGRAM", &p.name.name.clone(), &p.var_blocks)?
                }
                Declaration::Function(f) => {
                    check(self, "FUNCTION", &f.name.name.clone(), &f.var_blocks)?
                }
                Declaration::FunctionBlock(fb) => {
                    check(
                        self,
                        "FUNCTION_BLOCK",
                        &fb.name.name.clone(),
                        &fb.var_blocks,
                    )?;
                    // A METHOD has its own VAR blocks. Walking only the POU's blocks
                    // let `METHOD M VAR t : TON; END_VAR` through untouched — the same
                    // silent i32-slot miscompile, just one level down.
                    for m in &fb.methods {
                        let name = format!("{}.{}", fb.name.name, m.name.name);
                        check(self, "METHOD", &name, &m.var_blocks)?;
                    }
                }
                Declaration::Class(cls) => {
                    check(self, "CLASS", &cls.name.name.clone(), &cls.var_blocks)?;
                    for m in &cls.methods {
                        let name = format!("{}.{}", cls.name.name, m.name.name);
                        check(self, "METHOD", &name, &m.var_blocks)?;
                    }
                }
                Declaration::GlobalVarDecl(block) => {
                    check(self, "VAR_GLOBAL", "", std::slice::from_ref(block))?
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Name of the generated init function for a POU.
    fn init_fn_name_for(name: &str) -> String {
        format!("{}_init", name.to_lowercase())
    }

    /// Init function name recorded on a compiled FB/CLASS layout, if it has one.
    fn fb_init_fn_name(&self, fb_type_name: &str) -> String {
        self.compiled_fbs
            .get(&fb_type_name.to_uppercase())
            .map(|l| l.init_fn_name.clone())
            .unwrap_or_else(|| Self::init_fn_name_for(fb_type_name))
    }

    /// True when every FB-instance type reachable from `ty` already has a recorded layout.
    fn fb_layout_ready(&self, ty: &IecType) -> bool {
        match ty {
            IecType::FbInstance(name) => self.compiled_fbs.contains_key(&name.to_uppercase()),
            IecType::Array { element_type, .. } => self.fb_layout_ready(element_type),
            IecType::Struct { fields, .. } => fields.iter().all(|(_, t)| self.fb_layout_ready(t)),
            _ => true,
        }
    }

    /// Name of the generated scan function for a POU.
    fn scan_fn_name_for(name: &str) -> String {
        format!("{}_scan", name.to_lowercase())
    }

    /// Get, or declare, `<name>(ptr) -> void`.
    ///
    /// Declaring a prototype creates the `FunctionValue` without a body, so callers
    /// can reference it before the definition is compiled.
    fn declare_state_fn(&self, name: &str) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function(name) {
            return f;
        }
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let fn_type = self.context.void_type().fn_type(&[ptr_ty.into()], false);
        self.module.add_function(name, fn_type, None)
    }

    /// Record (or overwrite) the struct layout for an FB/CLASS.
    ///
    /// Also declares the POU's `_scan` and `_init` prototypes. `compile_fb_call`
    /// resolves the callee with `module.get_function(scan_fn_name)`, which only
    /// succeeds once that `FunctionValue` exists. Creating it in the definition
    /// (`compile_function_block`) made the whole module order-dependent: an OUTER
    /// declared before the INNER it instantiates failed with "FB scan function
    /// 'inner_scan' not found", and so did passing two .st files in the wrong order
    /// on the command line. Layout runs over every POU before any body is compiled,
    /// so declaring here makes every callee resolvable from every caller.
    fn record_pou_layout(&mut self, name: &str, fields: Vec<PouField>) {
        let field_types: Vec<BasicTypeEnum<'ctx>> = fields
            .iter()
            .map(|f| self.iec_to_llvm_type(&f.stored))
            .collect();
        let struct_type = self.context.struct_type(&field_types, false);
        let scan_fn_name = Self::scan_fn_name_for(name);
        let init_fn_name = Self::init_fn_name_for(name);
        self.declare_state_fn(&scan_fn_name);
        self.declare_state_fn(&init_fn_name);
        self.compiled_fbs.insert(
            name.to_uppercase(),
            FbLayout {
                struct_type,
                scan_fn_name,
                init_fn_name,
                fields,
                methods: HashMap::new(),
            },
        );
    }

    /// Resolve the ordered state-struct field list of a POU's variable blocks.
    ///
    /// A VAR_IN_OUT is stored as a pointer, not as a copy. IEC 61131-3 §6.6.1.5
    /// defines VAR_IN_OUT as pass-by-reference: the callee reads and writes the
    /// caller's variable. Giving the state struct a by-value slot meant the callee
    /// mutated a copy and the caller never saw it — `f(t := v)` on
    /// `FUNCTION_BLOCK BUMP VAR_IN_OUT t : DINT; END_VAR t := t + 5;` left v at 10.
    ///
    /// A caller-side copy-in/copy-out would fix that one case and break two others:
    /// two parameters bound to the same variable (the second copy-back would undo the
    /// first), and an FB that reads its VAR_IN_OUT across scans (it must see writes
    /// the caller made between calls). The pointer has neither problem.
    fn resolve_pou_fields(&mut self, var_blocks: &[VarBlock]) -> Vec<PouField> {
        let mut fields = Vec::new();
        for block in var_blocks {
            for decl in &block.declarations {
                let declared = self.resolve_type_spec(&decl.type_spec);
                let is_in_out = block.kind == VarBlockKind::VarInOut;
                let stored = if is_in_out {
                    IecType::Pointer(Box::new(declared.clone()))
                } else {
                    declared.clone()
                };
                // Validated up front by `plan_process_image`; a partial or bad
                // address never reaches here.
                let at = decl
                    .at_address
                    .as_ref()
                    .and_then(|a| Self::parse_located_address(&a.repr, a.span).ok());
                fields.push(PouField {
                    name: decl.name.name.clone(),
                    declared,
                    stored,
                    is_in_out,
                    block: Some(block.kind),
                    at,
                });
            }
        }
        // `IN : BOOL R_EDGE`: the input's previous value and its edge-detected
        // value, appended so declared fields keep their indices.
        for block in var_blocks.iter().filter(|b| b.kind == VarBlockKind::VarInput) {
            for decl in block.declarations.iter().filter(|d| d.edge.is_some()) {
                for prefix in ["__EDGE_M_", "__EDGE_Q_"] {
                    fields.push(PouField {
                        name: format!("{prefix}{}", decl.name.name.to_uppercase()),
                        declared: IecType::Bool,
                        stored: IecType::Bool,
                        is_in_out: false,
                        block: Some(VarBlockKind::Var),
                        at: None,
                    });
                }
            }
        }
        fields
    }

    /// On entry to an FB body: re-initialize every VAR_TEMP (IEC 61131-3: a
    /// temporary is initialized on every call; it used to keep its value from the
    /// previous call, so `tmp : INT := 5; tmp := tmp + 1;` counted up across
    /// scans), and evaluate `R_EDGE` / `F_EDGE` inputs: the body sees the edge, the
    /// raw input stays what the caller passed. The qualifier used to be ignored,
    /// so an `R_EDGE` input read TRUE on every scan the signal was high.
    fn emit_fb_entry(
        &mut self,
        var_blocks: &[VarBlock],
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        for block in var_blocks.iter().filter(|b| b.kind == VarBlockKind::VarTemp) {
            for decl in &block.declarations {
                let Some((ptr, ty)) = self.variables.get(&decl.name.name.to_uppercase()).cloned()
                else {
                    continue;
                };
                let llvm_ty = self.iec_to_llvm_type(&ty);
                self.builder.build_store(ptr, llvm_ty.const_zero()).map_err(err)?;
                self.emit_field_inits(ptr, &decl.type_spec, function)?;
                if let Some(init) = &decl.initializer {
                    self.emit_decl_initializer(ptr, &ty, init, function)?;
                }
            }
        }
        let i8t = self.context.i8_type();
        for block in var_blocks.iter().filter(|b| b.kind == VarBlockKind::VarInput) {
            for decl in &block.declarations {
                let Some(edge) = decl.edge else {
                    continue;
                };
                let key = decl.name.name.to_uppercase();
                let (Some((raw_p, _)), Some((m_p, _)), Some((q_p, _))) = (
                    self.variables.get(&key).cloned(),
                    self.variables.get(&format!("__EDGE_M_{key}")).cloned(),
                    self.variables.get(&format!("__EDGE_Q_{key}")).cloned(),
                ) else {
                    continue;
                };
                let raw = self.builder.build_load(i8t, raw_p, "edge_in").map_err(err)?.into_int_value();
                let raw = self
                    .builder
                    .build_int_compare(IntPredicate::NE, raw, i8t.const_zero(), "edge_raw")
                    .map_err(err)?;
                let m = self.builder.build_load(i8t, m_p, "edge_m").map_err(err)?.into_int_value();
                let m = self
                    .builder
                    .build_int_compare(IntPredicate::NE, m, i8t.const_zero(), "edge_mb")
                    .map_err(err)?;
                let q = match edge {
                    EdgeKind::Rising => {
                        let not_m = self.builder.build_not(m, "not_m").map_err(err)?;
                        self.builder.build_and(raw, not_m, "r_edge").map_err(err)?
                    }
                    EdgeKind::Falling => {
                        let not_raw = self.builder.build_not(raw, "not_raw").map_err(err)?;
                        self.builder.build_and(not_raw, m, "f_edge").map_err(err)?
                    }
                };
                let q8 = self.builder.build_int_z_extend(q, i8t, "edge_q").map_err(err)?;
                self.builder.build_store(q_p, q8).map_err(err)?;
                let raw8 = self.builder.build_int_z_extend(raw, i8t, "edge_raw8").map_err(err)?;
                self.builder.build_store(m_p, raw8).map_err(err)?;
                // In the body, the input's name means the edge.
                self.variables.insert(key, (q_p, IecType::Bool));
            }
        }
        Ok(())
    }

    /// The formal parameters of a FUNCTION or METHOD, in calling-convention order.
    ///
    /// Declaration order (inputs, in-outs and outputs interleaved as declared), which
    /// is also the positional-argument order. VAR_IN_OUT parameters are passed as
    /// pointers to the caller's variable (IEC 61131-3 §6.6.1.5, pass-by-reference).
    /// They used to be neither parameters nor references: a VAR_IN_OUT declaration
    /// fell into the "local variable" branch and became an alloca, so `BUMPF(t := v)`
    /// passed an argument to a function that declared none and LLVM's verifier
    /// rejected the module outright.
    fn resolve_params(&mut self, var_blocks: &[VarBlock]) -> Vec<Param> {
        // Declaration order, whatever the block kind: a positional call binds
        // its arguments in this order (IEC 61131-3 §6.6.1.4.2). Grouping inputs
        // before in-outs made `F(a, b)` on `VAR_IN_OUT x; VAR_INPUT y` pass `a` to
        // y and `b` to x.
        let mut params = Vec::new();
        for block in var_blocks {
            let (is_in_out, is_output) = match block.kind {
                VarBlockKind::VarInput => (false, false),
                VarBlockKind::VarInOut => (true, false),
                VarBlockKind::VarOutput => (false, true),
                _ => continue,
            };
            for decl in &block.declarations {
                let ty = self.resolve_type_spec(&decl.type_spec);
                params.push(Param {
                    name: decl.name.name.clone(),
                    ty,
                    is_in_out,
                    is_output,
                });
            }
        }
        params
    }

    /// LLVM types for `params`: the declared type by value, or an opaque pointer for a
    /// VAR_IN_OUT.
    fn param_llvm_types(&self, params: &[Param]) -> Vec<BasicMetadataTypeEnum<'ctx>> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        params
            .iter()
            .map(|p| {
                if p.is_in_out || p.is_output {
                    ptr_ty.into()
                } else {
                    self.iec_to_llvm_type(&p.ty).into()
                }
            })
            .collect()
    }

    /// Compute the LLVM struct layout of every FUNCTION_BLOCK and CLASS in the unit,
    /// in dependency order, so that nested FB instances get the right member type
    /// regardless of the order the POUs appear in the source.
    fn layout_pou_structs(&mut self, unit: &CompilationUnit) {
        let mut pending: Vec<(String, &[VarBlock])> = Vec::new();
        for decl in &unit.declarations {
            match decl {
                Declaration::FunctionBlock(fb) => {
                    pending.push((fb.name.name.clone(), &fb.var_blocks))
                }
                Declaration::Class(cls) => pending.push((cls.name.name.clone(), &cls.var_blocks)),
                _ => {}
            }
        }

        while !pending.is_empty() {
            let mut deferred: Vec<(String, &[VarBlock])> = Vec::new();
            let mut progress = false;
            for (name, blocks) in std::mem::take(&mut pending) {
                let fields = self.resolve_pou_fields(blocks);
                if fields.iter().all(|f| self.fb_layout_ready(&f.stored)) {
                    self.record_pou_layout(&name, fields);
                    progress = true;
                } else {
                    deferred.push((name, blocks));
                }
            }
            if !progress {
                // Unresolvable (recursive instantiation, or an unknown type). Lay the
                // rest out anyway with whatever iec_to_llvm_type falls back to.
                for (name, blocks) in deferred {
                    let fields = self.resolve_pou_fields(blocks);
                    self.record_pou_layout(&name, fields);
                }
                break;
            }
            pending = deferred;
        }
    }

    /// Without a CONFIGURATION, give every PROGRAM its instance now and lay it out
    /// like an FB, so another POU can call it — `Sub();`, `Sub(k := 1, o => x);` —
    /// and read its variables — `Sub.o` — as CODESYS allows. A called program
    /// then runs only when called (see `finish_runtime_contract`): the implicit
    /// task would otherwise run it a second time.
    ///
    /// With a CONFIGURATION, a program type is callable the same way when it has
    /// at most one instance and that instance is named like the program
    /// (`PROGRAM Main WITH t : Main;` — TwinCAT's model, where a program is its
    /// own single instance): the task and the callers share that instance, and
    /// the task still runs it. A program with several instances, or one named
    /// differently, cannot be called by name.
    fn layout_callable_programs(&mut self, unit: &CompilationUnit) {
        // Uppercase program type → its instances' names, under a CONFIGURATION.
        let mut instances: HashMap<String, Vec<String>> = HashMap::new();
        let mut configured = false;
        for d in &unit.declarations {
            if let Declaration::Configuration(cfg) = d {
                configured = true;
                for res in &cfg.resources {
                    for pc in &res.program_configs {
                        instances
                            .entry(pc.program_type.name.to_uppercase())
                            .or_default()
                            .push(pc.name.name.to_uppercase());
                    }
                }
            }
        }
        for d in &unit.declarations {
            let Declaration::Program(p) = d else {
                continue;
            };
            let key = p.name.name.to_uppercase();
            if configured
                && instances
                    .get(&key)
                    .is_some_and(|names| names.len() > 1 || names.iter().any(|n| *n != key))
            {
                continue;
            }
            if self.compiled_fbs.contains_key(&key) {
                continue; // an FB of the same name; not callable as a program
            }
            let fields = self.resolve_pou_fields(&p.var_blocks);
            self.record_pou_layout(&p.name.name, fields);
            let st = self.compiled_fbs[&key].struct_type;
            let sym = format!("plcc_inst_{}", contract::c_ident(&p.name.name));
            let g = self.module.add_global(st, None, &sym);
            g.set_initializer(&st.const_zero());
            g.set_alignment(8);
            self.program_instances.insert(key, g);
        }
    }

    /// Declare `<pou>_init(ptr) -> void` for every PROGRAM, FUNCTION_BLOCK and CLASS.
    /// Bodies are filled in later; declaring them all first lets init bodies call each
    /// other regardless of declaration order.
    fn declare_init_prototypes(&mut self, unit: &CompilationUnit) {
        for decl in &unit.declarations {
            let name = match decl {
                Declaration::FunctionBlock(fb) => &fb.name.name,
                Declaration::Class(cls) => &cls.name.name,
                Declaration::Program(p) => &p.name.name,
                _ => continue,
            };
            self.declare_state_fn(&Self::init_fn_name_for(name));
        }
    }

    /// Emit `plcc_globals_init()`, which initializes VAR_GLOBAL FB instances by
    /// calling their `<fb>_init` and applies declared STRUCT field defaults. Scalar
    /// globals are already handled by the constant aggregate initializer on the global
    /// itself; a constant aggregate cannot call a function or reach a nested default,
    /// so both need this runtime pass.
    fn emit_globals_init(&mut self, unit: &CompilationUnit) -> Result<(), CodegenError> {
        let Some((global_val, global_struct, names)) = self.global_var.clone() else {
            return Ok(());
        };
        // The declared TypeSpecs and initializers, in the same order the global
        // struct was built.
        let decls: Vec<VarDecl> = unit
            .declarations
            .iter()
            .filter_map(|d| match d {
                Declaration::GlobalVarDecl(block) => Some(block),
                _ => None,
            })
            .flat_map(|block| block.declarations.iter().cloned())
            .collect();
        let specs: Vec<TypeSpec> = decls.iter().map(|d| d.type_spec.clone()).collect();
        // An initializer the constant folder could not turn into the global's static
        // contents (`g : DINT := 2 * 21;`, `g : REAL := -1.5;`) is stored here at run
        // time. It used to leave the global silently zero.
        let runtime_inits: Vec<Option<Expression>> = decls
            .iter()
            .zip(names.iter())
            .map(|(d, (_, ty))| {
                d.initializer
                    .as_ref()
                    .filter(|e| self.eval_const_initializer(e, ty).is_none())
                    .cloned()
            })
            .collect();

        let any_fb = names
            .iter()
            .any(|(_, t)| matches!(t, IecType::FbInstance(_)));
        let any_defaults = specs.iter().any(|spec| {
            let mut out = Vec::new();
            self.collect_field_inits(spec, &mut Vec::new(), &mut Vec::new(), &mut out);
            !out.is_empty()
        });
        let any_runtime = runtime_inits.iter().any(Option::is_some);
        if !any_fb && !any_defaults && !any_runtime {
            return Ok(());
        }

        let fn_type = self.context.void_type().fn_type(&[], false);
        let func = self.module.add_function(GLOBALS_INIT_FN, fn_type, None);
        let entry = self.context.append_basic_block(func, "entry");
        self.builder.position_at_end(entry);

        let global_ptr = global_val.as_pointer_value();
        for (i, (name, ty)) in names.iter().enumerate() {
            let ptr = self
                .builder
                .build_struct_gep(global_struct, global_ptr, i as u32, name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            if let IecType::FbInstance(fb_name) = ty {
                let init_name = self.fb_init_fn_name(fb_name);
                if let Some(init_fn) = self
                    .module
                    .get_function(&init_name)
                    .filter(|f| f.count_basic_blocks() > 0)
                {
                    self.builder
                        .build_call(init_fn, &[ptr.into()], "")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                }
                // `g : FB := (x := 1)`: after the FB's own defaults, before
                // FB_init (CODESYS runs FB_init once initial values are set).
                if let Some(init) = decls.get(i).and_then(|d| d.initializer.clone()) {
                    self.variables.clear();
                    self.add_globals_to_variables()?;
                    self.emit_decl_initializer(ptr, ty, &init, func)?;
                }
                // FB_init, with the declaration's arguments (other globals are
                // visible to them by name).
                if let Some(d) = decls.get(i) {
                    self.variables.clear();
                    self.add_globals_to_variables()?;
                    let saved_state = self.current_state_ptr.take();
                    self.emit_fb_init_call("", name, fb_name, &d.init_args, d.name.span, func)?;
                    self.current_state_ptr = saved_state;
                }
            } else if let Some(spec) = specs.get(i).cloned() {
                self.emit_field_inits(ptr, &spec, func)?;
                if let Some(Some(init)) = runtime_inits.get(i).cloned() {
                    // Other globals are visible to the initializer by name.
                    self.variables.clear();
                    self.add_globals_to_variables()?;
                    self.emit_decl_initializer(ptr, ty, &init, func)?;
                }
            }
        }

        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        Ok(())
    }

    /// Widen/narrow/convert an initializer value so it matches the member's storage type.
    /// Without this, `x : DINT := -1` would store a 16-bit -1 into a 32-bit slot.
    ///
    /// The source type is unknown here, so widening falls back to the destination's
    /// signedness. That is right for a bare literal (`x : DINT := -1` must sign-extend)
    /// and wrong for a typed value, so anything that *has* a source type must call
    /// [`Self::coerce_value`] instead.
    fn coerce_init_value(
        &self,
        val: BasicValueEnum<'ctx>,
        ty: &IecType,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        self.coerce_value(val, None, ty)
    }

    /// True when values of `ty` are widened by zero-extension.
    fn widens_unsigned(ty: &IecType) -> bool {
        matches!(
            ty,
            IecType::Bool
                | IecType::Byte
                | IecType::Word
                | IecType::Dword
                | IecType::Lword
                | IecType::Usint
                | IecType::Uint
                | IecType::Udint
                | IecType::Ulint
                | IecType::Char
                | IecType::Wchar
        )
    }

    /// The value of an expression that is a compile-time integer constant, if it is
    /// one. Only literals and negated literals — enough to keep the common
    /// `BY 2` / `BY -1` loops branch-free, with everything else settled at run time.
    fn const_int_of(expr: &Expression) -> Option<i128> {
        match &expr.kind {
            ExpressionKind::IntegerLiteral(v) => Some(*v),
            ExpressionKind::Parenthesized(inner)
            | ExpressionKind::TypedLiteral { value: inner, .. } => Self::const_int_of(inner),
            ExpressionKind::UnaryOp {
                op: UnaryOp::Neg,
                operand,
            } => Self::const_int_of(operand).and_then(i128::checked_neg),
            _ => None,
        }
    }

    /// Signedness of one operand of a binary operator.
    fn signedness_of(ty: Option<&IecType>) -> Signedness {
        match ty {
            None => Signedness::Adaptive,
            Some(t) if Self::widens_unsigned(t) => Signedness::Unsigned,
            Some(_) => Signedness::Signed,
        }
    }

    /// The width a binary operator runs at, and whether the operator itself is
    /// unsigned (`udiv`/`urem`, `U..` compare predicates).
    ///
    /// The rules, in full:
    ///
    /// * Each operand is widened to the common width by **its own** signedness —
    ///   zero-extension for ANY_BIT and ANY_UNSIGNED, sign-extension otherwise.
    ///   This is the rule assignment, FB inputs and FOR bounds already follow.
    /// * Both operands unsigned: the operator runs unsigned at the wider width.
    ///   `BYTE 200 > BYTE 100` has to be `ugt`, because at i8 the signed reading of
    ///   200 is -56.
    /// * Both operands signed: signed, at the wider width. Unchanged behaviour.
    /// * **Mixed** signed and unsigned: the operator runs *signed*, in a type wide
    ///   enough to hold both operand ranges exactly. When the unsigned operand is at
    ///   least as wide as the signed one, no signed type of the common width holds
    ///   it, so the width is promoted to the next standard size. That is what makes
    ///   `BYTE 250 + SINT 10` evaluate to 260 rather than 4 (i8 wrap) or -6
    ///   (sign-extended 250). Choosing a signed common type — rather than C's
    ///   "unsigned wins" — keeps a negative operand negative, so `INT -5 < BYTE 200`
    ///   is true.
    /// * Mixed at 64 bits has nowhere to be promoted to: IEC has no 128-bit integer.
    ///   The operator runs unsigned at 64 bits, matching the ANY_BIT/ANY_UNSIGNED
    ///   operand, and a negative signed operand there is simply outside the domain.
    /// * An operand with no static IEC type — a bare integer literal, or a call whose
    ///   result type codegen cannot name — is *adaptive*: it takes the other
    ///   operand's signedness, and always widens by sign-extension so a negative
    ///   literal keeps its two's-complement bit pattern. That is what makes
    ///   `IF b > 100` unsigned when `b : BYTE` and signed when `b : INT`, without
    ///   promoting the width and without changing either result type.
    fn promote_int_operands(lw: u32, ls: Signedness, rw: u32, rs: Signedness) -> (u32, bool) {
        let w = lw.max(rw);
        match (ls, rs) {
            (Signedness::Unsigned, Signedness::Unsigned)
            | (Signedness::Unsigned, Signedness::Adaptive)
            | (Signedness::Adaptive, Signedness::Unsigned) => (w, true),
            (Signedness::Unsigned, Signedness::Signed) => Self::promote_mixed(lw, rw, w),
            (Signedness::Signed, Signedness::Unsigned) => Self::promote_mixed(rw, lw, w),
            _ => (w, false),
        }
    }

    /// Common representation for a mixed signed/unsigned operator: a signed type
    /// that holds both ranges, or 64-bit unsigned when no wider type exists.
    fn promote_mixed(unsigned_w: u32, signed_w: u32, common_w: u32) -> (u32, bool) {
        if unsigned_w < signed_w {
            // The signed operand is already wider, so zero-extending the unsigned
            // one lands it in range. Signed at the common width is exact.
            return (common_w, false);
        }
        if unsigned_w >= 64 {
            return (64, true);
        }
        let promoted = if unsigned_w <= 8 {
            16
        } else if unsigned_w <= 16 {
            32
        } else {
            64
        };
        (common_w.max(promoted), false)
    }

    /// Bring two integer operands to the common representation described by
    /// [`Self::promote_int_operands`], and report whether the operator is unsigned.
    fn prepare_int_operands(
        &self,
        l: inkwell::values::IntValue<'ctx>,
        l_ty: Option<&IecType>,
        r: inkwell::values::IntValue<'ctx>,
        r_ty: Option<&IecType>,
    ) -> Result<
        (
            inkwell::values::IntValue<'ctx>,
            inkwell::values::IntValue<'ctx>,
            bool,
        ),
        CodegenError,
    > {
        let ls = Self::signedness_of(l_ty);
        let rs = Self::signedness_of(r_ty);
        let lw = l.get_type().get_bit_width();
        let rw = r.get_type().get_bit_width();
        let (w, unsigned) = Self::promote_int_operands(lw, ls, rw, rs);
        let target = self.context.custom_width_int_type(w);
        Ok((
            self.widen_to(l, ls, target)?,
            self.widen_to(r, rs, target)?,
            unsigned,
        ))
    }

    /// The signedness of a group of operands taken together: signed if any member is
    /// signed, unsigned if any is unsigned and none is signed, adaptive if none has a
    /// static type at all.
    fn combine_signedness(a: Signedness, b: Signedness) -> Signedness {
        match (a, b) {
            (Signedness::Signed, _) | (_, Signedness::Signed) => Signedness::Signed,
            (Signedness::Unsigned, _) | (_, Signedness::Unsigned) => Signedness::Unsigned,
            _ => Signedness::Adaptive,
        }
    }

    /// [`Self::prepare_int_operands`] for three operands that must end up in one
    /// shared representation — LIMIT's MN, IN and MX, which are compared against each
    /// other and selected between, so all three have to reach the same width.
    fn prepare_int_triple(
        &self,
        vals: [inkwell::values::IntValue<'ctx>; 3],
        tys: [Option<&IecType>; 3],
    ) -> Result<([inkwell::values::IntValue<'ctx>; 3], bool), CodegenError> {
        let signs = tys.map(Self::signedness_of);
        let widths = vals.map(|v| v.get_type().get_bit_width());
        // Fold the pairwise rule across the triple: promote against the first two,
        // then against the third with their combined signedness.
        let (w01, _) = Self::promote_int_operands(widths[0], signs[0], widths[1], signs[1]);
        let s01 = Self::combine_signedness(signs[0], signs[1]);
        let (w, unsigned) = Self::promote_int_operands(w01, s01, widths[2], signs[2]);
        let target = self.context.custom_width_int_type(w);
        Ok((
            [
                self.widen_to(vals[0], signs[0], target)?,
                self.widen_to(vals[1], signs[1], target)?,
                self.widen_to(vals[2], signs[2], target)?,
            ],
            unsigned,
        ))
    }

    /// Widen one operand to `to`, zero-extending only when its own type is unsigned.
    fn widen_to(
        &self,
        v: inkwell::values::IntValue<'ctx>,
        s: Signedness,
        to: inkwell::types::IntType<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        if v.get_type().get_bit_width() >= to.get_bit_width() {
            return Ok(v);
        }
        if s == Signedness::Unsigned {
            self.builder.build_int_z_extend(v, to, "zext")
        } else {
            self.builder.build_int_s_extend(v, to, "sext")
        }
        .map_err(|e| CodegenError::LlvmError(e.to_string()))
    }

    /// The IEC type an arithmetic binary operator produces, mirroring exactly what
    /// [`Self::promote_int_operands`] evaluates it at.
    ///
    /// It matters that the two agree: a mixed operator runs in a wider *signed* type,
    /// and if the result were still reported as the unsigned operand's type then
    /// `total : LINT := w - i;` with `w : WORD := 5` and `i : INT := 10` would
    /// zero-extend -5 into 4294967291.
    fn arith_result_type(l: Option<IecType>, r: Option<IecType>) -> Option<IecType> {
        let (Some(lt), Some(rt)) = (l, r) else {
            return None;
        };
        // Only integers take part in the promotion rule. REAL/LREAL, durations and
        // anything without a static width fall through to "the wider operand wins".
        let int_like = |t: &IecType| {
            (t.is_any_int() || t.is_any_bit()) && t.bit_size().is_some_and(|b| b >= 8)
        };
        let lw = lt.bit_size().unwrap_or(0);
        let rw = rt.bit_size().unwrap_or(0);
        if int_like(&lt) && int_like(&rt) {
            let ls = Self::signedness_of(Some(&lt));
            let rs = Self::signedness_of(Some(&rt));
            if ls != rs {
                let (w, unsigned) = Self::promote_int_operands(lw, ls, rw, rs);
                if !unsigned {
                    return Some(match w {
                        0..=8 => IecType::Sint,
                        9..=16 => IecType::Int,
                        17..=32 => IecType::Dint,
                        _ => IecType::Lint,
                    });
                }
            }
        }
        Some(if rw > lw { rt } else { lt })
    }

    /// Convert `val` (of IEC type `src`, when known) to the storage type of `ty`.
    ///
    /// Widening signedness comes from the **source**, not the destination. A BYTE
    /// holding 16#FF is 255, and it is still 255 after `acc : DINT := raw_b;` — taking
    /// the sign from the DINT destination turned every ANY_BIT and ANY_UNSIGNED value
    /// assigned into a wider signed slot into a negative number.
    ///
    /// `src` is `None` for values with no static IEC type (bare literals); the
    /// destination's signedness is the fallback there.
    fn coerce_value(
        &self,
        val: BasicValueEnum<'ctx>,
        src: Option<&IecType>,
        ty: &IecType,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let target = self.iec_to_llvm_type(ty);
        let unsigned = match src {
            Some(src_ty) => Self::widens_unsigned(src_ty),
            None => Self::widens_unsigned(ty),
        };
        match (val, target) {
            (BasicValueEnum::IntValue(iv), BasicTypeEnum::IntType(it)) => {
                let (sw, tw) = (iv.get_type().get_bit_width(), it.get_bit_width());
                if sw == tw {
                    Ok(val)
                } else if sw < tw {
                    let ext = if unsigned {
                        self.builder.build_int_z_extend(iv, it, "initzext")
                    } else {
                        self.builder.build_int_s_extend(iv, it, "initsext")
                    }
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(ext.into())
                } else {
                    let tr = self
                        .builder
                        .build_int_truncate(iv, it, "inittrunc")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(tr.into())
                }
            }
            // By the source's signedness, like integer widening: `r := b;` with
            // `b : BYTE := 200` is 200.0, not -56.0.
            (BasicValueEnum::IntValue(iv), BasicTypeEnum::FloatType(ft)) => {
                let f = if unsigned && src.is_some() {
                    self.builder.build_unsigned_int_to_float(iv, ft, "inituitofp")
                } else {
                    self.builder.build_signed_int_to_float(iv, ft, "initsitofp")
                }
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(f.into())
            }
            (BasicValueEnum::FloatValue(fv), BasicTypeEnum::FloatType(ft)) => {
                if fv.get_type() == ft {
                    Ok(val)
                } else {
                    let c = self
                        .builder
                        .build_float_cast(fv, ft, "initfpcast")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(c.into())
                }
            }
            // An implicit REAL → integer store rounds like REAL_TO_<type>.
            (BasicValueEnum::FloatValue(fv), BasicTypeEnum::IntType(_)) => {
                let i = self.float_to_int(fv, ty, true)?;
                Ok(i.into())
            }
            // An address held in an integer (`pt := ADR(x) + INT_TO_DWORD(n)`, or a
            // literal 0 for "no pointer") stored into a POINTER, and a POINTER stored
            // into an address-sized integer (`dw := pt`), as CODESYS allows.
            (BasicValueEnum::IntValue(iv), BasicTypeEnum::PointerType(pt)) => {
                let iv = self.resize_int(iv, self.addr_int_type(), false)?;
                Ok(self
                    .builder
                    .build_int_to_ptr(iv, pt, "inttoptr")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .into())
            }
            (BasicValueEnum::PointerValue(_), BasicTypeEnum::IntType(it)) => {
                let iv = self.int_operand(val, "a pointer-to-integer store")?;
                Ok(self.resize_int(iv, it, false)?.into())
            }
            // A derived STRUCT (`TYPE D EXTENDS B`, whose fields start with B's)
            // where a B is expected: its B part, as CODESYS allows.
            (BasicValueEnum::StructValue(sv), BasicTypeEnum::StructType(st))
                if sv.get_type() != st
                    && st.count_fields() <= sv.get_type().count_fields()
                    && st
                        .get_field_types()
                        .iter()
                        .zip(sv.get_type().get_field_types())
                        .all(|(a, b)| *a == b) =>
            {
                let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
                let mut out = st.get_undef();
                for i in 0..st.count_fields() {
                    let f = self
                        .builder
                        .build_extract_value(sv, i, "basefield")
                        .map_err(err)?;
                    out = self
                        .builder
                        .build_insert_value(out, f, i, "basepart")
                        .map_err(err)?
                        .into_struct_value();
                }
                Ok(out.into())
            }
            // STRING[n] into STRING[m] (and WSTRING): re-shape the buffer to the
            // destination's length. Storing the source array as is wrote n+1 bytes
            // into an m+1 byte slot — past the end of the variable, over whatever
            // followed it, with no terminating NUL when n > m.
            (BasicValueEnum::ArrayValue(av), BasicTypeEnum::ArrayType(at))
                if matches!(ty, IecType::StringType { .. } | IecType::WstringType { .. })
                    && av.get_type() != at =>
            {
                let src_len = av.get_type().len();
                let dst_len = at.len();
                if dst_len == 0 || av.get_type().get_element_type() != at.get_element_type() {
                    return Err(CodegenError::UnsupportedType(format!(
                        "cannot store a value of this string type into a {ty}"
                    )));
                }
                let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
                let mut out = at.const_zero();
                // Characters 0..m, leaving the last slot as the NUL terminator.
                for i in 0..src_len.min(dst_len - 1) {
                    let ch = self
                        .builder
                        .build_extract_value(av, i, "strch")
                        .map_err(err)?;
                    out = self
                        .builder
                        .build_insert_value(out, ch, i, "strfit")
                        .map_err(err)?
                        .into_array_value();
                }
                Ok(out.into())
            }
            _ => Ok(val),
        }
    }

    /// Everything that has to be initialized inside a value of `spec`'s type.
    ///
    /// A STRUCT field's declared default (`TYPE S : STRUCT a : DINT := 5; END_STRUCT`)
    /// belongs to the *type*, so it applies at every place a value of that type comes
    /// into being — a PROGRAM var, an FB member, a nested STRUCT, an element of an
    /// array of STRUCTs, a VAR_GLOBAL. Nothing applied them at all: `x : S; n := x.a;`
    /// yielded 0.
    ///
    /// Positions are constant GEP index chains from the start of the value, so one
    /// flat list handles every depth uniformly.
    fn collect_field_inits(
        &mut self,
        spec: &TypeSpec,
        path: &mut Vec<u32>,
        visiting: &mut Vec<String>,
        out: &mut Vec<(Vec<u32>, FieldInit)>,
    ) {
        match &spec.kind {
            TypeSpecKind::Named(ident) => {
                let key = ident.name.to_uppercase();
                // An FB instance nested inside a STRUCT needs its own `_init` run;
                // a top-level FB-typed variable is already handled by the caller,
                // hence the empty-path check.
                if !path.is_empty() {
                    if let IecType::FbInstance(fb) = self.resolve_type_spec(spec) {
                        out.push((path.clone(), FieldInit::InitFb(fb)));
                        return;
                    }
                }
                // A TYPE may name another TYPE. Cycles through a named type are
                // rejected by `register_type_declarations`, but guard anyway so a
                // malformed unit cannot make this recurse forever.
                if visiting.contains(&key) {
                    return;
                }
                let Some(inner) = self.type_specs.get(&key).cloned() else {
                    return;
                };
                visiting.push(key);
                self.collect_field_inits(&inner, path, visiting, out);
                visiting.pop();
            }
            TypeSpecKind::Struct(fields) | TypeSpecKind::Union(fields) => {
                for (i, field) in fields.iter().enumerate() {
                    path.push(i as u32);
                    match &field.initializer {
                        Some(init) => {
                            let ty = self.resolve_type_spec(&field.type_spec);
                            out.push((path.clone(), FieldInit::Store(init.clone(), ty)));
                        }
                        None => self.collect_field_inits(&field.type_spec, path, visiting, out),
                    }
                    path.pop();
                }
            }
            TypeSpecKind::Array { base, .. } => {
                // Probe the element type once. An array whose element has nothing to
                // initialize contributes nothing, so the common case costs one walk
                // instead of one per element.
                let mut probe = Vec::new();
                if let IecType::FbInstance(fb) = self.resolve_type_spec(base) {
                    // An ARRAY OF <FB>: every element is an instance whose `_init`
                    // must run. The element probe below starts at an empty path,
                    // where an FB is taken to be the caller's own variable, so the
                    // elements' declared input/output/local defaults were skipped:
                    // `fbs : ARRAY[1..3] OF Counter` ran with `inc := 1` as 0.
                    probe.push((Vec::new(), FieldInit::InitFb(fb)));
                } else {
                    self.collect_field_inits(base, &mut Vec::new(), visiting, &mut probe);
                }
                if probe.is_empty() {
                    return;
                }
                let IecType::Array { ranges, .. } = self.resolve_type_spec(spec) else {
                    return;
                };
                let count: i64 = ranges.iter().map(|(lo, hi)| hi - lo + 1).product();
                for e in 0..count.max(0) {
                    path.push(e as u32);
                    for (sub, action) in &probe {
                        let mut full = path.clone();
                        full.extend_from_slice(sub);
                        out.push((full, action.clone()));
                    }
                    path.pop();
                }
            }
            // A POINTER is one word whatever it points at; nothing inside it belongs
            // to this value. Everything else is a scalar with no interior structure.
            _ => {}
        }
    }

    /// Apply every declared default inside the value at `base_ptr`.
    ///
    /// A no-op for a type with none, which is the overwhelmingly common case.
    fn emit_field_inits(
        &mut self,
        base_ptr: PointerValue<'ctx>,
        spec: &TypeSpec,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let mut inits = Vec::new();
        self.collect_field_inits(spec, &mut Vec::new(), &mut Vec::new(), &mut inits);
        if inits.is_empty() {
            return Ok(());
        }
        let base_ty = {
            let ty = self.resolve_type_spec(spec);
            self.iec_to_llvm_type(&ty)
        };
        let i32_ty = self.context.i32_type();
        for (path, action) in inits {
            let mut indices = vec![i32_ty.const_zero()];
            indices.extend(path.iter().map(|i| i32_ty.const_int(*i as u64, false)));
            let ptr = unsafe {
                self.builder
                    .build_in_bounds_gep(base_ty, base_ptr, &indices, "field_default")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            };
            match action {
                FieldInit::Store(expr, ty) => {
                    if let Some(text) = Self::string_literal_text(&expr) {
                        self.store_string_literal(ptr, &ty, &expr, text)?;
                    } else if matches!(
                        expr.kind,
                        ExpressionKind::ArrayInitializer(_) | ExpressionKind::StructInitializer(_)
                    ) {
                        self.emit_decl_initializer(ptr, &ty, &expr, function)?;
                    } else {
                        let Some(val) = self.compile_expression(&expr, function)? else {
                            return Err(self.no_value_error("STRUCT field default", &expr));
                        };
                        let src = self.rvalue_iec_type(&expr);
                        let val = self.coerce_value(val, src.as_ref(), &ty)?;
                        self.builder
                            .build_store(ptr, val)
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    }
                }
                FieldInit::InitFb(fb_name) => {
                    let init_name = self.fb_init_fn_name(&fb_name);
                    if let Some(init_fn) = self.module.get_function(&init_name) {
                        self.builder
                            .build_call(init_fn, &[ptr.into()], "")
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Emit the body of `<pou>_init(state_ptr)`.
    ///
    /// Walks `var_blocks` in the *same* order used to build the state struct, storing
    /// each declared initial value into its field. FB-instance fields recurse into the
    /// nested type's own `_init`, so arbitrarily deep nesting is initialized.
    fn emit_init_body(
        &mut self,
        pou_name: &str,
        var_blocks: &[VarBlock],
        struct_type: StructType<'ctx>,
        call_globals_init: bool,
    ) -> Result<(), CodegenError> {
        let init_name = Self::init_fn_name_for(pou_name);
        let init_fn = match self.module.get_function(&init_name) {
            Some(f) if f.count_basic_blocks() == 0 => f,
            Some(_) => return Ok(()), // already emitted
            None => {
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let fn_type = self.context.void_type().fn_type(&[ptr_ty.into()], false);
                self.module.add_function(&init_name, fn_type, None)
            }
        };

        let entry = self.context.append_basic_block(init_fn, "entry");
        self.builder.position_at_end(entry);

        let state_ptr = init_fn
            .get_nth_param(0)
            .ok_or_else(|| CodegenError::LlvmError("init fn missing state param".into()))?
            .into_pointer_value();

        let saved_struct_type = self.current_struct_type;
        let saved_state_ptr = self.current_state_ptr;
        self.variables.clear();
        self.current_struct_type = Some(struct_type);
        self.current_state_ptr = Some(state_ptr);

        // Globals first so that same-named POU members shadow them.
        self.add_globals_to_variables()?;

        if call_globals_init {
            if let Some(f) = self.module.get_function(GLOBALS_INIT_FN) {
                self.builder
                    .build_call(f, &[], "")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            }
            if let Some(f) = self.module.get_function(image::IMAGE_INIT_FN) {
                self.builder
                    .build_call(f, &[], "")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            }
        }

        let mut field_idx = 0u32;
        for block in var_blocks {
            for decl in &block.declarations {
                let iec_ty = self.resolve_type_spec(&decl.type_spec);
                // An AT variable is its process-image location: initialize and bind
                // that, not the unused struct slot.
                if let Some(at) = &decl.at_address {
                    let addr = Self::parse_located_address(&at.repr, at.span)?;
                    if let Some(init_expr) = &decl.initializer {
                        self.emit_at_initializer(&addr, &iec_ty, init_expr, init_fn)?;
                    }
                    self.bind_at(&decl.name.name, &addr, &iec_ty);
                    field_idx += 1;
                    continue;
                }
                let ptr = self
                    .builder
                    .build_struct_gep(struct_type, state_ptr, field_idx, &decl.name.name)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

                // A VAR_IN_OUT slot holds the caller's address, and no caller has run
                // yet. Null it so an instance that is initialized but never called
                // holds a defined pointer rather than whatever was in the buffer, and
                // do not bind it as a variable — loading it here would dereference a
                // pointer that does not exist yet.
                if block.kind == VarBlockKind::VarInOut {
                    let null = self.context.ptr_type(AddressSpace::default()).const_null();
                    self.builder
                        .build_store(ptr, null)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    field_idx += 1;
                    continue;
                }

                self.variables
                    .insert(decl.name.name.to_uppercase(), (ptr, iec_ty.clone()));

                if let IecType::FbInstance(inner) = &iec_ty {
                    // Recurse into the nested instance's own initializer.
                    let inner_init_name = self.fb_init_fn_name(inner);
                    if let Some(inner_init) = self.module.get_function(&inner_init_name) {
                        self.builder
                            .build_call(inner_init, &[ptr.into()], "")
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    }
                    // Then the instance's own initializer, which wins over the
                    // FB's declared defaults.
                    if let Some(init_expr) = &decl.initializer {
                        self.emit_decl_initializer(ptr, &iec_ty, init_expr, init_fn)?;
                    }
                    // Then its FB_init, with the declaration's arguments.
                    let inner = inner.clone();
                    self.emit_fb_init_call(
                        pou_name,
                        &decl.name.name,
                        &inner,
                        &decl.init_args,
                        decl.name.span,
                        init_fn,
                    )?;
                } else {
                    // Declared STRUCT field defaults come first, so an explicit
                    // initializer on the variable still wins.
                    self.emit_field_inits(ptr, &decl.type_spec, init_fn)?;
                    if let Some(init_expr) = &decl.initializer {
                        // Goes through emit_decl_initializer, not a bare store, so
                        // ARRAY aggregate initializers (`[10, 20, 30]`, `[3(0)]`)
                        // still work.
                        self.emit_decl_initializer(ptr, &iec_ty, init_expr, init_fn)?;
                    }
                }
                field_idx += 1;
            }
        }

        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        self.current_struct_type = saved_struct_type;
        self.current_state_ptr = saved_state_ptr;
        Ok(())
    }

    /// Number of elements an array type occupies, across all its dimensions.
    fn array_capacity(ranges: &[(i64, i64)]) -> usize {
        ranges
            .iter()
            .map(|(lo, hi)| (hi - lo + 1).max(0) as usize)
            .product()
    }

    /// Expand an aggregate into one initializer expression per array slot, in
    /// row-major order. IEC 61131-3's repetition syntax `n(v)` contributes `n` copies
    /// of `v`.
    ///
    /// Fewer entries than slots is legal — the remainder keeps its zero value. More
    /// entries than slots is not, and is reported rather than written past the end.
    fn flatten_array_aggregate<'e>(
        elements: &'e [ArrayInitElement],
        capacity: usize,
    ) -> Result<Vec<&'e Expression>, CodegenError> {
        let mut flat: Vec<&'e Expression> = Vec::new();
        for elem in elements {
            let count = match &elem.repeat {
                None => 1usize,
                Some(r) => {
                    let n = TypeChecker::const_int_expr(r).ok_or_else(|| {
                        CodegenError::UnsupportedType(
                            "array initializer repetition count must be a constant".into(),
                        )
                    })?;
                    usize::try_from(n).map_err(|_| {
                        CodegenError::UnsupportedType(format!(
                            "array initializer repetition count {n} is not a valid element count"
                        ))
                    })?
                }
            };
            for _ in 0..count {
                flat.push(&elem.value);
            }
        }
        if flat.len() > capacity {
            return Err(CodegenError::UnsupportedType(format!(
                "array initializer has {} values but the array holds {capacity}",
                flat.len()
            )));
        }
        Ok(flat)
    }

    /// Store an array aggregate initializer into the array at `ptr`.
    ///
    /// Multi-dimensional arrays are laid out as one flat LLVM array, so a flat
    /// aggregate fills them in row-major order. A nested aggregate against a nested
    /// array element recurses.
    fn emit_array_aggregate_store(
        &mut self,
        ptr: PointerValue<'ctx>,
        iec_ty: &IecType,
        elements: &[ArrayInitElement],
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let IecType::Array {
            ranges,
            element_type,
        } = iec_ty
        else {
            return Err(CodegenError::UnsupportedType(format!(
                "an array aggregate initializer cannot initialize {iec_ty}"
            )));
        };
        let capacity = Self::array_capacity(ranges);
        let flat = Self::flatten_array_aggregate(elements, capacity)?;
        let arr_llvm_ty = self.iec_to_llvm_type(iec_ty);
        let element_type = (**element_type).clone();
        let zero = self.context.i32_type().const_zero();

        for (i, init_expr) in flat.into_iter().enumerate() {
            let idx = self.context.i32_type().const_int(i as u64, false);
            let elem_ptr = unsafe {
                self.builder
                    .build_in_bounds_gep(arr_llvm_ty, ptr, &[zero, idx], "init_elem")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            };
            self.emit_decl_initializer(elem_ptr, &element_type, init_expr, function)?;
        }
        Ok(())
    }

    /// Store a structure initializer `(x := 1, y := 2)` into the STRUCT at `ptr`.
    /// Fields it does not name keep the value they already have (their declared
    /// default, applied before this).
    fn emit_struct_initializer(
        &mut self,
        ptr: PointerValue<'ctx>,
        iec_ty: &IecType,
        fields: &[StructInitField],
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        // An FB instance (`t : TON := (PT := T#1s)`): the named inputs,
        // outputs and locals of its state struct, applied after its `_init`.
        if let IecType::FbInstance(fb) = iec_ty.base() {
            let Some(layout) = self.compiled_fbs.get(&fb.to_uppercase()).cloned() else {
                return Err(CodegenError::UnsupportedType(format!(
                    "a structure initializer cannot initialize {iec_ty}"
                )));
            };
            for f in fields {
                let Some(idx) = layout
                    .fields
                    .iter()
                    .position(|pf| pf.name.eq_ignore_ascii_case(&f.name.name))
                    .filter(|&i| !layout.fields[i].is_in_out)
                else {
                    return Err(CodegenError::UndefinedVariable(format!(
                        "structure initializer: {iec_ty} has no member `{}` to initialize",
                        f.name.name
                    )));
                };
                let field_ptr = self
                    .builder
                    .build_struct_gep(layout.struct_type, ptr, idx as u32, &f.name.name)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                let field_ty = layout.fields[idx].stored.clone();
                self.emit_decl_initializer(field_ptr, &field_ty, &f.value, function)?;
            }
            return Ok(());
        }
        let IecType::Struct {
            fields: members, ..
        } = iec_ty.base()
        else {
            return Err(CodegenError::UnsupportedType(format!(
                "a structure initializer cannot initialize {iec_ty}"
            )));
        };
        let members = members.clone();
        let struct_ty = self.iec_to_llvm_type(iec_ty.base()).into_struct_type();
        for f in fields {
            let Some(idx) = members
                .iter()
                .position(|(n, _)| n.eq_ignore_ascii_case(&f.name.name))
            else {
                return Err(CodegenError::UndefinedVariable(format!(
                    "structure initializer: {iec_ty} has no field `{}`",
                    f.name.name
                )));
            };
            let field_ptr = self
                .builder
                .build_struct_gep(struct_ty, ptr, idx as u32, &f.name.name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            let field_ty = members[idx].1.clone();
            self.emit_decl_initializer(field_ptr, &field_ty, &f.value, function)?;
        }
        Ok(())
    }

    /// The text of a STRING/WSTRING literal, looking through parentheses.
    fn string_literal_text(expr: &Expression) -> Option<&str> {
        match &expr.kind {
            ExpressionKind::StringLiteral(s) | ExpressionKind::WstringLiteral(s) => Some(s),
            ExpressionKind::Parenthesized(inner) => Self::string_literal_text(inner),
            _ => None,
        }
    }

    /// A string literal as a constant of the storage type of `ty`: the full
    /// `[N+1 x i8]` (STRING) or `[N+1 x i16]` (WSTRING) buffer, truncated to the
    /// declared length and NUL-filled, or a single CHAR/WCHAR. `None` when `ty` is not
    /// a character type (the caller reports it).
    fn string_literal_const(&self, ty: &IecType, text: &str) -> Option<BasicValueEnum<'ctx>> {
        match ty {
            IecType::StringType { max_len } => {
                let cap = max_len.unwrap_or(plcc_hir::types::DEFAULT_STRING_LEN) as usize;
                let i8_ty = self.context.i8_type();
                let mut vals: Vec<_> = decode_iec_string(text, false)
                    .into_iter()
                    .take(cap)
                    .map(|b| i8_ty.const_int(b as u64, false))
                    .collect();
                vals.resize(cap + 1, i8_ty.const_zero());
                Some(i8_ty.const_array(&vals).into())
            }
            IecType::WstringType { max_len } => {
                let cap = max_len.unwrap_or(plcc_hir::types::DEFAULT_STRING_LEN) as usize;
                let i16_ty = self.context.i16_type();
                let mut vals: Vec<_> = decode_iec_string(text, true)
                    .into_iter()
                    .take(cap)
                    .map(|u| i16_ty.const_int(u as u64, false))
                    .collect();
                vals.resize(cap + 1, i16_ty.const_zero());
                Some(i16_ty.const_array(&vals).into())
            }
            // `c : CHAR := 'A';` — a one-character literal.
            IecType::Char => match decode_iec_string(text, false).as_slice() {
                [c] => Some(self.context.i8_type().const_int(*c as u64, false).into()),
                _ => None,
            },
            IecType::Wchar => match decode_iec_string(text, true).as_slice() {
                [c] => Some(self.context.i16_type().const_int(*c as u64, false).into()),
                _ => None,
            },
            _ => None,
        }
    }

    /// Store a string literal into the STRING/WSTRING/CHAR at `ptr`, or report why
    /// it cannot be.
    fn store_string_literal(
        &self,
        ptr: PointerValue<'ctx>,
        ty: &IecType,
        expr: &Expression,
        text: &str,
    ) -> Result<(), CodegenError> {
        let Some(val) = self.string_literal_const(ty, text) else {
            return Err(CodegenError::UnsupportedType(format!(
                "string literal {} cannot initialize or be assigned to a {ty}",
                Self::describe_lvalue(expr)
            )));
        };
        self.builder
            .build_store(ptr, val)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        Ok(())
    }

    /// Apply a declaration's initializer to an already-allocated slot, choosing
    /// between the scalar store and the array-aggregate walk.
    fn emit_decl_initializer(
        &mut self,
        ptr: PointerValue<'ctx>,
        iec_ty: &IecType,
        init: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        if let ExpressionKind::ArrayInitializer(elements) = &init.kind {
            return self.emit_array_aggregate_store(ptr, iec_ty, elements, function);
        }
        if let ExpressionKind::StructInitializer(fields) = &init.kind {
            return self.emit_struct_initializer(ptr, iec_ty, fields, function);
        }
        if let Some(ns) = Self::temporal_literal_ns(init, iec_ty) {
            self.builder
                .build_store(ptr, self.context.i64_type().const_int(ns as u64, true))
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            return Ok(());
        }
        if let Some(text) = Self::string_literal_text(init) {
            return self.store_string_literal(ptr, iec_ty, init, text);
        }
        // An initializer that yields no value is a diagnostic. Skipping the store
        // left `s : STRING := 'abc';` silently empty.
        let Some(val) = self.compile_expression(init, function)? else {
            return Err(self.no_value_error("initial value", init));
        };
        let val = self.coerce_init_value(val, iec_ty)?;
        self.builder
            .build_store(ptr, val)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        Ok(())
    }

    /// Evaluate a constant expression for use as a global initializer.
    fn eval_const_initializer(
        &self,
        expr: &Expression,
        ty: &IecType,
    ) -> Option<BasicValueEnum<'ctx>> {
        match &expr.kind {
            ExpressionKind::IntegerLiteral(_) if Self::temporal_literal_ns(expr, ty).is_some() => {
                let ns = Self::temporal_literal_ns(expr, ty)?;
                Some(self.context.i64_type().const_int(ns as u64, true).into())
            }
            ExpressionKind::IntegerLiteral(v) => Some(self.int_literal(*v).into()),
            ExpressionKind::RealLiteral(v) => Some(self.context.f64_type().const_float(*v).into()),
            ExpressionKind::BoolLiteral(v) => {
                Some(self.context.i8_type().const_int(*v as u64, false).into())
            }
            ExpressionKind::TypedLiteral { type_name, value } => {
                let lit_ty = plcc_hir::types::resolve_type_name(&type_name.name)?;
                self.typed_literal_const(&lit_ty, value)
            }
            ExpressionKind::StringLiteral(text) | ExpressionKind::WstringLiteral(text) => {
                self.string_literal_const(ty, text)
            }
            // A VAR_GLOBAL array's aggregate becomes the global's constant contents;
            // there is no init function to store it from.
            ExpressionKind::ArrayInitializer(elements) => {
                let IecType::Array {
                    ranges,
                    element_type,
                } = ty
                else {
                    return None;
                };
                let capacity = Self::array_capacity(ranges);
                let flat = Self::flatten_array_aggregate(elements, capacity).ok()?;
                let elem_llvm_ty = self.iec_to_llvm_type(element_type);
                let mut values: Vec<BasicValueEnum<'ctx>> = Vec::with_capacity(capacity);
                for init_expr in flat {
                    let val = self.eval_const_initializer(init_expr, element_type)?;
                    let val = if val.is_array_value() {
                        val
                    } else {
                        self.const_coerce(val, element_type)?
                    };
                    values.push(val);
                }
                while values.len() < capacity {
                    values.push(elem_llvm_ty.const_zero());
                }
                Self::const_array_of(elem_llvm_ty, &values).map(Into::into)
            }
            _ => None,
        }
    }

    /// Narrow/widen a constant to the storage type of an array element, without a
    /// builder — global initializers are built before any function exists.
    fn const_coerce(
        &self,
        val: BasicValueEnum<'ctx>,
        ty: &IecType,
    ) -> Option<BasicValueEnum<'ctx>> {
        match self.iec_to_llvm_type(ty) {
            // Mismatched kinds (a REAL literal in an ARRAY OF INT initializer) are
            // not coerced here; `None` sends them down the runtime-store path.
            BasicTypeEnum::IntType(target) => {
                let BasicValueEnum::IntValue(v) = val else {
                    return None;
                };
                let raw = v.get_sign_extended_constant()?;
                Some(target.const_int(raw as u64, true).into())
            }
            BasicTypeEnum::FloatType(target) => {
                let raw = match val {
                    BasicValueEnum::FloatValue(f) => f.get_constant()?.0,
                    BasicValueEnum::IntValue(i) => i.get_sign_extended_constant()? as f64,
                    _ => return None,
                };
                Some(target.const_float(raw).into())
            }
            _ => None,
        }
    }

    /// `const_array` is defined per concrete LLVM type, so the element type has to be
    /// matched out before the array can be built.
    fn const_array_of(
        elem_ty: BasicTypeEnum<'ctx>,
        values: &[BasicValueEnum<'ctx>],
    ) -> Option<inkwell::values::ArrayValue<'ctx>> {
        match elem_ty {
            BasicTypeEnum::IntType(t) => {
                let vs: Vec<_> = values.iter().map(|v| v.into_int_value()).collect();
                Some(t.const_array(&vs))
            }
            BasicTypeEnum::FloatType(t) => {
                let vs: Vec<_> = values.iter().map(|v| v.into_float_value()).collect();
                Some(t.const_array(&vs))
            }
            // ARRAY OF STRING, and nested arrays.
            BasicTypeEnum::ArrayType(t) => {
                let vs: Vec<_> = values.iter().map(|v| v.into_array_value()).collect();
                Some(t.const_array(&vs))
            }
            _ => None,
        }
    }

    /// Add global variable GEPs to the variables map.
    ///
    /// A name the POU already bound — a local, an input, a method parameter, the
    /// FUNCTION result — shadows the global of the same name, as IEC scoping and
    /// CODESYS require. Callers bind globals *after* their own variables, and the
    /// globals used to overwrite them: with `VAR_GLOBAL x`, every `x` in every
    /// program, FB, method and function read and wrote the global.
    fn add_globals_to_variables(&mut self) -> Result<(), CodegenError> {
        let own: std::collections::HashSet<String> = self.variables.keys().cloned().collect();
        if let Some((global_val, global_struct, ref names)) = self.global_var.clone() {
            let global_ptr = global_val.as_pointer_value();
            for (i, (name, iec_ty)) in names.iter().enumerate() {
                if own.contains(&name.to_uppercase()) {
                    continue;
                }
                let ptr = self
                    .builder
                    .build_struct_gep(global_struct, global_ptr, i as u32, name)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                self.variables
                    .insert(name.to_uppercase(), (ptr, iec_ty.clone()));
            }
        }
        // PROGRAM instances, callable like FB instances (implicit task only).
        for (name, g) in self.program_instances.clone() {
            if !self.variables.contains_key(&name) {
                self.variables
                    .insert(name.clone(), (g.as_pointer_value(), IecType::FbInstance(name)));
            }
        }
        // AT globals are the image location, not their (unused) struct slot.
        for (name, addr, ty, _) in self.rt.global_at.clone() {
            if !own.contains(&name.to_uppercase()) {
                self.bind_at(&name, &addr, &ty);
            }
        }
        Ok(())
    }

    /// Compile a POU body with a single exit block that RETURN can branch to.
    ///
    /// On return the builder is positioned at the (unterminated) exit block, so the
    /// caller emits its epilogue — load the result, `ret` — exactly once, and every
    /// RETURN reaches it with the result value as assigned so far.
    fn compile_pou_body(
        &mut self,
        body: &[Statement],
        function: FunctionValue<'ctx>,
        result_var: Option<String>,
    ) -> Result<(), CodegenError> {
        let exit_bb = self.context.append_basic_block(function, "pou_exit");
        let saved = (
            self.return_bb.replace(exit_bb),
            std::mem::replace(&mut self.return_result_var, result_var),
            self.loop_exit_bb.take(),
            self.loop_continue_bb.take(),
        );
        let result = body
            .iter()
            .try_for_each(|stmt| self.compile_statement(stmt, function));
        let (rb, rv, le, lc) = saved;
        self.return_bb = rb;
        self.return_result_var = rv;
        self.loop_exit_bb = le;
        self.loop_continue_bb = lc;
        result?;
        self.branch_to_join(exit_bb)?;
        self.builder.position_at_end(exit_bb);
        Ok(())
    }

    /// `RETURN;` — leave the POU now, from any depth of IF/CASE/loop nesting.
    ///
    /// `RETURN expr;` (not IEC 61131-3, but accepted by CODESYS and parsed here) first
    /// assigns `expr` to the FUNCTION/METHOD result. Like EXIT, the branch terminates
    /// the current block, so anything after it is compiled into a fresh block with no
    /// predecessors rather than after the terminator.
    fn compile_return(
        &mut self,
        value: Option<&Expression>,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let Some(exit_bb) = self.return_bb else {
            return Err(CodegenError::LlvmError(
                "RETURN outside a POU body".to_string(),
            ));
        };
        if let Some(value) = value {
            let Some(name) = self.return_result_var.clone() else {
                return Err(CodegenError::LlvmError(
                    "RETURN with a value in a POU that has no result".to_string(),
                ));
            };
            let Some((ptr, ty)) = self.variables.get(&name).cloned() else {
                return Err(CodegenError::LlvmError(format!(
                    "result variable '{name}' not bound"
                )));
            };
            let Some(val) = self.compile_expression(value, function)? else {
                return Err(self.no_value_error("RETURN value", value));
            };
            let src = self.rvalue_iec_type(value);
            let val = self.coerce_value(val, src.as_ref(), &ty)?;
            self.builder
                .build_store(ptr, val)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        }
        self.branch_to_join(exit_bb)?;
        let after = self.context.append_basic_block(function, "after_return");
        self.builder.position_at_end(after);
        Ok(())
    }

    /// Branch to `target` unless the block being built already ends in a terminator.
    ///
    /// LLVM's builder appends blindly at the end of the current block, so emitting a
    /// second terminator produces "Terminator found in the middle of a basic block"
    /// IR: everything after it is unreachable but still present, and the optimizer is
    /// free to do anything with it. Every structured-control-flow join goes through
    /// here so a body that already terminated (EXIT, a nested join, ...) cannot
    /// corrupt the block.
    fn branch_to_join(
        &self,
        target: inkwell::basic_block::BasicBlock<'ctx>,
    ) -> Result<(), CodegenError> {
        let Some(current) = self.builder.get_insert_block() else {
            return Ok(());
        };
        if current.get_terminator().is_some() {
            return Ok(());
        }
        self.builder
            .build_unconditional_branch(target)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        Ok(())
    }

    /// Convert any integer value to i1 for use in conditional branches.
    fn to_i1(
        &self,
        val: inkwell::values::IntValue<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let bit_width = val.get_type().get_bit_width();
        if bit_width == 1 {
            Ok(val)
        } else {
            // Truncate or compare != 0
            let zero = val.get_type().const_zero();
            self.builder
                .build_int_compare(IntPredicate::NE, val, zero, "tobool")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))
        }
    }

    /// Materialize an integer literal at the narrowest standard width that holds it.
    ///
    /// IEC 61131-3 gives an integer literal the type its context demands. Codegen has
    /// no context here, so it picks by magnitude: INT (i16) for anything that fits —
    /// which is the overwhelmingly common case and keeps ordinary INT arithmetic
    /// 16-bit — then DINT (i32), then LINT (i64). Fixing the width at i16
    /// unconditionally silently truncated every literal above 32767, so
    /// `ctr(PV := 100000)` passed -31072.
    ///
    /// Binary operations sign-extend to the wider operand, and assignment coerces to
    /// the destination, so a wider literal never leaks into a narrower slot.
    fn int_literal(&self, v: i128) -> inkwell::values::IntValue<'ctx> {
        if (i16::MIN as i128..=i16::MAX as i128).contains(&v) {
            self.context.i16_type().const_int(v as u64, true)
        } else if (i32::MIN as i128..=i32::MAX as i128).contains(&v) {
            self.context.i32_type().const_int(v as u64, true)
        } else {
            self.context.i64_type().const_int(v as u64, true)
        }
    }

    fn iec_to_llvm_type(&self, ty: &IecType) -> BasicTypeEnum<'ctx> {
        match ty {
            IecType::Bool => self.context.i8_type().into(), // i8 for memory-safe layout
            // USINT/UINT are 8/16 bits like their signed siblings. They used to be
            // stored one size up (i16/i32), so USINT 200 + 100 was 300 instead of
            // wrapping to 44, NOT USINT#16#0F was 16#FFF0, and the host-visible
            // layout of any POU with a USINT/UINT field disagreed with IEC sizes.
            // Unsignedness is carried by the IEC type, not the width.
            IecType::Sint | IecType::Byte | IecType::Usint => self.context.i8_type().into(),
            IecType::Int | IecType::Word | IecType::Uint => self.context.i16_type().into(),
            IecType::Dint | IecType::Dword | IecType::Udint => {
                self.context.i32_type().into()
            }
            IecType::Lint | IecType::Lword | IecType::Ulint => self.context.i64_type().into(),
            IecType::Real => self.context.f32_type().into(),
            IecType::Lreal => self.context.f64_type().into(),
            IecType::Array {
                ranges,
                element_type,
            } => {
                let elem_ty = self.iec_to_llvm_type(element_type);
                let total_size: u32 = ranges.iter().map(|(lo, hi)| (hi - lo + 1) as u32).product();
                elem_ty.array_type(total_size).into()
            }
            // TIME/LTIME stored as i64 (nanoseconds)
            IecType::Time | IecType::Ltime => self.context.i64_type().into(),
            // DATE types stored as i64 (Unix timestamp in nanoseconds)
            IecType::Date
            | IecType::Tod
            | IecType::Dt
            | IecType::Ldate
            | IecType::Ltod
            | IecType::Ldt => self.context.i64_type().into(),
            // STRING stored as fixed-size byte array (default 256 bytes)
            IecType::StringType { max_len } => {
                let len = max_len.unwrap_or(plcc_hir::types::DEFAULT_STRING_LEN) + 1; // +1 for null terminator
                self.context.i8_type().array_type(len as u32).into()
            }
            IecType::WstringType { max_len } => {
                let len = max_len.unwrap_or(plcc_hir::types::DEFAULT_STRING_LEN) + 1;
                self.context.i16_type().array_type(len as u32).into()
            }
            IecType::Char => self.context.i8_type().into(),
            IecType::Wchar => self.context.i16_type().into(),
            // ENUM backed by base type (default i32)
            IecType::Enum { base_type, .. } => self.iec_to_llvm_type(base_type),
            // Subrange uses base type
            IecType::Subrange { base_type, .. } => self.iec_to_llvm_type(base_type),
            // Pointer as opaque ptr
            IecType::Pointer(_) => self.context.ptr_type(AddressSpace::default()).into(),
            // Struct with known fields
            IecType::Struct { fields, .. } => {
                let field_types: Vec<BasicTypeEnum<'ctx>> = fields
                    .iter()
                    .map(|(_, ft)| self.iec_to_llvm_type(ft))
                    .collect();
                self.context.struct_type(&field_types, false).into()
            }
            // FB instance — look up the compiled FB's struct type
            IecType::FbInstance(name) => {
                if let Some(layout) = self.compiled_fbs.get(&name.to_uppercase()) {
                    layout.struct_type.into()
                } else {
                    // Fallback if FB hasn't been compiled yet
                    self.context.i32_type().into()
                }
            }
            // Fallback for others
            _ => self.context.i32_type().into(),
        }
    }

    fn resolve_type_spec(&mut self, spec: &TypeSpec) -> IecType {
        // A REFERENCE is stored as the address of what it refers to (see refs.rs).
        if let TypeSpecKind::Reference(base) = &spec.kind {
            return IecType::Pointer(Box::new(self.resolve_type_spec(base)));
        }
        let ty = self.type_checker.resolve_type_spec(spec);
        // An inline enumeration (`x : (A, B, C);`) declares its enumerators here.
        if matches!(spec.kind, TypeSpecKind::Enum(_)) {
            self.register_enum("", &ty);
        }
        ty
    }

    /// Point `variables` at every field of a POU state struct.
    ///
    /// A VAR_IN_OUT field holds the *caller's address*, so its variable is the pointer
    /// stored there, loaded once on entry — not the slot itself. Binding the slot would
    /// make the body read and write the pointer word as if it were the value.
    fn bind_state_fields(
        &mut self,
        struct_type: StructType<'ctx>,
        state_ptr: PointerValue<'ctx>,
        fields: &[PouField],
    ) -> Result<(), CodegenError> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        for (i, field) in fields.iter().enumerate() {
            if let Some(addr) = &field.at {
                self.bind_at(&field.name, addr, &field.declared);
                continue;
            }
            let slot = self
                .builder
                .build_struct_gep(struct_type, state_ptr, i as u32, &field.name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            let ptr = if field.is_in_out {
                self.builder
                    .build_load(ptr_ty, slot, &field.name)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .into_pointer_value()
            } else {
                slot
            };
            self.variables
                .insert(field.name.to_uppercase(), (ptr, field.declared.clone()));
        }
        Ok(())
    }

    /// Point `variables` at every formal parameter of a FUNCTION or METHOD.
    ///
    /// A by-value parameter gets an alloca holding a copy, so the body can assign to
    /// it. A VAR_IN_OUT parameter *is* an address — the caller's — so the incoming
    /// pointer becomes the variable directly. Copying it into an alloca would give the
    /// body a local it could write to with no effect on the caller.
    ///
    /// `first` is the index of the first formal parameter: 0 for a FUNCTION, 1 for a
    /// METHOD, which takes the instance pointer first.
    fn bind_params(
        &mut self,
        function: FunctionValue<'ctx>,
        first: u32,
        params: &[Param],
    ) -> Result<(), CodegenError> {
        for (i, param) in params.iter().enumerate() {
            let incoming = function
                .get_nth_param(first + i as u32)
                .ok_or_else(|| CodegenError::LlvmError(format!("missing param {}", param.name)))?;
            let ptr = if param.is_in_out || param.is_output {
                incoming.into_pointer_value()
            } else {
                let llvm_ty = self.iec_to_llvm_type(&param.ty);
                let alloca = self
                    .builder
                    .build_alloca(llvm_ty, &param.name)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                self.builder
                    .build_store(alloca, incoming)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                alloca
            };
            self.variables
                .insert(param.name.to_uppercase(), (ptr, param.ty.clone()));
        }
        Ok(())
    }

    fn compile_program(&mut self, prog: &ProgramDecl) -> Result<(), CodegenError> {
        self.set_fault_site(&prog.name.name, &prog.name.name);
        // Create a scan() function for this program
        let fn_name = format!("{}_scan", prog.name.name.to_lowercase());

        // Build the struct type for program state
        let fields = self.resolve_pou_fields(&prog.var_blocks);
        let field_types: Vec<BasicTypeEnum<'ctx>> = fields
            .iter()
            .map(|f| self.iec_to_llvm_type(&f.stored))
            .collect();

        let struct_type = self.context.struct_type(&field_types, false);
        let state_ptr_type = self.context.ptr_type(AddressSpace::default());

        // Methods (and the properties and actions lowered to methods) of a
        // program: compiled against its single, callable instance.
        let prog_key = prog.name.name.to_uppercase();
        if !prog.methods.is_empty() {
            let Some(layout) = self.compiled_fbs.get(&prog_key) else {
                return Err(CodegenError::Located {
                    message: format!(
                        "PROGRAM `{}` has methods, properties or actions, which need its \
                         single instance: give it at most one instance in the \
                         CONFIGURATION, named `{}`",
                        prog.name.name, prog.name.name
                    ),
                    span: prog.name.span,
                });
            };
            let (layout_struct, layout_fields) = (layout.struct_type, layout.fields.clone());
            for method in &prog.methods {
                let info = self.compile_method(&prog.name.name, method, layout_struct, &layout_fields)?;
                if let Some(l) = self.compiled_fbs.get_mut(&prog_key) {
                    l.methods.insert(method.name.name.to_uppercase(), info);
                }
            }
        }

        // scan(state: *mut ProgramState) -> void
        let fn_type = self
            .context
            .void_type()
            .fn_type(&[state_ptr_type.into()], false);
        let _ = fn_type;
        // Declared up front when another POU may call this program.
        let function = self.declare_state_fn(&fn_name);
        if function.count_basic_blocks() > 0 {
            return Err(CodegenError::LlvmError(format!(
                "duplicate PROGRAM definition '{}'",
                prog.name.name
            )));
        }

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        let state_ptr = function.get_nth_param(0).unwrap().into_pointer_value();

        // Set up variables as GEP into the state struct, and detect FB instances
        self.variables.clear();
        self.current_struct_type = Some(struct_type);
        self.current_state_ptr = Some(state_ptr);

        self.bind_state_fields(struct_type, state_ptr, &fields)?;
        // `M()` inside a program with methods is `THIS^.M()`.
        self.current_pou = None;
        if !prog.methods.is_empty() {
            self.bind_this_super(&prog.name.name, None, state_ptr, function)?;
        }
        self.emit_fb_entry(&prog.var_blocks, function)?;

        // Add global variables
        self.add_globals_to_variables()?;

        // Compile body
        self.compile_pou_body(&prog.body, function, None)?;

        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Generate _init() function that applies variable initializers, initializes
        // VAR_GLOBAL FB instances, and recursively initializes FB instance members.
        self.emit_init_body(&prog.name.name, &prog.var_blocks, struct_type, true)?;

        Ok(())
    }

    /// Declare (or fetch) a FUNCTION's LLVM prototype.
    ///
    /// Done for every FUNCTION before any body is compiled, so a FUNCTION can call
    /// one declared *later* in the unit. Only FB scan functions and methods were
    /// declared up front; a call to a later FUNCTION was "unknown function", which
    /// is what stopped the merged OSCAT compile at its first forward reference
    /// (`SIGN_R`).
    fn declare_function_prototype(&mut self, func: &FunctionDecl) -> FunctionValue<'ctx> {
        let name = func.name.name.to_lowercase();
        if let Some(f) = self.module.get_function(&name) {
            return f;
        }
        let ret_iec_ty = func
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_spec(t))
            .unwrap_or(IecType::Void);
        let params = self.resolve_params(&func.var_blocks);
        let param_types = self.param_llvm_types(&params);
        let fn_type = if ret_iec_ty == IecType::Void {
            self.context.void_type().fn_type(&param_types, false)
        } else {
            let ret_llvm = self.iec_to_llvm_type(&ret_iec_ty);
            ret_llvm.fn_type(&param_types, false)
        };
        self.module.add_function(&name, fn_type, None)
    }

    fn compile_function(&mut self, func: &FunctionDecl) -> Result<(), CodegenError> {
        self.set_fault_site(&func.name.name, &func.name.name);
        let ret_iec_ty = func
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_spec(t))
            .unwrap_or(IecType::Void);

        // Collect params: VAR_INPUT by value, then VAR_IN_OUT by reference.
        let params = self.resolve_params(&func.var_blocks);

        let function = self.declare_function_prototype(func);
        if function.count_basic_blocks() > 0 {
            return Err(CodegenError::LlvmError(format!(
                "duplicate FUNCTION definition '{}'",
                func.name.name
            )));
        }
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        self.variables.clear();

        self.bind_params(function, 0, &params)?;

        // Allocate local vars. VAR_IN_OUT is a parameter, not a local — giving it an
        // alloca made the callee write to a copy that no caller could see.
        for block in &func.var_blocks {
            if !matches!(block.kind, VarBlockKind::VarInput | VarBlockKind::VarInOut) {
                for decl in &block.declarations {
                    let iec_ty = self.resolve_type_spec(&decl.type_spec);
                    let llvm_ty = self.iec_to_llvm_type(&iec_ty);
                    // A VAR_OUTPUT is a parameter (the caller's temporary), bound by
                    // `bind_params`; it is initialized here like any local.
                    let bound_output = if block.kind == VarBlockKind::VarOutput {
                        self.variables
                            .get(&decl.name.name.to_uppercase())
                            .map(|(p, _)| *p)
                    } else {
                        None
                    };
                    let alloca = match bound_output {
                        Some(p) => p,
                        None => self
                            .builder
                            .build_alloca(llvm_ty, &decl.name.name)
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                    };
                    // IEC: a FUNCTION/METHOD local starts every call at its type's
                    // initial value (0 unless declared otherwise). An alloca left
                    // uninitialized made `F := SHL(F, 1) OR 1` accumulate garbage.
                    self.builder
                        .build_store(alloca, llvm_ty.const_zero())
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

                    // Declared STRUCT field defaults, then the variable's own
                    // initializer, so an explicit initializer wins.
                    self.emit_field_inits(alloca, &decl.type_spec, function)?;
                    if let Some(init) = &decl.initializer {
                        self.emit_decl_initializer(alloca, &iec_ty, init, function)?;
                    }

                    self.variables
                        .insert(decl.name.name.to_uppercase(), (alloca, iec_ty));
                }
            }
        }

        // Return value variable (function name = return)
        if ret_iec_ty != IecType::Void {
            let ret_llvm = self.iec_to_llvm_type(&ret_iec_ty);
            let ret_alloca = self
                .builder
                .build_alloca(ret_llvm, &func.name.name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            // The result starts at 0, as a METHOD's already does.
            self.builder
                .build_store(ret_alloca, ret_llvm.const_zero())
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            self.variables.insert(
                func.name.name.to_uppercase(),
                (ret_alloca, ret_iec_ty.clone()),
            );
        }

        // Add global variables
        self.add_globals_to_variables()?;

        // Compile body
        self.compile_pou_body(&func.body, function, (ret_iec_ty != IecType::Void).then(|| func.name.name.to_uppercase()))?;

        // Return
        if ret_iec_ty != IecType::Void {
            let ret_llvm_ty = self.iec_to_llvm_type(&ret_iec_ty);
            if let Some((ptr, _)) = self.variables.get(&func.name.name.to_uppercase()) {
                let val = self
                    .builder
                    .build_load(ret_llvm_ty, *ptr, "retval")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                self.builder
                    .build_return(Some(&val))
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            } else {
                self.builder
                    .build_return(None)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            }
        } else {
            self.builder
                .build_return(None)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        }

        Ok(())
    }

    fn compile_function_block(&mut self, fb: &FunctionBlockDecl) -> Result<(), CodegenError> {
        self.set_fault_site(&fb.name.name, &fb.name.name);
        // Similar to program — fills in the body of the scan function that
        // `record_pou_layout` already declared.
        let fn_name = Self::scan_fn_name_for(&fb.name.name);

        let fields = self.resolve_pou_fields(&fb.var_blocks);
        let field_types: Vec<BasicTypeEnum<'ctx>> = fields
            .iter()
            .map(|f| self.iec_to_llvm_type(&f.stored))
            .collect();

        let struct_type = self.context.struct_type(&field_types, false);
        // Reuse the prototype declared during layout; only create one if this FB was
        // never laid out (defensive — layout covers every FB in the unit).
        let function = self.declare_state_fn(&fn_name);
        if function.count_basic_blocks() > 0 {
            return Err(CodegenError::LlvmError(format!(
                "duplicate FUNCTION_BLOCK definition '{}'",
                fb.name.name
            )));
        }

        // Compile methods first (before the scan body) so they're available
        let mut method_infos: HashMap<String, MethodInfo> = HashMap::new();
        for method in &fb.methods {
            let method_info = self.compile_method(&fb.name.name, method, struct_type, &fields)?;
            method_infos.insert(method.name.name.to_uppercase(), method_info);
        }

        self.compiled_fbs.insert(
            fb.name.name.to_uppercase(),
            FbLayout {
                struct_type,
                scan_fn_name: fn_name.clone(),
                init_fn_name: Self::init_fn_name_for(&fb.name.name),
                fields: fields.clone(),
                methods: method_infos,
            },
        );

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        let state_ptr = function.get_nth_param(0).unwrap().into_pointer_value();

        self.variables.clear();
        // An FB's own state struct is the parent struct for anything it instantiates.
        self.current_struct_type = Some(struct_type);
        self.current_state_ptr = Some(state_ptr);
        self.bind_state_fields(struct_type, state_ptr, &fields)?;
        self.bind_this_super(&fb.name.name, None, state_ptr, function)?;
        self.emit_fb_entry(&fb.var_blocks, function)?;

        // Add global variables
        self.add_globals_to_variables()?;

        self.compile_pou_body(&fb.body, function, None)?;
        self.current_pou = None;

        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Emit `<fb>_init` so declared initial values reach every instance.
        self.emit_init_body(&fb.name.name, &fb.var_blocks, struct_type, false)?;
        Ok(())
    }

    /// Compile a CLASS declaration. A CLASS is like an FB but has no scan body — only methods.
    fn compile_class(&mut self, cls: &ClassDecl) -> Result<(), CodegenError> {
        self.set_fault_site(&cls.name.name, &cls.name.name);
        let fn_name = Self::scan_fn_name_for(&cls.name.name);

        let fields = self.resolve_pou_fields(&cls.var_blocks);
        let field_types: Vec<BasicTypeEnum<'ctx>> = fields
            .iter()
            .map(|f| self.iec_to_llvm_type(&f.stored))
            .collect();

        let struct_type = self.context.struct_type(&field_types, false);

        // Fill in the prototype declared during layout with an empty body (classes
        // have no scan body like FBs).
        let scan_function = self.declare_state_fn(&fn_name);
        if scan_function.count_basic_blocks() > 0 {
            return Err(CodegenError::LlvmError(format!(
                "duplicate CLASS definition '{}'",
                cls.name.name
            )));
        }
        let entry = self.context.append_basic_block(scan_function, "entry");
        self.builder.position_at_end(entry);
        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Compile methods
        let mut method_infos: HashMap<String, MethodInfo> = HashMap::new();
        for method in &cls.methods {
            let method_info = self.compile_method(&cls.name.name, method, struct_type, &fields)?;
            method_infos.insert(method.name.name.to_uppercase(), method_info);
        }

        self.compiled_fbs.insert(
            cls.name.name.to_uppercase(),
            FbLayout {
                struct_type,
                scan_fn_name: fn_name,
                init_fn_name: Self::init_fn_name_for(&cls.name.name),
                fields,
                methods: method_infos,
            },
        );

        // Emit `<cls>_init` so declared initial values reach every instance.
        self.emit_init_body(&cls.name.name, &cls.var_blocks, struct_type, false)?;

        Ok(())
    }

    /// Compute a METHOD's signature and declare (or fetch) its LLVM prototype.
    ///
    /// Split out of [`Self::compile_method`] so [`Self::layout_pou_methods`] can record
    /// every method of every POU before any body is compiled. `compile_method_call`
    /// looks the callee up in the owner's recorded `methods` map, so calling a method
    /// on a CLASS or FB declared *later* in the file used to fail with
    /// "method 'X' not found on FB type 'Y'" — the same declaration-order hazard the
    /// `_scan` prototypes had.
    fn declare_method(
        &mut self,
        fb_name: &str,
        method: &MethodDecl,
    ) -> (MethodInfo, FunctionValue<'ctx>) {
        // `fb.method`: a `.` cannot occur in an ST name, so this never meets
        // the POU's own `fb_init` / `fb_scan` (a method named `Init` used to
        // become a second `fb_init`), nor `A.B_C` against `A_B.C`.
        let method_fn_name = format!(
            "{}.{}",
            fb_name.to_lowercase(),
            method.name.name.to_lowercase()
        );

        let ret_iec_ty = method
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_spec(t))
            .unwrap_or(IecType::Void);

        // First param is always the instance pointer
        let state_ptr_type = self.context.ptr_type(AddressSpace::default());
        let params = self.resolve_params(&method.var_blocks);
        let mut param_types: Vec<BasicMetadataTypeEnum<'ctx>> = vec![state_ptr_type.into()];
        param_types.extend(self.param_llvm_types(&params));

        let fn_type = if ret_iec_ty == IecType::Void {
            self.context.void_type().fn_type(&param_types, false)
        } else {
            let ret_llvm = self.iec_to_llvm_type(&ret_iec_ty);
            ret_llvm.fn_type(&param_types, false)
        };

        let function = match self.module.get_function(&method_fn_name) {
            Some(f) => f,
            None => self.module.add_function(&method_fn_name, fn_type, None),
        };

        (
            MethodInfo {
                fn_name: method_fn_name,
                params,
                return_type: ret_iec_ty,
            },
            function,
        )
    }

    /// Record every FB/CLASS method signature (and declare its prototype) before any
    /// body is compiled, so method calls resolve regardless of declaration order.
    /// Record the declared parameters of every user FUNCTION.
    ///
    /// Runs before any body is compiled, so `compile_expression`'s call path can bind
    /// arguments by name, widen or narrow each to the parameter's declared type, and
    /// take the address of a VAR_IN_OUT argument — regardless of the order the
    /// functions appear in.
    fn layout_function_signatures(&mut self, unit: &CompilationUnit) {
        for decl in &unit.declarations {
            let Declaration::Function(func) = decl else {
                continue;
            };
            let params = self.resolve_params(&func.var_blocks);
            self.fn_signatures
                .insert(func.name.name.to_lowercase(), params);
            self.declare_function_prototype(func);
            if let Some(rt) = &func.return_type {
                let rt = self.resolve_type_spec(rt);
                if rt != IecType::Void {
                    self.fn_return_types
                        .insert(func.name.name.to_lowercase(), rt);
                }
            }
        }
    }

    /// Put a call's arguments into declared-parameter order.
    ///
    /// IEC 61131-3 §6.6.1.6 lets a call name its arguments, and the *name* decides
    /// which parameter each value goes to. Consuming a named argument list
    /// positionally is a silent wrong answer: `SUB2(b := 3, a := 10)` on
    /// `SUB2 := a - b` returned -7 instead of 7.
    ///
    /// A call either names every argument or none, as CODESYS 3 requires. Positional
    /// arguments fill the VAR_INPUT / VAR_IN_OUT parameters in declaration order; a
    /// positional call leaves every VAR_OUTPUT unconnected.
    ///
    /// The result has one slot per parameter. Every input and in-out must be given;
    /// a VAR_OUTPUT slot is `Some` only when the call binds it with `=>`. Binding an
    /// input with `=>`, or an output with `:=`, is an error — CODESYS rejects both.
    fn bind_args<'a>(
        callee: &str,
        params: &[Param],
        args: &'a [CallArg],
    ) -> Result<Vec<Option<&'a CallArg>>, CodegenError> {
        let err = |problem: String| CodegenError::ArgumentBinding {
            callee: callee.to_string(),
            problem,
        };

        let mut bound: Vec<Option<&'a CallArg>> = vec![None; params.len()];
        let mut seen_named = false;
        let mut next_positional = params.iter().enumerate().filter(|(_, p)| !p.is_output);
        for arg in args {
            let Some(name) = &arg.name else {
                if seen_named {
                    return Err(err(
                        "a positional argument cannot follow a named one".into(),
                    ));
                }
                let Some((i, _)) = next_positional.next() else {
                    return Err(err(format!(
                        "too many arguments: it takes {} input(s)",
                        params.iter().filter(|p| !p.is_output).count()
                    )));
                };
                bound[i] = Some(arg);
                continue;
            };
            seen_named = true;
            // `F(10, b := 3)`: does 10 fill `a`, or the first parameter not named
            // later? CODESYS 3 rejects every mix of named and positional arguments,
            // `F(10, o => x)` included, so a call names all its arguments or none.
            if args.iter().any(|a| a.name.is_none()) {
                return Err(err(
                    "named and positional arguments cannot be mixed — name every \
                     argument or none of them (to read an output, name the inputs too: \
                     `F(a := 10, o => x)`)"
                        .into(),
                ));
            }
            let Some(i) = params
                .iter()
                .position(|p| p.name.eq_ignore_ascii_case(&name.name))
            else {
                return Err(err(format!("there is no parameter named `{}`", name.name)));
            };
            let param = &params[i];
            Self::check_binding_direction(callee, &param.name, Self::param_block(param), arg)?;
            if bound[i].is_some() {
                return Err(err(format!("`{}` is given more than once", name.name)));
            }
            bound[i] = Some(arg);
        }
        for (p, b) in params.iter().zip(&bound) {
            if b.is_none() && !p.is_output {
                return Err(err(format!("no value for parameter `{}`", p.name)));
            }
        }
        Ok(bound)
    }

    fn param_block(param: &Param) -> VarBlockKind {
        if param.is_output {
            VarBlockKind::VarOutput
        } else if param.is_in_out {
            VarBlockKind::VarInOut
        } else {
            VarBlockKind::VarInput
        }
    }

    /// `name => v` binds only a VAR_OUTPUT; `name := v` never binds one.
    ///
    /// Treating `=>` as `:=` stored the target into the output before the call and
    /// never copied the result back. Assigning an output with `:=` in a call is an
    /// error in CODESYS, and IEC 61131-3 §6.6.1.4 only defines `=>` for outputs.
    fn check_binding_direction(
        callee: &str,
        param: &str,
        block: VarBlockKind,
        arg: &CallArg,
    ) -> Result<(), CodegenError> {
        let kind = match block {
            VarBlockKind::VarInput => "VAR_INPUT",
            VarBlockKind::VarOutput => "VAR_OUTPUT",
            VarBlockKind::VarInOut => "VAR_IN_OUT",
            _ => "not a parameter (a local variable)",
        };
        let problem = if arg.is_output && block != VarBlockKind::VarOutput {
            format!(
                "`{param}` is {kind}; `=>` binds only a VAR_OUTPUT — pass it with `{param} := ...`"
            )
        } else if !arg.is_output && block == VarBlockKind::VarOutput {
            format!(
                "`{param}` is VAR_OUTPUT and cannot be assigned in a call; \
                 read it with `{param} => variable`"
            )
        } else {
            return Ok(());
        };
        Err(CodegenError::ArgumentBinding {
            callee: callee.to_string(),
            problem,
        })
    }

    /// Allocate a stack slot in `function`'s entry block, whatever block the builder
    /// is in, so a call inside a loop does not grow the stack every iteration.
    fn entry_alloca(
        &self,
        function: FunctionValue<'ctx>,
        ty: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let entry = function
            .get_first_basic_block()
            .ok_or_else(|| CodegenError::LlvmError("function has no entry block".into()))?;
        let b = self.context.create_builder();
        match entry.get_first_instruction() {
            Some(i) => b.position_before(&i),
            None => b.position_at_end(entry),
        }
        b.build_alloca(ty, name)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))
    }

    /// After a call, copy one output (at `src`, of type `src_ty`) to the `=>` target,
    /// inverted for `NOT q => x`.
    ///
    /// The copy is compiled as the assignment `target := [NOT] <output>` so it gets
    /// exactly the assignment semantics — width and signedness coercion, bounded
    /// STRING copies, STRUCT copies, any addressable target (`arr[i]`, `s.f`, `%QX0.0`).
    fn copy_output(
        &mut self,
        arg: &CallArg,
        src: PointerValue<'ctx>,
        src_ty: &IecType,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let tmp_name = format!("__PLCC_OUT{}", self.output_copy_seq);
        self.output_copy_seq += 1;
        self.variables
            .insert(tmp_name.clone(), (src, src_ty.clone()));
        let span = arg.span;
        let mut value = Expression {
            kind: ExpressionKind::Identifier(Ident::new(tmp_name.clone(), span)),
            span,
        };
        if arg.negated {
            value = Expression {
                kind: ExpressionKind::UnaryOp {
                    op: UnaryOp::Not,
                    operand: Box::new(value),
                },
                span,
            };
        }
        let stmt = Statement {
            kind: StatementKind::Assignment {
                target: arg.value.clone(),
                value,
            },
            span,
        };
        let result = self.compile_statement(&stmt, function);
        self.variables.remove(&tmp_name);
        result
    }

    fn layout_pou_methods(&mut self, unit: &CompilationUnit) {
        for decl in &unit.declarations {
            let (name, methods) = match decl {
                Declaration::FunctionBlock(fb) => (&fb.name.name, &fb.methods),
                Declaration::Class(cls) => (&cls.name.name, &cls.methods),
                // A callable program's methods (see `layout_callable_programs`).
                Declaration::Program(p)
                    if !p.methods.is_empty()
                        && self.compiled_fbs.contains_key(&p.name.name.to_uppercase()) =>
                {
                    (&p.name.name, &p.methods)
                }
                _ => continue,
            };
            let mut infos: HashMap<String, MethodInfo> = HashMap::new();
            for method in methods {
                let (info, _) = self.declare_method(name, method);
                infos.insert(method.name.name.to_uppercase(), info);
            }
            if let Some(layout) = self.compiled_fbs.get_mut(&name.to_uppercase()) {
                layout.methods = infos;
            }
        }
    }

    /// Compile a METHOD declaration on an FB/Class.
    /// Produces an LLVM function: `{fb_name}_{method_name}(instance_ptr, ...params) -> ret_type`
    fn compile_method(
        &mut self,
        fb_name: &str,
        method: &MethodDecl,
        fb_struct_type: StructType<'ctx>,
        fb_fields: &[PouField],
    ) -> Result<MethodInfo, CodegenError> {
        let (info, function) = self.declare_method(fb_name, method);
        if function.count_basic_blocks() > 0 {
            // Body already emitted (a duplicate POU definition). Keep the first.
            return Ok(info);
        }
        let ret_iec_ty = info.return_type.clone();
        let params = info.params.clone();

        // An inherited method's spans point into the POU that declared it.
        let saved_site = (self.fault.site.clone(), self.fault.source_pou.clone());
        let declaring = self
            .hierarchy
            .origins
            .get(&(fb_name.to_uppercase(), method.name.name.to_uppercase()))
            .cloned()
            .unwrap_or_else(|| fb_name.to_uppercase());
        self.set_fault_site(&format!("{fb_name}.{}", method.name.name), &declaring);

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        // Save and clear current variables
        let saved_vars = std::mem::take(&mut self.variables);
        let saved_struct_type = self.current_struct_type.take();
        let saved_state_ptr = self.current_state_ptr.take();

        let instance_ptr = function.get_nth_param(0).unwrap().into_pointer_value();

        // Set up FB fields as variables via GEP on the instance pointer
        self.current_struct_type = Some(fb_struct_type);
        self.current_state_ptr = Some(instance_ptr);

        self.bind_state_fields(fb_struct_type, instance_ptr, fb_fields)?;
        let saved_pou = self.current_pou.take();
        self.bind_this_super(fb_name, Some(&method.name.name), instance_ptr, function)?;

        // Method params. Param 0 is instance_ptr, so formals start at index 1.
        self.bind_params(function, 1, &params)?;

        // Allocate local vars (VAR, VAR_TEMP, etc. — not VAR_INPUT or VAR_IN_OUT,
        // which are parameters).
        for block in &method.var_blocks {
            if !matches!(block.kind, VarBlockKind::VarInput | VarBlockKind::VarInOut) {
                for decl in &block.declarations {
                    let iec_ty = self.resolve_type_spec(&decl.type_spec);
                    let llvm_ty = self.iec_to_llvm_type(&iec_ty);
                    // A VAR_OUTPUT is a parameter (the caller's temporary), bound by
                    // `bind_params`; it is initialized here like any local.
                    let bound_output = if block.kind == VarBlockKind::VarOutput {
                        self.variables
                            .get(&decl.name.name.to_uppercase())
                            .map(|(p, _)| *p)
                    } else {
                        None
                    };
                    let alloca = match bound_output {
                        Some(p) => p,
                        None => self
                            .builder
                            .build_alloca(llvm_ty, &decl.name.name)
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                    };
                    // IEC: a FUNCTION/METHOD local starts every call at its type's
                    // initial value (0 unless declared otherwise). An alloca left
                    // uninitialized made `F := SHL(F, 1) OR 1` accumulate garbage.
                    self.builder
                        .build_store(alloca, llvm_ty.const_zero())
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    self.emit_field_inits(alloca, &decl.type_spec, function)?;
                    if let Some(init) = &decl.initializer {
                        self.emit_decl_initializer(alloca, &iec_ty, init, function)?;
                    }
                    self.variables
                        .insert(decl.name.name.to_uppercase(), (alloca, iec_ty));
                }
            }
        }

        // Return value variable (method name = return value, like functions)
        if ret_iec_ty != IecType::Void {
            let ret_llvm = self.iec_to_llvm_type(&ret_iec_ty);
            let ret_alloca = self
                .builder
                .build_alloca(ret_llvm, &method.name.name)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            // Initialize to zero
            self.builder
                .build_store(ret_alloca, ret_llvm.const_zero())
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            self.variables.insert(
                method.name.name.to_uppercase(),
                (ret_alloca, ret_iec_ty.clone()),
            );
        }

        // Add global variables
        self.add_globals_to_variables()?;

        // Compile method body
        self.compile_pou_body(&method.body, function, (ret_iec_ty != IecType::Void).then(|| method.name.name.to_uppercase()))?;

        // Return
        if ret_iec_ty != IecType::Void {
            let ret_llvm_ty = self.iec_to_llvm_type(&ret_iec_ty);
            if let Some((ptr, _)) = self.variables.get(&method.name.name.to_uppercase()) {
                let val = self
                    .builder
                    .build_load(ret_llvm_ty, *ptr, "retval")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                self.builder
                    .build_return(Some(&val))
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            } else {
                self.builder
                    .build_return(Some(&ret_llvm_ty.const_zero()))
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            }
        } else {
            self.builder
                .build_return(None)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        }

        // Restore saved state
        self.variables = saved_vars;
        self.current_pou = saved_pou;
        (self.fault.site, self.fault.source_pou) = saved_site;
        self.current_struct_type = saved_struct_type;
        self.current_state_ptr = saved_state_ptr;

        Ok(info)
    }

    fn compile_statement(
        &mut self,
        stmt: &Statement,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        match &stmt.kind {
            StatementKind::Assignment { target, value } => {
                // A bit-addressed variable or a literal `%QX0.1` target is written
                // with a read-modify-write of its image byte.
                if self.try_compile_image_assignment(target, value, function)? {
                    return Ok(());
                }
                // `x.3 := b;` — CODESYS bit access, a read-modify-write of x.
                if self.try_compile_bit_assignment(target, value, function)? {
                    return Ok(());
                }
                // `itf := inst;` — bind an INTERFACE reference.
                if self.try_compile_interface_store(target, value, function)? {
                    return Ok(());
                }
                // `s := 'abc';` stores the literal's bytes into the STRING buffer.
                if let (Some(text), Some(ty)) =
                    (Self::string_literal_text(value), self.lvalue_iec_type(target))
                {
                    let ptr = self
                        .compile_lvalue_with_fn(target, function)?
                        .ok_or_else(|| {
                            CodegenError::UnsupportedType(format!(
                                "`{}` is not an assignable location",
                                Self::describe_lvalue(target)
                            ))
                        })?;
                    return self.store_string_literal(ptr, &ty, value, text);
                }
                // A STRING target from a string expression (CONCAT, MID, ... at any
                // depth, a FUNCTION returning STRING): bounded copy.
                if !self.try_compile_string_store(target, value, function)? {
                    // An assignment target that resolves to no address is a
                    // diagnostic, not a no-op. `compile_lvalue_inner` returns
                    // `Ok(None)` for expressions with no address at all (literals,
                    // calls, `%IX0.0`), which the string builtins ask about
                    // speculatively — but reaching here means the user wrote it on
                    // the left of `:=`, and dropping the store silently is how this
                    // class of bug has repeatedly gone unnoticed.
                    let ptr = self
                        .compile_lvalue_with_fn(target, function)?
                        .ok_or_else(|| {
                            CodegenError::UnsupportedType(format!(
                                "`{}` is not an assignable location",
                                Self::describe_lvalue(target)
                            ))
                        })?;
                    if let Some(ns) = self
                        .lvalue_iec_type(target)
                        .and_then(|t| Self::temporal_literal_ns(value, &t))
                    {
                        self.builder
                            .build_store(ptr, self.context.i64_type().const_int(ns as u64, true))
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        return Ok(());
                    }
                    // A right-hand side that yields no value is a diagnostic, like an
                    // unaddressable target. Skipping the store instead made
                    // `pt := ADR(bin);` compile to nothing and leave `pt` unset.
                    let Some(val) = self.compile_expression(value, function)? else {
                        return Err(self.no_value_error(
                            format!(
                                "right-hand side of the assignment to `{}`",
                                Self::describe_lvalue(target)
                            ),
                            value,
                        ));
                    };
                    {
                        // Match the store width to the destination. Without this a
                        // narrow RHS (integer literals default to INT/i16) stored
                        // into a wider slot wrote only part of it.
                        // Match the store width to the destination, but take the
                        // *source's* signedness: `acc : DINT := raw_byte;` where
                        // raw_byte is 16#FF must widen to 255, not to -1.
                        let val = match self.lvalue_iec_type(target) {
                            Some(ty) => {
                                let src = self.rvalue_iec_type(value);
                                self.coerce_value(val, src.as_ref(), &ty)?
                            }
                            None => val,
                        };
                        self.builder
                            .build_store(ptr, val)
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    }
                }
            }
            StatementKind::If {
                condition,
                then_body,
                elsif_branches,
                else_body,
            } => {
                self.compile_if(condition, then_body, elsif_branches, else_body, function)?;
            }
            StatementKind::For {
                variable,
                from,
                to,
                by,
                body,
            } => {
                self.compile_for(variable, from, to, by, body, function)?;
            }
            StatementKind::While { condition, body } => {
                self.compile_while(condition, body, function)?;
            }
            StatementKind::Case {
                selector,
                branches,
                else_body,
            } => {
                self.compile_case(selector, branches, else_body, function)?;
            }
            StatementKind::FunctionCall { callee, args } => {
                // `M(..)` inside an FB/CLASS is `THIS^.M(..)` when M is its method.
                if let Some(method) = self.implicit_method_call(callee) {
                    let stmt = Statement {
                        kind: StatementKind::FunctionCall {
                            callee: method,
                            args: args.clone(),
                        },
                        span: stmt.span,
                    };
                    return self.compile_statement(&stmt, function);
                }
                let as_expression = |this: &mut Self| -> Result<(), CodegenError> {
                    let call_expr = Expression {
                        kind: ExpressionKind::FunctionCall {
                            callee: Box::new(callee.clone()),
                            args: args.clone(),
                        },
                        span: stmt.span,
                    };
                    if this.compile_expression(&call_expr, function)?.is_some() {
                        return Ok(());
                    }
                    // No value is expected only from a FUNCTION declared without a
                    // result type. Anything else yielding none means nothing was
                    // emitted: `NOSUCH(x);` used to compile to no code at all.
                    let void_fn = match &callee.kind {
                        ExpressionKind::Identifier(id) => this
                            .module
                            .get_function(&id.name.to_lowercase())
                            .is_some_and(|f| f.get_type().get_return_type().is_none()),
                        _ => false,
                    };
                    if void_fn {
                        this.no_value_cause = None;
                        Ok(())
                    } else {
                        Err(this.no_value_error("call statement", &call_expr))
                    }
                };

                // An FB instance call. The callee is anything addressable whose *type*
                // is an FB instance: a plain variable, a STRUCT field (`x.f(...)`), an
                // array element, a VAR_GLOBAL. Testing a per-POU map of bare identifier
                // names instead dropped every other form on the floor — `x.f(s := 7);`
                // compiled to nothing at all.
                if self.fb_layout_of(callee).is_some() {
                    self.compile_fb_call(callee, args, function)?;
                } else if let ExpressionKind::Identifier(ident) = &callee.kind {
                    if ident.name.eq_ignore_ascii_case("PRINT") {
                        // PRINT('string literal') or PRINT(string_var)
                        // Emits a call to extern void plcc_print(i8*)
                        self.compile_print_call(args, function)?;
                    } else {
                        as_expression(self)?;
                    }
                } else if let ExpressionKind::MemberAccess { object, member } = &callee.kind {
                    // Method call: obj.Method(args). `obj` is the FB instance here —
                    // `obj.Method` itself is not one, which is what distinguishes this
                    // from the FB-call case above.
                    if self.fb_layout_of(object).is_some() {
                        self.compile_method_call(object, &member.name, args, function)?;
                    } else if self.interface_of_expr(object).is_some() {
                        self.compile_interface_call(object, &member.name, args, function)?;
                    } else {
                        as_expression(self)?;
                    }
                } else {
                    as_expression(self)?;
                }
            }
            StatementKind::Repeat { body, until } => {
                self.compile_repeat(body, until, function)?;
            }
            // EXIT / CONTINUE outside a loop are errors (as in CODESYS); they used
            // to be dropped without a word.
            StatementKind::Exit | StatementKind::Continue
                if self.loop_exit_bb.is_none() || self.loop_continue_bb.is_none() =>
            {
                return Err(CodegenError::UnsupportedType(format!(
                    "{} outside a FOR, WHILE or REPEAT loop (source offset {})",
                    if matches!(stmt.kind, StatementKind::Exit) {
                        "EXIT"
                    } else {
                        "CONTINUE"
                    },
                    stmt.span.start
                )));
            }
            StatementKind::Exit => {
                if let Some(exit_bb) = self.loop_exit_bb {
                    self.builder
                        .build_unconditional_branch(exit_bb)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    // Create an unreachable block so subsequent statements have somewhere to go
                    let after_exit = self.context.append_basic_block(function, "after_exit");
                    self.builder.position_at_end(after_exit);
                }
            }
            StatementKind::Continue => {
                // Jump to the enclosing loop's *continue target*, which for a FOR loop
                // is the increment block rather than the condition test. Compiling
                // CONTINUE to nothing at all made it a no-op: the rest of the body ran
                // anyway, so `IF i <= 2 THEN CONTINUE; END_IF; n := n + 1;` counted
                // every iteration.
                if let Some(cont_bb) = self.loop_continue_bb {
                    self.builder
                        .build_unconditional_branch(cont_bb)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let after_continue =
                        self.context.append_basic_block(function, "after_continue");
                    self.builder.position_at_end(after_continue);
                }
            }
            StatementKind::Return { value } => {
                self.compile_return(value.as_ref(), function)?;
            }
            StatementKind::Empty | StatementKind::Comment(_) => {}
        }
        Ok(())
    }

    /// One by-value argument of a FUNCTION or METHOD call, converted to the declared
    /// parameter type when there is one. A string literal becomes a constant of the
    /// STRING parameter's type; an argument that yields no value is a diagnostic
    /// rather than being dropped from the call.
    fn compile_call_arg(
        &mut self,
        arg: &Expression,
        param_ty: Option<&IecType>,
        what: &str,
        function: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        if let Some(ns) = param_ty.and_then(|t| Self::temporal_literal_ns(arg, t)) {
            return Ok(self.context.i64_type().const_int(ns as u64, true).into());
        }
        if let Some(iface) = param_ty.and_then(Self::interface_of_type) {
            if let Some(v) = self.interface_value(arg, &iface, function)? {
                return Ok(v);
            }
        }
        if let (Some(text), Some(ty)) = (Self::string_literal_text(arg), param_ty) {
            return self.string_literal_const(ty, text).ok_or_else(|| {
                CodegenError::UnsupportedType(format!(
                    "{what}: string literal {} cannot be passed as a {ty}",
                    Self::describe_lvalue(arg)
                ))
            });
        }
        let Some(val) = self.compile_expression(arg, function)? else {
            return Err(self.no_value_error(what.to_string(), arg));
        };
        match param_ty {
            Some(ty) => {
                let src = self.rvalue_iec_type(arg);
                self.coerce_value(val, src.as_ref(), ty)
            }
            None => Ok(val),
        }
    }

    /// The address to pass for a VAR_IN_OUT argument.
    ///
    /// A VAR_IN_OUT is bound by reference, so the argument has to be something with an
    /// address. A literal or an expression is not, and quietly passing a temporary
    /// would make the callee's writes vanish — which is the whole class of bug this
    /// fixes, so it is a diagnostic instead.
    fn compile_argument_reference(
        &mut self,
        arg: &Expression,
        callee: &str,
        param_name: &str,
        function: FunctionValue<'ctx>,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        self.compile_lvalue_with_fn(arg, function)?
            .ok_or_else(|| CodegenError::ArgumentBinding {
                callee: callee.to_string(),
                problem: format!(
                    "`{param_name}` is VAR_IN_OUT, so its argument must be a variable, \
                     not `{}`",
                    Self::describe_lvalue(arg)
                ),
            })
    }

    /// Address of the FB/CLASS instance `expr` denotes, with its compiled layout.
    ///
    /// The address comes from the ordinary lvalue path, so every addressable form
    /// works the same way: a variable, `x.f`, `arr[2]`, a VAR_GLOBAL.
    fn fb_instance_at(
        &mut self,
        expr: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<(PointerValue<'ctx>, FbLayout<'ctx>), CodegenError> {
        let layout = self.fb_layout_of(expr).ok_or_else(|| {
            CodegenError::UndefinedVariable(format!(
                "`{}` is not a function block instance",
                Self::describe_lvalue(expr)
            ))
        })?;
        let ptr = self
            .compile_lvalue_inner(expr, Some(function))?
            .ok_or_else(|| {
                CodegenError::UnsupportedType(format!(
                    "`{}` has no address, so the function block instance cannot be called",
                    Self::describe_lvalue(expr)
                ))
            })?;
        Ok((ptr, layout))
    }

    /// Compile an FB instance call: write inputs, call scan, leave outputs in place.
    fn compile_fb_call(
        &mut self,
        instance: &Expression,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let instance_name = Self::describe_lvalue(instance);
        let (fb_ptr, layout) = self.fb_instance_at(instance, function)?;
        let type_name = match self.lvalue_iec_type(instance) {
            Some(IecType::FbInstance(t)) => t,
            _ => instance_name.clone(),
        };
        if self.program_instances.contains_key(&type_name.to_uppercase()) {
            self.called_programs.insert(type_name.to_uppercase());
        }

        self.emit_fb_call(
            fb_ptr,
            layout.struct_type,
            &layout.fields,
            &type_name,
            &layout.scan_fn_name,
            args,
            function,
            &instance_name,
        )
    }

    /// The IEC 61131-3 name of a standard FB parameter written with its CODESYS
    /// name. CODESYS's Standard library names them differently from the standard
    /// the bundled blocks follow — `SR(SET1, RESET)`, `RS(SET, RESET1)`,
    /// `CTU(CU, RESET, PV)`, `CTD(CD, LOAD, PV)`, `CTUD(.., RESET, LOAD, ..)` versus
    /// IEC's `S1`/`R`, `S`/`R1`, `R`, `LD` — and CODESYS code uses its own names, so
    /// both are accepted.
    fn std_fb_param_alias(fb_type: &str, name: &str) -> Option<&'static str> {
        let fb = fb_type.to_uppercase();
        let name = name.to_uppercase();
        Some(match (fb.as_str(), name.as_str()) {
            ("SR", "SET1") => "S1",
            ("SR", "RESET") => "R",
            ("RS", "SET") => "S",
            ("RS", "RESET1") => "R1",
            ("CTU" | "CTUD", "RESET") => "R",
            ("CTD" | "CTUD", "LOAD") => "LD",
            _ => return None,
        })
    }

    /// Give every positional argument of an FB call the name of the parameter it
    /// binds, so the call is lowered as if it had been written formally.
    ///
    /// IEC 61131-3 3rd ed. Annex A lets any `param_assign` omit its name. The
    /// positional (non-formal) order is the declaration order of the VAR_INPUT and
    /// VAR_IN_OUT parameters — outputs are not bound positionally, as for a
    /// FUNCTION call. Unnamed arguments used to be skipped outright, so `f(1, 2);`
    /// ran the FB with its inputs unchanged. Mixing named and positional arguments
    /// is an error (CODESYS 3 rejects it), as is passing more than there are inputs.
    fn name_positional_fb_args<'a>(
        label: &str,
        fields: &[PouField],
        args: &'a [CallArg],
    ) -> Result<std::borrow::Cow<'a, [CallArg]>, CodegenError> {
        if args.iter().all(|a| a.name.is_some()) {
            return Ok(std::borrow::Cow::Borrowed(args));
        }
        let err = |problem: String| CodegenError::ArgumentBinding {
            callee: label.to_string(),
            problem,
        };
        if args.iter().any(|a| a.name.is_some()) {
            return Err(err(
                "named and positional arguments cannot be mixed — name every argument \
                 or none of them"
                    .into(),
            ));
        }
        let params: Vec<&PouField> = fields
            .iter()
            .filter(|f| {
                matches!(
                    f.block,
                    Some(VarBlockKind::VarInput) | Some(VarBlockKind::VarInOut)
                )
            })
            .collect();
        if args.len() > params.len() {
            return Err(err(format!(
                "too many arguments: it takes {} input(s)",
                params.len()
            )));
        }
        Ok(std::borrow::Cow::Owned(
            args.iter()
                .zip(params)
                .map(|(a, p)| {
                    let mut a = a.clone();
                    a.name = Some(Ident {
                        name: p.name.clone(),
                        span: a.span,
                    });
                    a
                })
                .collect(),
        ))
    }

    /// Store the named inputs into an FB instance's state and call its scan function.
    /// Shared by the named-instance and chained-instance call paths.
    #[allow(clippy::too_many_arguments)]
    fn emit_fb_call(
        &mut self,
        fb_ptr: PointerValue<'ctx>,
        struct_type: StructType<'ctx>,
        fields: &[PouField],
        fb_type_name: &str,
        scan_fn_name: &str,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
        label: &str,
    ) -> Result<(), CodegenError> {
        // Write named arguments (inputs) to the FB struct fields. Output bindings
        // (`q => x`) are collected and copied out after the call.
        let mut outputs = Vec::new();
        let args = Self::name_positional_fb_args(label, fields, args)?;
        for arg in args.iter() {
            if let Some(arg_name) = &arg.name {
                // Find the field index in the FB's struct
                let field_idx = fields
                    .iter()
                    .position(|f| f.name.eq_ignore_ascii_case(&arg_name.name))
                    .or_else(|| {
                        let canonical = Self::std_fb_param_alias(fb_type_name, &arg_name.name)?;
                        fields
                            .iter()
                            .position(|f| f.name.eq_ignore_ascii_case(canonical))
                    })
                    .ok_or_else(|| {
                        CodegenError::UndefinedVariable(format!(
                            "FB field '{}' not found in '{}'",
                            arg_name.name, fb_type_name
                        ))
                    })?;
                let field = fields[field_idx].clone();
                if let Some(block) = field.block {
                    Self::check_binding_direction(label, &field.name, block, arg)?;
                }
                let field_ptr = self
                    .builder
                    .build_struct_gep(struct_type, fb_ptr, field_idx as u32, &arg_name.name)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

                if arg.is_output {
                    outputs.push((arg, field_ptr, field.declared.clone()));
                    continue;
                }

                let is_reference_input = self
                    .reference_inputs
                    .contains(&(fb_type_name.to_uppercase(), field.name.to_uppercase()));
                if field.is_in_out || is_reference_input {
                    // Bind the reference: the slot holds the *address* of the caller's
                    // variable, so the FB reads and writes it directly. Passing a copy
                    // is why `f(t := v)` never wrote anything back to v. A
                    // `REFERENCE TO` input is bound the same way (`fb(r := x)` is an
                    // implicit `r REF= x` in CODESYS).
                    let target = self.compile_argument_reference(
                        &arg.value,
                        fb_type_name,
                        &arg_name.name,
                        function,
                    )?;
                    self.builder
                        .build_store(field_ptr, target)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    continue;
                }

                // `ton(PT := 20)`: an integer literal on a TIME input is ms.
                if let Some(ns) = Self::temporal_literal_ns(&arg.value, &field.declared) {
                    self.builder
                        .build_store(field_ptr, self.context.i64_type().const_int(ns as u64, true))
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    continue;
                }
                // `f(shape := inst)` on an INTERFACE-typed input.
                if let Some(iface) = Self::interface_of_type(&field.declared) {
                    if let Some(v) = self.interface_value(&arg.value, &iface, function)? {
                        self.builder
                            .build_store(field_ptr, v)
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        continue;
                    }
                }
                // `f(name := 'abc')`: the literal's bytes into the STRING input.
                if let Some(text) = Self::string_literal_text(&arg.value) {
                    self.store_string_literal(field_ptr, &field.declared, &arg.value, text)?;
                    continue;
                }
                // Compile the argument value. One that yields none is a diagnostic:
                // skipping the store left the input at its previous value.
                let Some(val) = self.compile_expression(&arg.value, function)? else {
                    return Err(self.no_value_error(
                        format!("input `{}` of `{fb_type_name}`", arg_name.name),
                        &arg.value,
                    ));
                };
                // Widen/narrow to the declared input type. Integer literals
                // default to INT (i16), so `ctr(PV := 100000)` on a DINT input
                // used to store a truncated 16-bit value into a 32-bit field.
                let src = self.rvalue_iec_type(&arg.value);
                let val = self.coerce_value(val, src.as_ref(), &field.declared)?;
                self.builder
                    .build_store(field_ptr, val)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
            }
        }

        // Call the FB's scan function
        let scan_fn = self.module.get_function(scan_fn_name).ok_or_else(|| {
            CodegenError::LlvmError(format!("FB scan function '{}' not found", scan_fn_name))
        })?;
        self.builder
            .build_call(scan_fn, &[fb_ptr.into()], "fb_call")
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        for (arg, field_ptr, ty) in outputs {
            self.copy_output(arg, field_ptr, &ty, function)?;
        }
        Ok(())
    }

    /// Compile a method call: `instance.MethodName(args)`
    /// Returns the method's return value (if any).
    fn compile_method_call(
        &mut self,
        instance: &Expression,
        method_name: &str,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let instance_name = Self::describe_lvalue(instance);
        let (fb_ptr, layout) = self.fb_instance_at(instance, function)?;

        let method_info = layout
            .methods
            .get(&method_name.to_uppercase())
            .ok_or_else(|| {
                CodegenError::UndefinedVariable(format!(
                    "method '{method_name}' not found on `{instance_name}`"
                ))
            })?
            .clone();

        // Build argument list: instance pointer + method params
        let mut call_args: Vec<BasicValueEnum<'ctx>> = vec![fb_ptr.into()];

        // Method params can be positional or named; named ones bind by name. Either
        // way the value has to be widened/narrowed to the declared parameter type: an
        // integer literal is an INT (i16), so `acc.Bump(amount := 7)` on an
        // `amount : DINT` parameter would otherwise pass an i16 to an i32 parameter
        // and produce an invalid module.
        let ordered = Self::bind_args(
            &format!("{instance_name}.{method_name}"),
            &method_info.params,
            args,
        )?;
        let mut outputs = Vec::new();
        for (i, bound) in ordered.iter().enumerate() {
            let param = method_info.params.get(i);
            if let Some(p) = param.filter(|p| p.is_output) {
                let llvm_ty = self.iec_to_llvm_type(&p.ty);
                let tmp = self.entry_alloca(function, llvm_ty, &p.name)?;
                if let Some(a) = bound {
                    outputs.push((*a, tmp, p.ty.clone()));
                }
                call_args.push(tmp.into());
                continue;
            }
            let Some(bound) = bound else {
                return Err(CodegenError::ArgumentBinding {
                    callee: format!("{instance_name}.{method_name}"),
                    problem: format!("no value for argument {}", i + 1),
                });
            };
            let arg = &bound.value;
            if param.is_some_and(|p| p.is_in_out) {
                call_args.push(
                    self.compile_argument_reference(
                        arg,
                        &format!("{instance_name}.{method_name}"),
                        &param.expect("checked").name,
                        function,
                    )?
                    .into(),
                );
                continue;
            }
            let val = self.compile_call_arg(
                arg,
                param.map(|p| &p.ty),
                &format!("argument {} of `{instance_name}.{method_name}`", i + 1),
                function,
            )?;
            call_args.push(val);
        }

        let method_fn = self
            .module
            .get_function(&method_info.fn_name)
            .ok_or_else(|| {
                CodegenError::LlvmError(format!(
                    "method function '{}' not found",
                    method_info.fn_name
                ))
            })?;

        let call_args_meta: Vec<inkwell::values::BasicMetadataValueEnum> =
            call_args.iter().map(|v| (*v).into()).collect();
        let call = self
            .builder
            .build_call(method_fn, &call_args_meta, "method_call")
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        for (a, tmp, ty) in outputs {
            self.copy_output(a, tmp, &ty, function)?;
        }

        match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => Ok(Some(v)),
            inkwell::values::ValueKind::Instruction(_) => Ok(None),
        }
    }

    fn compile_if(
        &mut self,
        condition: &Expression,
        then_body: &[Statement],
        elsif_branches: &[ElsifBranch],
        else_body: &Option<Vec<Statement>>,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let cond_val = self
            .compile_expression(condition, function)?
            .ok_or_else(|| self.no_value_error("IF condition", condition))?;

        let then_bb = self.context.append_basic_block(function, "then");
        let merge_bb = self.context.append_basic_block(function, "merge");

        if elsif_branches.is_empty() && else_body.is_none() {
            self.builder
                .build_conditional_branch(self.to_i1(self.int_operand(cond_val, "a condition")?)?, then_bb, merge_bb)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

            self.builder.position_at_end(then_bb);
            for stmt in then_body {
                self.compile_statement(stmt, function)?;
            }
            self.branch_to_join(merge_bb)?;
        } else {
            let else_bb = self.context.append_basic_block(function, "else");
            self.builder
                .build_conditional_branch(self.to_i1(self.int_operand(cond_val, "a condition")?)?, then_bb, else_bb)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

            self.builder.position_at_end(then_bb);
            for stmt in then_body {
                self.compile_statement(stmt, function)?;
            }
            self.branch_to_join(merge_bb)?;

            self.builder.position_at_end(else_bb);
            // Handle elsif chains
            if !elsif_branches.is_empty() {
                for (i, branch) in elsif_branches.iter().enumerate() {
                    let elsif_cond = self
                        .compile_expression(&branch.condition, function)?
                        .ok_or_else(|| self.no_value_error("ELSIF condition", &branch.condition))?;

                    let elsif_then = self.context.append_basic_block(function, "elsif_then");
                    // Always a fresh block, never `merge_bb` itself. Aliasing the last
                    // ELSIF's false edge onto the join meant the fall-through branch
                    // emitted below landed *in* the join block as `merge: br %merge` —
                    // an infinite self-loop, with the rest of the enclosing body
                    // appended after that terminator.
                    let elsif_else = self.context.append_basic_block(function, "elsif_else");

                    self.builder
                        .build_conditional_branch(
                            self.to_i1(self.int_operand(elsif_cond, "an ELSIF condition")?)?,
                            elsif_then,
                            elsif_else,
                        )
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

                    self.builder.position_at_end(elsif_then);
                    for stmt in &branch.body {
                        self.compile_statement(stmt, function)?;
                    }
                    self.branch_to_join(merge_bb)?;

                    self.builder.position_at_end(elsif_else);
                }
            }

            if let Some(body) = else_body {
                for stmt in body {
                    self.compile_statement(stmt, function)?;
                }
            }
            self.branch_to_join(merge_bb)?;
        }

        self.builder.position_at_end(merge_bb);
        Ok(())
    }

    fn compile_for(
        &mut self,
        variable: &Expression,
        from: &Expression,
        to: &Expression,
        by: &Option<Expression>,
        body: &[Statement],
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let var_name = Self::describe_lvalue(variable);
        // A plain name, or (TwinCAT) a location such as `idx[2]`, whose
        // address is taken once, before the loop.
        let (var_ptr, var_ty) = match &variable.kind {
            ExpressionKind::Identifier(id) => self
                .variables
                .get(&id.name.to_uppercase())
                .ok_or_else(|| CodegenError::UndefinedVariable(id.name.clone()))?
                .clone(),
            _ => {
                let ty = self.lvalue_iec_type(variable).ok_or_else(|| {
                    CodegenError::UnsupportedType(format!(
                        "cannot determine the type of the FOR control variable `{var_name}`"
                    ))
                })?;
                let ptr = self
                    .compile_lvalue_inner(variable, Some(function))?
                    .ok_or_else(|| {
                        CodegenError::UnsupportedType(format!(
                            "the FOR control variable `{var_name}` has no address"
                        ))
                    })?;
                (ptr, ty)
            }
        };

        let from_val = self
            .compile_expression(from, function)?
            .ok_or_else(|| self.no_value_error("FOR start value", from))?;
        let to_val = self
            .compile_expression(to, function)?
            .ok_or_else(|| self.no_value_error("FOR end value", to))?;
        let to_ty = self.rvalue_iec_type(to);

        // Store the initial value at the control variable's own width. A raw store
        // of a narrower value wrote only its low bytes and left the rest of the slot
        // holding whatever was there, so `FOR i := lo TO 260` with `i : DINT` and
        // `lo : BYTE := 16#FE` started at 0x000000FE-or-worse rather than at 254.
        let from_ty = self.rvalue_iec_type(from);
        let from_val = self.coerce_value(from_val, from_ty.as_ref(), &var_ty)?;
        self.builder
            .build_store(var_ptr, from_val)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        let llvm_ty = self.iec_to_llvm_type(&var_ty);

        // The step is evaluated once, before the loop, for two reasons. IEC 61131-3
        // says so — BY is not re-read per iteration. And the loop condition needs it:
        // the direction of the comparison depends on the step's sign, and a step held
        // in a variable only has a sign at run time.
        let (step, step_ty) = match by {
            Some(by_expr) => {
                let val = self
                    .compile_expression(by_expr, function)?
                    .ok_or_else(|| self.no_value_error("FOR step (BY)", by_expr))?;
                (self.int_operand(val, "a FOR step (BY)")?, self.rvalue_iec_type(by_expr))
            }
            None => (
                llvm_ty.into_int_type().const_int(1, false),
                Some(var_ty.clone()),
            ),
        };

        // Which way the control variable walks. `BY -1` and `BY 10` are settled here;
        // `BY st` with `st : INT` is not, and used to be treated as ascending because
        // the check was syntactic — a literal `< 0` or a unary minus. `FOR i := 5 TO 1
        // BY st` with `st := -1` then compared with SLE and ran zero iterations.
        let step_dir = match by {
            // An ANY_BIT or ANY_UNSIGNED step cannot be negative, whatever it holds.
            _ if step_ty.as_ref().is_some_and(Self::widens_unsigned) => StepDir::Up,
            None => StepDir::Up,
            Some(by_expr) => match Self::const_int_of(by_expr) {
                // `BY 0` never reaches the end value: an endless loop, which
                // IEC 61131-3 makes an error.
                Some(0) => {
                    return Err(CodegenError::UnsupportedType(format!(
                        "FOR `{}` ... BY 0 never terminates (source offset {})",
                        var_name, by_expr.span.start
                    )));
                }
                Some(v) if v < 0 => StepDir::Down,
                Some(_) => StepDir::Up,
                None => {
                    let zero = step.get_type().const_zero();
                    let neg = self
                        .builder
                        .build_int_compare(IntPredicate::SLT, step, zero, "step_neg")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    StepDir::Runtime(neg)
                }
            },
        };

        let loop_bb = self.context.append_basic_block(function, "for_loop");
        let body_bb = self.context.append_basic_block(function, "for_body");
        // The increment lives in its own block so CONTINUE has somewhere to jump that
        // still advances the control variable. Branching straight back to `loop_bb`
        // would re-test the same value and never terminate.
        let inc_bb = self.context.append_basic_block(function, "for_inc");
        let end_bb = self.context.append_basic_block(function, "for_end");

        // Save and set loop_exit_bb for EXIT support, loop_continue_bb for CONTINUE
        let prev_exit_bb = self.loop_exit_bb;
        let prev_continue_bb = self.loop_continue_bb;
        self.loop_exit_bb = Some(end_bb);
        self.loop_continue_bb = Some(inc_bb);

        self.builder
            .build_unconditional_branch(loop_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Loop condition
        self.builder.position_at_end(loop_bb);
        let cur_val = self
            .builder
            .build_load(llvm_ty, var_ptr, "cur")
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        // The bound widens by *its* signedness, not the control variable's, and the
        // predicate follows the pair's signedness: an unsigned control variable
        // holding a value above its signed range compared wrongly with SLE, so
        // `FOR i := 100 TO lim` with `i, lim : BYTE` and `lim := 200` ran no
        // iterations at all.
        let (cur_i, to_i, unsigned) = self.prepare_int_operands(
            self.int_operand(cur_val, "a FOR control variable")?,
            Some(&var_ty),
            self.int_operand(to_val, "a FOR bound (TO)")?,
            to_ty.as_ref(),
        )?;
        let (le, ge) = if unsigned {
            (IntPredicate::ULE, IntPredicate::UGE)
        } else {
            (IntPredicate::SLE, IntPredicate::SGE)
        };
        let cond = match step_dir {
            StepDir::Up => self
                .builder
                .build_int_compare(le, cur_i, to_i, "for_cond")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
            StepDir::Down => self
                .builder
                .build_int_compare(ge, cur_i, to_i, "for_cond")
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
            StepDir::Runtime(neg) => {
                let up = self
                    .builder
                    .build_int_compare(le, cur_i, to_i, "for_cond_up")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                let down = self
                    .builder
                    .build_int_compare(ge, cur_i, to_i, "for_cond_down")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                self.builder
                    .build_select(neg, down, up, "for_cond")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    .into_int_value()
            }
        };
        self.builder
            .build_conditional_branch(cond, body_bb, end_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Body
        self.builder.position_at_end(body_bb);
        for stmt in body {
            self.compile_statement(stmt, function)?;
        }
        self.branch_to_join(inc_bb)?;

        // Increment
        self.builder.position_at_end(inc_bb);
        let cur_val2 = self
            .builder
            .build_load(llvm_ty, var_ptr, "cur2")
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        // Same rule for the step: `BY st` with `st : BYTE := 16#C8` is +200, and
        // sign-extending it to -56 walked the control variable downward forever.
        //
        // The increment is `i := i + BY` in the control variable's own type — it
        // wraps — followed by the ordinary bound test, as in CODESYS. So `FOR b :=
        // 250 TO 255` on a BYTE goes 255 → 0 → ... and never ends: the CODESYS help
        // (ST statement FOR) warns that an end value equal to the type's upper limit
        // gives an infinite loop. The sum is formed wide enough for both operands
        // and truncated, which is that same modulo-2^n addition.
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let cur_i = self.int_operand(cur_val2, "a FOR control variable")?;
        let var_bits = cur_i.get_type().get_bit_width();
        let wide_bits = var_bits.max(step.get_type().get_bit_width());
        let wide = self.context.custom_width_int_type(wide_bits);
        let var_unsigned = Self::widens_unsigned(&var_ty);
        let cur_w = self.resize_int(cur_i, wide, !var_unsigned)?;
        let step_signed = !step_ty.as_ref().is_some_and(Self::widens_unsigned);
        let step_w = self.resize_int(step, wide, step_signed)?;
        let next_w = self.builder.build_int_add(cur_w, step_w, "next").map_err(err)?;
        let back = self.resize_int(next_w, cur_i.get_type(), true)?;
        let next_val = self.coerce_value(back.into(), Some(&var_ty), &var_ty)?;
        self.builder.build_store(var_ptr, next_val).map_err(err)?;
        if self.builder.get_insert_block().and_then(|b| b.get_terminator()).is_none() {
            self.builder.build_unconditional_branch(loop_bb).map_err(err)?;
        }

        self.builder.position_at_end(end_bb);
        self.loop_exit_bb = prev_exit_bb;
        self.loop_continue_bb = prev_continue_bb;
        Ok(())
    }

    fn compile_while(
        &mut self,
        condition: &Expression,
        body: &[Statement],
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let cond_bb = self.context.append_basic_block(function, "while_cond");
        let body_bb = self.context.append_basic_block(function, "while_body");
        let end_bb = self.context.append_basic_block(function, "while_end");

        // Save and set loop_exit_bb for EXIT support, loop_continue_bb for CONTINUE.
        // A WHILE loop's next iteration starts at the condition test.
        let prev_exit_bb = self.loop_exit_bb;
        let prev_continue_bb = self.loop_continue_bb;
        self.loop_exit_bb = Some(end_bb);
        self.loop_continue_bb = Some(cond_bb);

        self.builder
            .build_unconditional_branch(cond_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        self.builder.position_at_end(cond_bb);
        let cond_val = self
            .compile_expression(condition, function)?
            .ok_or_else(|| self.no_value_error("WHILE condition", condition))?;
        // BOOL is an i8 in this ABI; a branch condition must be i1.
        let cond_bool = self.to_i1(self.int_operand(cond_val, "a condition")?)?;
        self.builder
            .build_conditional_branch(cond_bool, body_bb, end_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        self.builder.position_at_end(body_bb);
        for stmt in body {
            self.compile_statement(stmt, function)?;
        }
        self.branch_to_join(cond_bb)?;

        self.builder.position_at_end(end_bb);
        self.loop_exit_bb = prev_exit_bb;
        self.loop_continue_bb = prev_continue_bb;
        Ok(())
    }

    fn compile_repeat(
        &mut self,
        body: &[Statement],
        until: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let body_bb = self.context.append_basic_block(function, "repeat_body");
        let cond_bb = self.context.append_basic_block(function, "repeat_cond");
        let end_bb = self.context.append_basic_block(function, "repeat_end");

        // Save and set loop_exit_bb for EXIT support, loop_continue_bb for CONTINUE.
        // CONTINUE in a REPEAT skips to the UNTIL test — the test still decides whether
        // another iteration runs, so the loop cannot be made to spin by it.
        let prev_exit_bb = self.loop_exit_bb;
        let prev_continue_bb = self.loop_continue_bb;
        self.loop_exit_bb = Some(end_bb);
        self.loop_continue_bb = Some(cond_bb);

        // Jump into body (do-while: body executes at least once)
        self.builder
            .build_unconditional_branch(body_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        // Body
        self.builder.position_at_end(body_bb);
        for stmt in body {
            self.compile_statement(stmt, function)?;
        }
        self.branch_to_join(cond_bb)?;

        // Condition check: if UNTIL condition is true, exit; otherwise loop back
        self.builder.position_at_end(cond_bb);
        let cond_val = self
            .compile_expression(until, function)?
            .ok_or_else(|| self.no_value_error("UNTIL condition", until))?;
        let cond_bool = self.to_i1(self.int_operand(cond_val, "a condition")?)?;
        // UNTIL means: exit when true, loop when false
        self.builder
            .build_conditional_branch(cond_bool, end_bb, body_bb)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        self.builder.position_at_end(end_bb);
        self.loop_exit_bb = prev_exit_bb;
        self.loop_continue_bb = prev_continue_bb;
        Ok(())
    }

    fn compile_case(
        &mut self,
        selector: &Expression,
        branches: &[CaseBranch],
        else_body: &Option<Vec<Statement>>,
        function: FunctionValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let sel_val = self
            .compile_expression(selector, function)?
            .ok_or_else(|| self.no_value_error("CASE selector", selector))?;

        let end_bb = self.context.append_basic_block(function, "case_end");
        let else_bb = if else_body.is_some() {
            self.context.append_basic_block(function, "case_else")
        } else {
            end_bb
        };

        // Test the labels branch by branch, first match wins. Each label is an
        // ordinary comparison with the selector (so it takes the selector's
        // signedness and width like any `=`), which is what lets a label be any
        // constant expression: `-1`, `INT#5`, `16#FF`, an enumerator (`Idle`,
        // `Mode#Idle`), a named CONSTANT. Only a bare integer literal used to be
        // understood — every other label was dropped without a diagnostic, so its
        // branch could never run — and a range became one switch case per value,
        // which for `0..2000000000` never finished. LLVM turns the chain back into a
        // switch where it can.
        let sel_ty = self.rvalue_iec_type(selector);
        // IEC and CODESYS: the selector is ANY_INT or an enumeration.
        if !sel_val.is_int_value() {
            self.int_operand(sel_val, "a CASE selector")?;
        }
        let mut branch_blocks = Vec::new();
        for (i, branch) in branches.iter().enumerate() {
            let bb = self
                .context
                .append_basic_block(function, &format!("case_{i}"));
            branch_blocks.push(bb);
            let mut cond: Option<inkwell::values::IntValue<'ctx>> = None;
            for label in &branch.labels {
                let cmp = |this: &mut Self,
                               e: &Expression,
                               op: BinaryOp|
                 -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
                    let v = this
                        .compile_expression(e, function)?
                        .ok_or_else(|| this.no_value_error("CASE label", e))?;
                    let vty = this.rvalue_iec_type(e);
                    let r = this.compile_binary_op(op, sel_val, sel_ty.as_ref(), v, vty.as_ref())?;
                    this.int_operand(r, "a CASE label comparison")
                };
                let hit = match label {
                    CaseLabel::Value(e) => cmp(self, e, BinaryOp::Equal)?,
                    CaseLabel::Range(lo, hi) => {
                        let ge = cmp(self, lo, BinaryOp::GreaterEqual)?;
                        let le = cmp(self, hi, BinaryOp::LessEqual)?;
                        self.builder
                            .build_and(ge, le, "in_range")
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                    }
                };
                cond = Some(match cond {
                    None => hit,
                    Some(c) => self
                        .builder
                        .build_or(c, hit, "case_any")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                });
            }
            let next = if i + 1 < branches.len() {
                self.context
                    .append_basic_block(function, &format!("case_test_{}", i + 1))
            } else {
                else_bb
            };
            match cond {
                Some(c) => {
                    self.builder
                        .build_conditional_branch(c, bb, next)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                }
                None => {
                    self.builder
                        .build_unconditional_branch(next)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                }
            }
            if i + 1 < branches.len() {
                self.builder.position_at_end(next);
            }
        }
        if branches.is_empty() {
            self.builder
                .build_unconditional_branch(else_bb)
                .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        }

        // Compile branch bodies
        for (i, branch) in branches.iter().enumerate() {
            self.builder.position_at_end(branch_blocks[i]);
            for stmt in &branch.body {
                self.compile_statement(stmt, function)?;
            }
            self.branch_to_join(end_bb)?;
        }

        // Else body
        if let Some(body) = else_body {
            self.builder.position_at_end(else_bb);
            for stmt in body {
                self.compile_statement(stmt, function)?;
            }
            self.branch_to_join(end_bb)?;
        }

        self.builder.position_at_end(end_bb);
        Ok(())
    }

    fn compile_lvalue_with_fn(
        &mut self,
        expr: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<Option<PointerValue<'ctx>>, CodegenError> {
        self.compile_lvalue_inner(expr, Some(function))
    }

    /// IEC type of an assignable expression, when it can be determined statically.
    ///
    /// Assignment stores the compiled RHS straight into the target pointer. With
    /// opaque pointers there is nothing to check the width against, so without this
    /// an `INT` expression assigned to a `TIME` (i64) field stored only two of the
    /// eight bytes and left the rest stale. `ET := 0;` was quietly broken.
    ///
    /// Returns `None` for anything not recognized, in which case the store happens
    /// unchanged — no behavior is *removed* by this, only widths corrected.
    fn lvalue_iec_type(&self, expr: &Expression) -> Option<IecType> {
        match &expr.kind {
            ExpressionKind::Identifier(ident) => self
                .variables
                .get(&ident.name.to_uppercase())
                .map(|(_, t)| t.clone()),
            ExpressionKind::Parenthesized(inner) => self.lvalue_iec_type(inner),
            ExpressionKind::DirectVariable(repr) => Self::direct_variable_type(repr),
            ExpressionKind::ArrayIndex { array, .. } => match self.lvalue_iec_type(array)? {
                IecType::Array { element_type, .. } => Some(*element_type),
                _ => None,
            },
            ExpressionKind::MemberAccess { object, member } => {
                if self.bit_access(expr).is_some() {
                    return Some(IecType::Bool);
                }
                // STRUCT field or FB-instance field, at any depth: `s.i.v` and
                // `x.f.o` both ask the type of the object first.
                // The *declared* type: a VAR_IN_OUT field is stored as a pointer, but
                // `f.t` denotes the DINT it refers to, not the pointer word.
                let fields = self.member_fields_of(object)?;
                fields
                    .iter()
                    .find(|f| f.name.eq_ignore_ascii_case(&member.name))
                    .map(|f| f.declared.clone())
            }
            // `pt^` denotes the pointee, so `pt^[i]`, `pt^.f` and `pt^ := x` all
            // resolve through the pointer's declared base type.
            ExpressionKind::Dereference(inner) => match self.rvalue_iec_type(inner)? {
                IecType::Pointer(base) => Some(*base),
                _ => None,
            },
            _ => None,
        }
    }

    /// Ordered members of whatever `object` denotes, if it has any.
    ///
    /// A STRUCT and an FB/CLASS instance are laid out the same way — an ordered field
    /// list — and `.member` reaches into either identically, so both answer here.
    /// Resolving this from the object's *type* rather than from its spelling is what
    /// lets an FB instance live in a STRUCT field, an array element or a VAR_GLOBAL:
    /// matching only a bare `Identifier` against a per-POU instance map meant
    /// `n := x.f.o;` silently produced nothing.
    fn member_fields_of(&self, object: &Expression) -> Option<Vec<PouField>> {
        match self.lvalue_iec_type(object)? {
            IecType::Struct { fields, .. } => Some(
                fields
                    .into_iter()
                    .map(|(n, t)| PouField::plain(n, t))
                    .collect(),
            ),
            IecType::FbInstance(name) => self
                .compiled_fbs
                .get(&name.to_uppercase())
                .map(|l| l.fields.clone()),
            _ => None,
        }
    }

    /// The compiled layout of the FB/CLASS instance `expr` denotes, if it is one.
    ///
    /// Pure — it inspects types only, so it is safe to use as the "is this an FB
    /// call?" test before any code is emitted.
    fn fb_layout_of(&self, expr: &Expression) -> Option<FbLayout<'ctx>> {
        match self.lvalue_iec_type(expr)? {
            IecType::FbInstance(name) => self.compiled_fbs.get(&name.to_uppercase()).cloned(),
            _ => None,
        }
    }

    /// IEC type of a *value-producing* expression, when it can be determined statically.
    ///
    /// Only used to decide whether widening the value is a zero-extension or a
    /// sign-extension, so it is deliberately conservative: `None` means "no static
    /// type", and the caller falls back to the destination's signedness.
    fn rvalue_iec_type(&self, expr: &Expression) -> Option<IecType> {
        if let Some(t) = self.string_expr_type(expr) {
            return Some(t);
        }
        if let Ok(Some((_, ty))) = self.enum_constant_of(expr) {
            return Some(ty);
        }
        match &expr.kind {
            ExpressionKind::Identifier(_)
            | ExpressionKind::ArrayIndex { .. }
            | ExpressionKind::MemberAccess { .. }
            | ExpressionKind::DirectVariable(_)
            | ExpressionKind::Dereference(_) => self.lvalue_iec_type(expr),
            ExpressionKind::Parenthesized(inner) => self.rvalue_iec_type(inner),
            ExpressionKind::BoolLiteral(_) => Some(IecType::Bool),
            // `DWORD#1` is a DWORD: unsigned, whatever the digits look like.
            ExpressionKind::TypedLiteral { type_name, .. } => {
                plcc_hir::types::resolve_type_name(&type_name.name)
            }
            // Negation only makes sense on a signed value, and the result is signed
            // regardless of what went in. NOT preserves its operand's type.
            ExpressionKind::UnaryOp { op, operand } => match op {
                UnaryOp::Neg => None,
                UnaryOp::Not => self.rvalue_iec_type(operand),
            },
            ExpressionKind::BinaryOp { op, left, right } => match op {
                // Comparisons yield BOOL whatever the operands were. Getting this
                // wrong would sign-extend an i1 `1` into 16#FF.
                BinaryOp::Equal
                | BinaryOp::NotEqual
                | BinaryOp::Less
                | BinaryOp::LessEqual
                | BinaryOp::Greater
                | BinaryOp::GreaterEqual
                | BinaryOp::AndThen
                | BinaryOp::OrElse => Some(IecType::Bool),
                BinaryOp::Power => Some(self.expt_result_type(left, right)),
                // The wider operand's type governs — except when the two disagree
                // about signedness, where the operator runs in a wider signed type
                // and the result has to say so. A literal operand contributes nothing,
                // so the typed side wins.
                _ => match (self.rvalue_iec_type(left), self.rvalue_iec_type(right)) {
                    (Some(l), Some(r)) => Self::arith_result_type(Some(l), Some(r)),
                    (Some(t), None) | (None, Some(t)) => Some(t),
                    (None, None) => None,
                },
            },
            // The builtins whose result type is one of their arguments' types. Saying
            // so keeps the result unsigned on the way out: `SHL(b, 1)` with
            // `b : BYTE := 254` is 16#FC, and calling that an untyped value
            // sign-extended it to -4 in any wider destination — the same for
            // `MAX(b, 100)` = 200.
            ExpressionKind::FunctionCall { callee, args } => {
                if let Some(method) = self.implicit_method_call(callee) {
                    let call = Expression {
                        kind: ExpressionKind::FunctionCall {
                            callee: Box::new(method),
                            args: args.clone(),
                        },
                        span: expr.span,
                    };
                    return self.rvalue_iec_type(&call);
                }
                if let Some(d) = self.desugar_std_call(expr) {
                    return self.rvalue_iec_type(&d);
                }
                if self.is_mux_call(expr) {
                    return self.mux_result_type(expr);
                }
                // `obj.Method(..)`: the method's declared result type.
                if let ExpressionKind::MemberAccess { object, member } = &callee.kind {
                    if self.interface_of_expr(object).is_some() {
                        return self.interface_method_type(object, &member.name);
                    }
                    return self
                        .fb_layout_of(object)?
                        .methods
                        .get(&member.name.to_uppercase())
                        .map(|m| m.return_type.clone())
                        .filter(|t| *t != IecType::Void);
                }
                let ExpressionKind::Identifier(name) = &callee.kind else {
                    return None;
                };
                let arg_ty = |i: usize| args.get(i).and_then(|a| self.rvalue_iec_type(&a.value));
                // A FUNCTION of this unit takes precedence over a builtin name.
                if let Some(t) = self.fn_return_types.get(&name.name.to_lowercase()) {
                    return Some(t.clone());
                }
                match name.name.to_uppercase().as_str() {
                    // Result is the type of IN; N is only a count.
                    "SHL" | "SHR" | "ROL" | "ROR" => arg_ty(0),
                    // ABS never changes its argument's type.
                    "ABS" => arg_ty(0),
                    "TRUNC" => Some(IecType::Dint),
                    "EXPT" if args.len() == 2 => {
                        Some(self.expt_result_type(&args[0].value, &args[1].value))
                    }
                    // The address of the argument, typed as a pointer to it.
                    "__ISVALIDREF" => Some(IecType::Bool),
                    "ADR" | "__REF_OF" if self.module.get_function("adr").is_none() => args
                        .first()
                        .and_then(|a| self.lvalue_iec_type(&a.value))
                        .map(|t| IecType::Pointer(Box::new(t))),
                    "SIZEOF" if self.module.get_function("sizeof").is_none() => {
                        Some(IecType::Udint)
                    }
                    // MIN/MAX return one of their operands, so the result type is the
                    // one the comparison was performed in.
                    "MIN" | "MAX" => Self::arith_result_type(arg_ty(0), arg_ty(1)),
                    "SEL" => Self::arith_result_type(arg_ty(1), arg_ty(2)),
                    // LIMIT(MN, IN, MX) likewise, over all three.
                    "LIMIT" => Self::arith_result_type(
                        Self::arith_result_type(arg_ty(0), arg_ty(1)),
                        arg_ty(2),
                    ),
                    n if Self::parse_conversion(n).is_some() => {
                        Self::parse_conversion(n).map(|(_, dst)| dst)
                    }
                    n if Self::is_datetime_function(n) => Self::datetime_result_type(n),
                    n if Self::string_to_target(n).is_some() => Self::string_to_target(n),
                    // A user FUNCTION's declared result type. Without it
                    // `NOT MY_BYTE_FN()` could not be told from a BOOL NOT.
                    _ => self.fn_return_types.get(&name.name.to_lowercase()).cloned(),
                }
            }
            _ => None,
        }
    }

    /// Render an assignable expression back into something a user recognizes, for
    /// diagnostics. Indices are elided — the point is to name the construct.
    fn describe_lvalue(expr: &Expression) -> String {
        match &expr.kind {
            ExpressionKind::Identifier(i) => i.name.clone(),
            ExpressionKind::Parenthesized(inner) => Self::describe_lvalue(inner),
            ExpressionKind::ArrayIndex { array, .. } => {
                format!("{}[..]", Self::describe_lvalue(array))
            }
            ExpressionKind::MemberAccess { object, member } => {
                format!("{}.{}", Self::describe_lvalue(object), member.name)
            }
            ExpressionKind::FunctionCall { callee, .. } => {
                format!("{}(..)", Self::describe_lvalue(callee))
            }
            ExpressionKind::Dereference(inner) => format!("{}^", Self::describe_lvalue(inner)),
            ExpressionKind::DirectVariable(addr) => format!("%{addr}"),
            ExpressionKind::StringLiteral(s) | ExpressionKind::WstringLiteral(s) => {
                format!("'{s}'")
            }
            ExpressionKind::IntegerLiteral(v) => v.to_string(),
            ExpressionKind::RealLiteral(v) => v.to_string(),
            ExpressionKind::BoolLiteral(v) => {
                if *v {
                    "TRUE".into()
                } else {
                    "FALSE".into()
                }
            }
            ExpressionKind::TimeLiteral(s)
            | ExpressionKind::DateLiteral(s)
            | ExpressionKind::TodLiteral(s)
            | ExpressionKind::DtLiteral(s) => s.clone(),
            ExpressionKind::TypedLiteral { type_name, value } => {
                format!("{}#{}", type_name.name, Self::describe_lvalue(value))
            }
            ExpressionKind::BinaryOp { .. } | ExpressionKind::UnaryOp { .. } => {
                "an arithmetic expression".into()
            }
            ExpressionKind::ArrayInitializer(_) => "an array initializer".into(),
            ExpressionKind::StructInitializer(_) => "a structure initializer".into(),
        }
    }

    /// The diagnostic for an expression that `compile_expression` returned `Ok(None)`
    /// for, in a position that needs a value. `what` names the position ("argument 2
    /// of `MIN`", "IF condition").
    ///
    /// The reason comes from the leaf that `compile_expression` recorded when it
    /// returned `Ok(None)`, provided that leaf lies inside `expr` — so a stale cause
    /// from some earlier, tolerated `None` is never attached to an unrelated
    /// expression. Failing that, it is reconstructed from the expression's shape.
    fn no_value_error(&mut self, what: impl Into<String>, expr: &Expression) -> CodegenError {
        let recorded = self.no_value_cause.take().and_then(|(span, reason)| {
            (span.start >= expr.span.start && span.end <= expr.span.end).then_some(reason)
        });
        let reason = recorded
            .or_else(|| self.no_value_reason(expr))
            .unwrap_or_else(|| "codegen does not lower this expression to a value".to_string());
        CodegenError::NoValue {
            what: what.into(),
            expr: Self::describe_lvalue(expr),
            reason,
            offset: expr.span.start,
        }
    }

    /// Return `Ok(None)` from a `compile_expression` leaf, recording why.
    fn no_value(
        &mut self,
        expr: &Expression,
        reason: String,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        self.no_value_cause = Some((expr.span, reason));
        Ok(None)
    }

    /// `no_value` with the reason reconstructed from `expr`'s shape.
    fn no_value_here(
        &mut self,
        expr: &Expression,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let reason = self
            .no_value_reason(expr)
            .unwrap_or_else(|| "codegen does not lower this expression to a value".to_string());
        self.no_value(expr, reason)
    }

    /// Why `expr` yields no value, from its shape alone. This mirrors the `Ok(None)`
    /// exits of `compile_expression` without emitting any code; `None` means no
    /// specific cause was identified here.
    ///
    /// Exact at a leaf (where `compile_expression` already knows the leaf itself is
    /// at fault). On a composite tree it is only a fallback: it cannot tell a builtin
    /// call that compiled from an unknown function, since builtins have no module
    /// declaration — which is why the leaf's recorded cause is preferred.
    fn no_value_reason(&self, expr: &Expression) -> Option<String> {
        match &expr.kind {
            ExpressionKind::IntegerLiteral(_)
            | ExpressionKind::RealLiteral(_)
            | ExpressionKind::BoolLiteral(_)
            | ExpressionKind::TimeLiteral(_)
            | ExpressionKind::DateLiteral(_)
            | ExpressionKind::TodLiteral(_)
            | ExpressionKind::DtLiteral(_) => None,
            ExpressionKind::Identifier(_) => self.unknown_root(expr),
            ExpressionKind::Parenthesized(inner) => self.no_value_reason(inner),
            ExpressionKind::BinaryOp { left, right, .. } => self
                .no_value_reason(left)
                .or_else(|| self.no_value_reason(right)),
            ExpressionKind::UnaryOp { operand, .. } => self.no_value_reason(operand),
            ExpressionKind::FunctionCall { callee, .. } => match &callee.kind {
                ExpressionKind::Identifier(ident) => {
                    let uname = ident.name.to_uppercase();
                    if matches!(uname.as_str(), "CONCAT" | "LEFT" | "RIGHT" | "MID") {
                        return Some(format!(
                            "`{uname}` returns a STRING, which is only supported as the \
                             whole right-hand side of an assignment"
                        ));
                    }
                    match self.module.get_function(&ident.name.to_lowercase()) {
                        None => Some(format!(
                            "unknown function `{}` (no builtin and no FUNCTION of that \
                             name in this compilation unit)",
                            ident.name
                        )),
                        Some(f) if f.get_type().get_return_type().is_none() => {
                            Some(format!("function `{}` returns no value", ident.name))
                        }
                        Some(_) => None,
                    }
                }
                ExpressionKind::MemberAccess { object, member } => {
                    self.unknown_root(object).or_else(|| {
                        Some(format!(
                            "`{}` is not a FUNCTION_BLOCK or CLASS instance, so `.{}(..)` \
                             is not a method call",
                            Self::describe_lvalue(object),
                            member.name
                        ))
                    })
                }
                _ => Some(format!(
                    "`{}` is not callable",
                    Self::describe_lvalue(callee)
                )),
            },
            ExpressionKind::MemberAccess { .. } | ExpressionKind::ArrayIndex { .. } => {
                self.unknown_root(expr).or_else(|| {
                    self.lvalue_iec_type(expr).is_none().then(|| {
                        format!(
                            "`{}` does not resolve to a field or element of known type",
                            Self::describe_lvalue(expr)
                        )
                    })
                })
            }
            ExpressionKind::StringLiteral(_) | ExpressionKind::WstringLiteral(_) => {
                Some("a string literal has no scalar value in this position".to_string())
            }
            ExpressionKind::DirectVariable(addr) => Some(format!(
                "direct variable `%{addr}` is not supported in expressions"
            )),
            ExpressionKind::Dereference(inner) => self.unknown_root(expr).or_else(|| {
                Some(format!(
                    "`{}` is not a POINTER of known base type, so `{}` has no type to load",
                    Self::describe_lvalue(inner),
                    Self::describe_lvalue(expr)
                ))
            }),
            ExpressionKind::TypedLiteral { .. } => Some(format!(
                "typed literal `{}` is not supported in this position",
                Self::describe_lvalue(expr)
            )),
            ExpressionKind::ArrayInitializer(_) | ExpressionKind::StructInitializer(_) => {
                Some("an array or structure initializer is only an initial value".to_string())
            }
        }
    }

    /// For an access chain (`a.b[i].c`, `p^.x`), report its root identifier when that
    /// identifier is not a variable in scope.
    fn unknown_root(&self, expr: &Expression) -> Option<String> {
        match &expr.kind {
            ExpressionKind::Identifier(ident) => {
                if self.variables.contains_key(&ident.name.to_uppercase()) {
                    None
                } else {
                    Some(format!("unknown identifier `{}`", ident.name))
                }
            }
            ExpressionKind::MemberAccess { object, .. } => self.unknown_root(object),
            ExpressionKind::ArrayIndex { array, .. } => self.unknown_root(array),
            ExpressionKind::Parenthesized(inner) | ExpressionKind::Dereference(inner) => {
                self.unknown_root(inner)
            }
            _ => None,
        }
    }

    /// Pointer to an assignable location.
    ///
    /// `Ok(None)` means "this expression has no address", and callers that *ask*
    /// speculatively (the string builtins, which accept either a variable or a
    /// literal) rely on that. It must never mean "recognized the construct and gave
    /// up": returning `Ok(None)` from a recognized construct is how
    /// `s.i.v := 7;` and `o[1][2].a := 7;` came to emit no code at all. Arms that
    /// recognize a construct and cannot lower it raise a `CodegenError` naming it.
    ///
    /// The assignment statement path turns a `None` target into a diagnostic, so the
    /// remaining unaddressable cases are reported rather than dropped.
    fn compile_lvalue_inner(
        &mut self,
        expr: &Expression,
        function: Option<FunctionValue<'ctx>>,
    ) -> Result<Option<PointerValue<'ctx>>, CodegenError> {
        match &expr.kind {
            ExpressionKind::Identifier(ident) => {
                let key = ident.name.to_uppercase();
                if self.bit_binding(&key).is_some() {
                    return Err(self.bit_address_error(&ident.name));
                }
                Ok(self.variables.get(&key).map(|(ptr, _)| *ptr))
            }
            ExpressionKind::Parenthesized(inner) => self.compile_lvalue_inner(inner, function),
            ExpressionKind::ArrayIndex { array, indices } => {
                // The indexed thing is resolved as an lvalue in its own right, so an
                // index can sit on top of any chain: `o[1][2]`, `s.arr[3]`,
                // `a[1].b[2]`. Matching only a bare identifier here dropped every
                // such chain silently — `o[1][2].a := 7;` and `n := o[1][2].a;` both
                // emitted no code at all, with no diagnostic.
                let Some(iec_ty) = self.lvalue_iec_type(array) else {
                    return Err(CodegenError::UnsupportedType(format!(
                        "cannot determine the type of `{}`, the array indexed in `{}`",
                        Self::describe_lvalue(array),
                        Self::describe_lvalue(expr)
                    )));
                };
                let IecType::Array { ref ranges, .. } = iec_ty else {
                    return Err(CodegenError::UnsupportedType(format!(
                        "array indexing on non-array type: {iec_ty}"
                    )));
                };
                let ranges = ranges.clone();
                let function = function.ok_or_else(|| {
                    CodegenError::LlvmError(
                        "array index in lvalue requires function context".into(),
                    )
                })?;
                let Some(arr_ptr) = self.compile_lvalue_inner(array, Some(function))? else {
                    return Err(CodegenError::UnsupportedType(format!(
                        "`{}` has no address, so `{}` cannot be indexed",
                        Self::describe_lvalue(array),
                        Self::describe_lvalue(expr)
                    )));
                };
                let arr_llvm_ty = self.iec_to_llvm_type(&iec_ty);

                if indices.len() != ranges.len() {
                    return Err(CodegenError::UnsupportedType(format!(
                        "`{}` has {} dimension(s) but is indexed with {}",
                        Self::describe_lvalue(array),
                        ranges.len(),
                        indices.len()
                    )));
                }
                // Row-major linear index, in i64. Each subscript widens by its
                // *own* signedness (a BYTE index of 200 is 200, not -56 — which
                // wrote 56 elements before the array) and is clamped into its
                // dimension's bounds, so an out-of-range subscript can never
                // address memory outside the array (an out-of-bounds `inbounds`
                // GEP is also undefined behaviour to the optimizer).
                let i64t = self.context.i64_type();
                let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
                let mut linear_idx = i64t.const_zero();
                for (dim, idx_expr) in indices.iter().enumerate() {
                    let idx_val = self.compile_expression(idx_expr, function)?.ok_or_else(|| {
                        self.no_value_error(
                            format!("index {} of `{}`", dim + 1, Self::describe_lvalue(expr)),
                            idx_expr,
                        )
                    })?;
                    let idx_int = self.int_operand(idx_val, "an array index")?;
                    let idx_ty = self.rvalue_iec_type(idx_expr);
                    let idx = self.widen_to(idx_int, Self::signedness_of(idx_ty.as_ref()), i64t)?;
                    let (lo, hi) = ranges[dim];
                    let adjusted = self
                        .builder
                        .build_int_sub(idx, i64t.const_int(lo as u64, true), "adj_idx")
                        .map_err(err)?;
                    let last = i64t.const_int((hi - lo).max(0) as u64, true);
                    let below = self
                        .builder
                        .build_int_compare(IntPredicate::SLT, adjusted, i64t.const_zero(), "idx_lo")
                        .map_err(err)?;
                    let above = self
                        .builder
                        .build_int_compare(IntPredicate::SGT, adjusted, last, "idx_hi")
                        .map_err(err)?;
                    // Where the dialect faults on a bad subscript (Logix), the
                        // clamp below only runs on the not-faulted path.
                    if self.faults_on_bounds() {
                        let out = self.builder.build_or(below, above, "idx_out").map_err(err)?;
                        let in_range = out.is_const() && out.get_zero_extended_constant() == Some(0);
                        if !in_range {
                            self.fault.span.set(Some(idx_expr.span));
                            self.fault_if(out, plcc_runtime::fault::FaultCode::ArrayBounds)?;
                            self.fault.span.set(None);
                        }
                    }
                    let clamped = self
                        .builder
                        .build_select(below, i64t.const_zero(), adjusted, "idx_clamp_lo")
                        .map_err(err)?
                        .into_int_value();
                    let clamped = self
                        .builder
                        .build_select(above, last, clamped, "idx_clamp")
                        .map_err(err)?
                        .into_int_value();
                    let stride: i64 = ranges[dim + 1..]
                        .iter()
                        .map(|(l, h)| (h - l + 1).max(0))
                        .product();
                    let component = self
                        .builder
                        .build_int_mul(clamped, i64t.const_int(stride as u64, true), "dim_component")
                        .map_err(err)?;
                    linear_idx = self
                        .builder
                        .build_int_add(linear_idx, component, "linear_idx")
                        .map_err(err)?;
                }
                let elem_ptr = unsafe {
                    self.builder
                        .build_in_bounds_gep(
                            arr_llvm_ty,
                            arr_ptr,
                            &[i64t.const_zero(), linear_idx],
                            "arr_elem",
                        )
                        .map_err(err)?
                };
                Ok(Some(elem_ptr))
            }
            ExpressionKind::MemberAccess { object, member } => {
                // STRUCT field or FB-instance field, at any depth. The object is
                // resolved as an lvalue in its own right, so `s.i.v` GEPs through
                // `s.i` and `x.f.o` GEPs through `x.f` — matching only a bare
                // identifier here dropped every chain longer than one level, silently:
                // `s.i.v := 7;` emitted nothing at all.
                let obj_ty = self.lvalue_iec_type(object).ok_or_else(|| {
                    CodegenError::UnsupportedType(format!(
                        "cannot determine the type of `{}`, whose member is taken in `{}`",
                        Self::describe_lvalue(object),
                        Self::describe_lvalue(expr)
                    ))
                })?;
                let Some(fields) = self.member_fields_of(object) else {
                    return Err(CodegenError::UnsupportedType(format!(
                        "`{}` is {obj_ty}, which has no member `{}`",
                        Self::describe_lvalue(object),
                        member.name
                    )));
                };
                let field_idx = fields
                    .iter()
                    .position(|f| f.name.eq_ignore_ascii_case(&member.name))
                    .ok_or_else(|| CodegenError::UndefinedVariable(member.name.clone()))?;
                let Some(obj_ptr) = self.compile_lvalue_inner(object, function)? else {
                    return Err(CodegenError::UnsupportedType(format!(
                        "`{}` has no address, so `{}` cannot be reached",
                        Self::describe_lvalue(object),
                        Self::describe_lvalue(expr)
                    )));
                };
                if let Some(addr) = fields[field_idx].at {
                    // An AT member is the image location, shared by every instance.
                    if addr.bit.is_some() {
                        return Err(self.bit_address_error(&Self::describe_lvalue(expr)));
                    }
                    return Ok(Some(self.image_ptr(&addr)));
                }
                let struct_llvm_ty = self.iec_to_llvm_type(&obj_ty);
                let field_ptr = self
                    .builder
                    .build_struct_gep(
                        struct_llvm_ty.into_struct_type(),
                        obj_ptr,
                        field_idx as u32,
                        &member.name,
                    )
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                if fields[field_idx].is_in_out {
                    // The slot holds the caller's address. `f.t` from outside the FB
                    // must reach the referenced variable, not the pointer word.
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let target = self
                        .builder
                        .build_load(ptr_ty, field_ptr, &member.name)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    return Ok(Some(target.into_pointer_value()));
                }
                Ok(Some(field_ptr))
            }
            // `pt^`: the address is the pointer's value. A pointer held in an
            // integer (CODESYS lets an address live in a DWORD) is converted.
            ExpressionKind::Dereference(inner) => {
                let function = function.ok_or_else(|| {
                    CodegenError::LlvmError("pointer dereference requires function context".into())
                })?;
                let Some(v) = self.compile_expression(inner, function)? else {
                    return Err(self.no_value_error(
                        format!("the pointer dereferenced in `{}`", Self::describe_lvalue(expr)),
                        inner,
                    ));
                };
                match v {
                    BasicValueEnum::PointerValue(p) => Ok(Some(p)),
                    BasicValueEnum::IntValue(iv) => {
                        let iv = self.resize_int(iv, self.addr_int_type(), false)?;
                        let p = self
                            .builder
                            .build_int_to_ptr(
                                iv,
                                self.context.ptr_type(AddressSpace::default()),
                                "deref_addr",
                            )
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        Ok(Some(p))
                    }
                    _ => Err(CodegenError::UnsupportedType(format!(
                        "`{}` dereferences `{}`, which is not a POINTER",
                        Self::describe_lvalue(expr),
                        Self::describe_lvalue(inner)
                    ))),
                }
            }
            // Literals, calls, and direct representation (%IX0.0) have no address.
            // This is the one arm where `None` is the honest answer, and the string
            // builtins depend on it to accept a literal where a variable would also
            // do. Assignment reports it — see `compile_statement`.
            _ => Ok(None),
        }
    }

    fn compile_expression(
        &mut self,
        expr: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        if let Some((v, ty)) = self.enum_constant_of(expr)? {
            return Ok(Some(self.enum_value_const(v, &ty)));
        }
        match &expr.kind {
            // An integer literal needs 64 bits at most (LINT or ULINT); a larger
            // one was truncated to its low 64 bits without a word.
            ExpressionKind::IntegerLiteral(v) => {
                if !(i64::MIN as i128..=u64::MAX as i128).contains(v) {
                    return Err(CodegenError::UnsupportedType(format!(
                        "integer literal {v} does not fit in 64 bits (source offset {})",
                        expr.span.start
                    )));
                }
                Ok(Some(self.int_literal(*v).into()))
            }
            // An untyped real literal is held as an LREAL and takes the type its
            // context needs (IEC 61131-3 §6.3.3): stored into a REAL it is rounded
            // once, combined with a REAL it becomes a REAL (`compile_binary_op`).
            // Holding it as a REAL made `lr := 0.1` 0.10000000149 and
            // `lr := 1.0E300` +inf.
            ExpressionKind::RealLiteral(v) => {
                Ok(Some(self.context.f64_type().const_float(*v).into()))
            }
            ExpressionKind::BoolLiteral(v) => Ok(Some(
                self.context.i8_type().const_int(*v as u64, false).into(),
            )),
            ExpressionKind::Identifier(ident) => {
                if let Some((ptr, bit)) = self.bit_binding(&ident.name.to_uppercase()) {
                    return self.load_bit(ptr, bit).map(Some);
                }
                if let Some((ptr, ty)) = self.variables.get(&ident.name.to_uppercase()).cloned() {
                    let llvm_ty = self.iec_to_llvm_type(&ty);
                    let val = self
                        .builder
                        .build_load(llvm_ty, ptr, &ident.name)
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(Some(val))
                } else {
                    self.no_value_here(expr)
                }
            }
            ExpressionKind::BinaryOp { op, left, right } => {
                if matches!(op, BinaryOp::AndThen | BinaryOp::OrElse) {
                    return self
                        .compile_short_circuit(*op, left, right, function)
                        .map(Some);
                }
                // `t + 1000`, `t > 500` with t a TIME: the literal is milliseconds.
                if matches!(
                    op,
                    BinaryOp::Add
                        | BinaryOp::Sub
                        | BinaryOp::Equal
                        | BinaryOp::NotEqual
                        | BinaryOp::Less
                        | BinaryOp::LessEqual
                        | BinaryOp::Greater
                        | BinaryOp::GreaterEqual
                ) {
                    let lt = self.rvalue_iec_type(left);
                    let rt = self.rvalue_iec_type(right);
                    let scaled = |ty: &Option<IecType>, lit: &Expression| {
                        ty.as_ref().and_then(|t| Self::temporal_literal_ns(lit, t)).map(|ns| (ns, t_of(ty)))
                    };
                    fn t_of(t: &Option<IecType>) -> IecType {
                        t.clone().unwrap_or(IecType::Time)
                    }
                    if let Some((ns, t)) = scaled(&lt, right) {
                        let l = self
                            .compile_expression(left, function)?
                            .ok_or_else(|| self.no_value_error("operand", left))?;
                        let r = self.context.i64_type().const_int(ns as u64, true).into();
                        return self
                            .compile_binary_op(*op, l, lt.as_ref(), r, Some(&t))
                            .map(Some);
                    }
                    if let Some((ns, t)) = scaled(&rt, left) {
                        let r = self
                            .compile_expression(right, function)?
                            .ok_or_else(|| self.no_value_error("operand", right))?;
                        let l = self.context.i64_type().const_int(ns as u64, true).into();
                        return self
                            .compile_binary_op(*op, l, Some(&t), r, rt.as_ref())
                            .map(Some);
                    }
                }
                if self.interface_of_expr(left).is_some() || self.interface_of_expr(right).is_some() {
                    if let Some(v) = self.compile_interface_compare(*op, left, right, function)? {
                        return Ok(Some(v));
                    }
                }
                if matches!(
                    op,
                    BinaryOp::Equal
                        | BinaryOp::NotEqual
                        | BinaryOp::Less
                        | BinaryOp::LessEqual
                        | BinaryOp::Greater
                        | BinaryOp::GreaterEqual
                ) && (self.is_string_expr(left) || self.is_string_expr(right))
                {
                    return self
                        .compile_string_compare(*op, left, right, function)
                        .map(Some);
                }
                let lhs = self.compile_expression(left, function)?;
                let rhs = self.compile_expression(right, function)?;
                match (lhs, rhs) {
                    (Some(l), Some(r)) => {
                        let l_ty = self.rvalue_iec_type(left);
                        let r_ty = self.rvalue_iec_type(right);
                        self.fault.span.set(Some(expr.span));
                        let result =
                            self.compile_binary_op(*op, l, l_ty.as_ref(), r, r_ty.as_ref());
                        self.fault.span.set(None);
                        let result = result?;
                        Ok(Some(result))
                    }
                    _ => Ok(None),
                }
            }
            ExpressionKind::UnaryOp { op, operand } => {
                let val = self.compile_expression(operand, function)?;
                match val {
                    Some(v) => {
                        let operand_ty = self.rvalue_iec_type(operand);
                        let result = self.compile_unary_op(*op, v, operand_ty.as_ref())?;
                        Ok(Some(result))
                    }
                    None => Ok(None),
                }
            }
            ExpressionKind::Parenthesized(inner) => self.compile_expression(inner, function),
            ExpressionKind::FunctionCall { callee, args } => {
                if let Some(method) = self.implicit_method_call(callee) {
                    let call = Expression {
                        kind: ExpressionKind::FunctionCall {
                            callee: Box::new(method),
                            args: args.clone(),
                        },
                        span: expr.span,
                    };
                    return self.compile_expression(&call, function);
                }
                if let Some(d) = self.desugar_std_call(expr) {
                    return self.compile_expression(&d, function);
                }
                if self.is_mux_call(expr) {
                    return self.compile_mux(expr, function);
                }
                if let ExpressionKind::Identifier(ident) = &callee.kind {
                    // ADR and SIZEOF take their argument as a *location* or a type,
                    // not a value, so they cannot go through the value-evaluating
                    // builtin path. A FUNCTION of the same name in the unit wins.
                    let lname = ident.name.to_lowercase();
                    if lname == "adr" && self.module.get_function("adr").is_none() {
                        return self.compile_adr(args, function).map(Some);
                    }
                    // `r REF= x` (see refs.rs).
                    if lname == "__ref_of" {
                        return self.compile_adr(args, function).map(Some);
                    }
                    // `__ISVALIDREF(r)`: the reference is bound (not NULL).
                    if lname == "__isvalidref" && args.len() == 1 {
                        let Some(v) = self.compile_expression(&args[0].value, function)? else {
                            return Err(self.no_value_error("argument of `__ISVALIDREF`", &args[0].value));
                        };
                        let iv = self.int_operand(v, "__ISVALIDREF")?;
                        let nz = self
                            .builder
                            .build_int_compare(IntPredicate::NE, iv, iv.get_type().const_zero(), "isvalid")
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        return Ok(Some(
                            self.builder
                                .build_int_z_extend(nz, self.context.i8_type(), "isvalid8")
                                .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                                .into(),
                        ));
                    }
                    if lname == "sizeof" && self.module.get_function("sizeof").is_none() {
                        return self.compile_sizeof(args).map(Some);
                    }
                    // A FUNCTION declared in this unit wins over a builtin of the same
                    // name. Trying the builtin first was wrong twice over: a non-IEC
                    // builtin (FLOOR) shadowed the program's own FUNCTION, and the
                    // builtin dispatcher compiles every argument before it knows
                    // whether it handles the name, so a user FUNCTION's arguments were
                    // emitted twice and a side-effecting argument ran twice.
                    let user_fn = self.fn_signatures.contains_key(&lname);
                    if !user_fn && Self::is_string_builtin(&lname) {
                        return self.compile_string_value(expr, function);
                    }
                    if !user_fn {
                        if let Some(result) =
                            self.compile_stdlib_call(&ident.name, args, function)?
                        {
                            return Ok(Some(result));
                        }
                    }

                    // Fall back to user-defined functions
                    if let Some(fn_val) = self.module.get_function(&ident.name.to_lowercase()) {
                        let signature = self.fn_signatures.get(&ident.name.to_lowercase()).cloned();
                        let params = signature.clone().unwrap_or_default();
                        // Named arguments bind by name, not by position — see
                        // `bind_args`. Only a call that names nothing keeps the source
                        // order. A callee with no recorded signature (not a FUNCTION in
                        // this unit) has no parameter names to bind against, so it stays
                        // positional rather than being rejected for naming them.
                        let ordered: Vec<Option<&CallArg>> = match &signature {
                            Some(params) => Self::bind_args(&ident.name, params, args)?,
                            None => args.iter().map(Some).collect(),
                        };
                        let mut compiled_args = Vec::new();
                        let mut outputs = Vec::new();
                        for (i, bound) in ordered.iter().enumerate() {
                            let param = params.get(i);
                            // A VAR_OUTPUT parameter points at a temporary that is
                            // copied to its `=>` target after the call.
                            if let Some(p) = param.filter(|p| p.is_output) {
                                let llvm_ty = self.iec_to_llvm_type(&p.ty);
                                let tmp = self.entry_alloca(function, llvm_ty, &p.name)?;
                                if let Some(a) = bound {
                                    outputs.push((*a, tmp, p.ty.clone()));
                                }
                                compiled_args.push(tmp.into());
                                continue;
                            }
                            let Some(bound) = bound else {
                                return Err(CodegenError::ArgumentBinding {
                                    callee: ident.name.clone(),
                                    problem: format!("no value for argument {}", i + 1),
                                });
                            };
                            let arg = &bound.value;
                            // A VAR_IN_OUT parameter takes the caller's address.
                            if param.is_some_and(|p| p.is_in_out) {
                                let ptr = self.compile_argument_reference(
                                    arg,
                                    &ident.name,
                                    &param.expect("checked").name,
                                    function,
                                )?;
                                compiled_args.push(ptr.into());
                                continue;
                            }
                            // Coerced to the declared parameter type inside: passing the
                            // value through unconverted produces a call whose argument
                            // width does not match the signature, which LLVM's verifier
                            // rejects outright.
                            let val = self.compile_call_arg(
                                arg,
                                param.map(|p| &p.ty),
                                &format!("argument {} of `{}`", i + 1, ident.name),
                                function,
                            )?;
                            compiled_args.push(val.into());
                        }
                        let call = self
                            .builder
                            .build_call(fn_val, &compiled_args, "call")
                            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                        for (a, tmp, ty) in outputs {
                            self.copy_output(a, tmp, &ty, function)?;
                        }
                        match call.try_as_basic_value() {
                            inkwell::values::ValueKind::Basic(v) => Ok(Some(v)),
                            inkwell::values::ValueKind::Instruction(_) => self.no_value_here(expr),
                        }
                    } else {
                        self.no_value_here(expr)
                    }
                } else if let ExpressionKind::MemberAccess { object, member } = &callee.kind {
                    // Method call: obj.Method(args), where `obj` is any addressable
                    // FB/CLASS instance — including one held in a STRUCT field.
                    if self.fb_layout_of(object).is_some() {
                        return self.compile_method_call(object, &member.name, args, function);
                    }
                    if self.interface_of_expr(object).is_some() {
                        return self.compile_interface_call(object, &member.name, args, function);
                    }
                    // `fb_layout_of` above resolves the instance by type from any
                    // addressable expression — `a[1].Bump(5)`, `s.parts[2].Reset()` —
                    // so there is no separate indirect path to fall through to.
                    self.no_value_here(expr)
                } else {
                    self.no_value_here(expr)
                }
            }
            ExpressionKind::TimeLiteral(s) => {
                let ns = parse_time_literal_ns(s).map_err(|why| {
                    CodegenError::UnsupportedType(format!("malformed duration `{s}`: {why}"))
                })?;
                Ok(Some(
                    self.context.i64_type().const_int(ns as u64, true).into(),
                ))
            }
            // DATE and DT are nanoseconds since 1970-01-01, TOD nanoseconds since
            // midnight — the i64 representation `iec_to_llvm_type` documents. These
            // used to compile to a constant 0 whatever the literal said.
            ExpressionKind::DateLiteral(_)
            | ExpressionKind::TodLiteral(_)
            | ExpressionKind::DtLiteral(_) => match parse_date_time_literal_ns(&expr.kind) {
                Some(ns) => Ok(Some(
                    self.context.i64_type().const_int(ns as u64, true).into(),
                )),
                None => Err(CodegenError::UnsupportedType(format!(
                    "malformed date/time literal `{}`",
                    Self::describe_lvalue(expr)
                ))),
            },
            ExpressionKind::StringLiteral(_) => self.compile_string_value(expr, function),
            ExpressionKind::WstringLiteral(_) => {
                // A WSTRING literal as a value: not yet supported in codegen (it is
                // supported as an initializer and as the right-hand side of `:=`).
                self.no_value_here(expr)
            }
            ExpressionKind::DirectVariable(repr) => {
                self.compile_direct_variable_load(repr, expr)
            }
            ExpressionKind::ArrayIndex { .. } => {
                // Element pointer via the lvalue path, then load. The element type
                // comes from the same walk, so a base that is itself a chain
                // (`o[1][2]`, `s.arr[3]`) loads instead of silently producing
                // nothing — matching only a bare identifier here meant `n := o[1][2].a;`
                // emitted no code at all.
                let Some(elem_ptr) = self.compile_lvalue_with_fn(expr, function)? else {
                    return self.no_value_here(expr);
                };
                let Some(elem_ty) = self.lvalue_iec_type(expr) else {
                    return self.no_value_here(expr);
                };
                let elem_llvm_ty = self.iec_to_llvm_type(&elem_ty);
                let val = self
                    .builder
                    .build_load(elem_llvm_ty, elem_ptr, "arr_load")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(val))
            }
            ExpressionKind::MemberAccess { member, .. } => {
                if let Some((object, bit)) = self.bit_access(expr) {
                    return self.compile_bit_read(expr, object, bit, function);
                }
                // STRUCT or FB-instance field at any depth. The field's type comes from
                // the same walk the lvalue path uses, so `o := s.i.v;` and
                // `n := x.f.o;` load instead of silently producing nothing.
                let Some(field_ty) = self.lvalue_iec_type(expr) else {
                    return self.no_value_here(expr);
                };
                let Some(field_ptr) = self.compile_lvalue_with_fn(expr, function)? else {
                    return self.no_value_here(expr);
                };
                let field_llvm_ty = self.iec_to_llvm_type(&field_ty);
                let val = self
                    .builder
                    .build_load(field_llvm_ty, field_ptr, &member.name)
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(val))
            }
            ExpressionKind::TypedLiteral { type_name, value } => {
                self.compile_typed_literal(expr, &type_name.name, value, function)
            }
            ExpressionKind::Dereference(_) => {
                // `pt^` as a value: load the pointee at the pointer's declared base
                // type. Without a known base type there is no width to load.
                let Some(pointee) = self.lvalue_iec_type(expr) else {
                    return self.no_value_here(expr);
                };
                let Some(addr) = self.compile_lvalue_with_fn(expr, function)? else {
                    return self.no_value_here(expr);
                };
                let llvm_ty = self.iec_to_llvm_type(&pointee);
                let val = self
                    .builder
                    .build_load(llvm_ty, addr, "deref")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                Ok(Some(val))
            }
            _ => self.no_value_here(expr),
        }
    }

    /// The literal inside `TYPE#value`, as a compile-time number: an integer (from
    /// an integer literal, a BOOL literal or a negated integer) or a real.
    fn typed_literal_number(value: &Expression) -> Option<Result<i128, f64>> {
        match &value.kind {
            ExpressionKind::IntegerLiteral(v) => Some(Ok(*v)),
            ExpressionKind::BoolLiteral(b) => Some(Ok(i128::from(*b))),
            ExpressionKind::RealLiteral(r) => Some(Err(*r)),
            ExpressionKind::Parenthesized(inner) => Self::typed_literal_number(inner),
            ExpressionKind::UnaryOp {
                op: UnaryOp::Neg,
                operand,
            } => match Self::typed_literal_number(operand)? {
                Ok(v) => Some(Ok(v.checked_neg()?)),
                Err(r) => Some(Err(-r)),
            },
            _ => None,
        }
    }

    /// The constant for a numeric `TYPE#value` of elementary type `ty`, if `value`
    /// is a number that type can hold (an integer for ANY_INT/ANY_BIT, an integer or
    /// a real for ANY_REAL). No builder needed, so global initializers use it too.
    fn typed_literal_const(
        &self,
        ty: &IecType,
        value: &Expression,
    ) -> Option<BasicValueEnum<'ctx>> {
        let is_int = ty.is_any_int() || ty.is_any_bit() || matches!(ty, IecType::Char | IecType::Wchar);
        match (self.iec_to_llvm_type(ty), Self::typed_literal_number(value)?) {
            (BasicTypeEnum::IntType(it), Ok(v)) if is_int => {
                // BOOL#1 / BOOL#TRUE is a truth value, not a bit pattern.
                let v = if matches!(ty, IecType::Bool) {
                    i128::from(v != 0)
                } else {
                    v
                };
                // Keep the low `width` bits ourselves: LLVM's constant constructor
                // does not promise to truncate an out-of-range value.
                let width = it.get_bit_width();
                let bits = if width >= 64 {
                    v as u64
                } else {
                    (v as u64) & ((1u64 << width) - 1)
                };
                Some(it.const_int(bits, false).into())
            }
            (BasicTypeEnum::FloatType(ft), Ok(v)) => Some(ft.const_float(v as f64).into()),
            (BasicTypeEnum::FloatType(ft), Err(r)) => Some(ft.const_float(r).into()),
            _ => None,
        }
    }

    /// `ADR(x)`: the address of the location `x` — a variable, a STRUCT field, an
    /// array element, `pt^`. The result is a POINTER; used as a number it is the
    /// address (see [`Self::compile_pointer_binary_op`]), so the CODESYS idioms
    /// `ADR(str) + pos - 1` and `dw := ADR(x)` both work.
    fn compile_adr(
        &mut self,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let [arg] = args else {
            return Err(CodegenError::UnsupportedType(format!(
                "ADR takes exactly one argument, got {}",
                args.len()
            )));
        };
        match self.compile_lvalue_with_fn(&arg.value, function)? {
            Some(ptr) => Ok(ptr.into()),
            None => Err(CodegenError::UnsupportedType(format!(
                "ADR needs a variable or other addressable location, but `{}` has no address",
                Self::describe_lvalue(&arg.value)
            ))),
        }
    }

    /// `SIZEOF(x)`: the storage size in bytes of variable `x`, or of the type `x`
    /// names, as a UDINT.
    fn compile_sizeof(&mut self, args: &[CallArg]) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let [arg] = args else {
            return Err(CodegenError::UnsupportedType(format!(
                "SIZEOF takes exactly one argument, got {}",
                args.len()
            )));
        };
        let ty = self.lvalue_iec_type(&arg.value).or_else(|| match &arg.value.kind {
            ExpressionKind::Identifier(ident) => plcc_hir::types::resolve_type_name(&ident.name),
            _ => None,
        });
        let Some(ty) = ty else {
            return Err(CodegenError::UnsupportedType(format!(
                "SIZEOF: cannot determine the type of `{}`",
                Self::describe_lvalue(&arg.value)
            )));
        };
        let size = self.iec_to_llvm_type(&ty).size_of().ok_or_else(|| {
            CodegenError::UnsupportedType(format!("SIZEOF: {ty} has no fixed size"))
        })?;
        Ok(self.resize_int(size, self.context.i32_type(), false)?.into())
    }

    /// `TYPE#value` — a literal of an explicit elementary type (`DWORD#1`,
    /// `BYTE#255`, `INT#-10`, `REAL#1`, `BOOL#1`).
    ///
    /// The constant is built at exactly the named type's width, and
    /// [`Self::rvalue_iec_type`] reports that type, so `DWORD#16#FFFFFFFF` widens as
    /// the unsigned 4294967295 and `SINT#-1` as -1. An integer that does not fit the
    /// type keeps its low bits, as the standard's modular integer types do.
    /// Duration, date and string literals already carry their type in the value
    /// (`TIME#5s`, `STRING#'x'`), so their value is compiled as is.
    fn compile_typed_literal(
        &mut self,
        expr: &Expression,
        type_name: &str,
        value: &Expression,
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let Some(ty) = plcc_hir::types::resolve_type_name(type_name) else {
            // A user-defined type (an alias, an enum): the value carries no width of
            // its own here, so compile it untyped.
            return self.compile_expression(value, function);
        };
        // `INT#70000`: a typed literal must fit its type (either as a signed or as
        // an unsigned value of its width, so `WORD#16#FFFF` and `INT#-1` pass).
        // It used to be truncated to its low bits: INT#70000 was 4464.
        if let (Some(Ok(v)), Some(bits)) = (Self::typed_literal_number(value), ty.bit_size()) {
            let fits = if ty == IecType::Bool {
                v == 0 || v == 1
            } else if ty.is_any_int() || ty.is_any_bit() {
                let lo = -(1i128 << (bits - 1));
                let hi = (1i128 << bits) - 1;
                (lo..=hi).contains(&v)
            } else {
                true
            };
            if !fits {
                return Err(CodegenError::UnsupportedType(format!(
                    "typed literal `{}`: {v} is out of range for {ty}",
                    Self::describe_lvalue(expr)
                )));
            }
        }
        if let Some(c) = self.typed_literal_const(&ty, value) {
            return Ok(Some(c));
        }
        match (self.iec_to_llvm_type(&ty), Self::typed_literal_number(value)) {
            (BasicTypeEnum::IntType(_), Some(Err(r)))
                if !matches!(
                    ty,
                    IecType::Time | IecType::Ltime | IecType::Date | IecType::Tod | IecType::Dt
                ) =>
            {
                Err(CodegenError::UnsupportedType(format!(
                    "typed literal `{}`: {r} is not a value of the integer type {ty}",
                    Self::describe_lvalue(expr)
                )))
            }
            _ => self.compile_expression(value, function),
        }
    }

    /// Lower one binary operator.
    ///
    /// `left_ty`/`right_ty` are the operands' static IEC types where codegen can name
    /// them. They decide how each operand widens and whether the operator is signed —
    /// without them every ANY_BIT and ANY_UNSIGNED value above its type's signed range
    /// compared, divided and added wrongly. See [`Self::promote_int_operands`].
    fn compile_binary_op(
        &self,
        op: BinaryOp,
        left: BasicValueEnum<'ctx>,
        left_ty: Option<&IecType>,
        right: BasicValueEnum<'ctx>,
        right_ty: Option<&IecType>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        if left.is_pointer_value() || right.is_pointer_value() {
            return self.compile_pointer_binary_op(op, left, left_ty, right, right_ty);
        }
        for v in [left, right] {
            if !(v.is_int_value() || v.is_float_value()) {
                return Err(CodegenError::UnsupportedType(format!(
                    "operator {op:?} needs scalar operands, but one operand is an ARRAY, \
                     STRUCT or FB instance value"
                )));
            }
        }
        // `**` is EXPT, whose result is always ANY_REAL — never an integer power.
        if op == BinaryOp::Power {
            return self.compile_expt(left, left_ty, right, right_ty);
        }

        // Check if we're dealing with integers or floats
        let is_float = left.is_float_value() || right.is_float_value();

        if is_float {
            // Determine the target float type (use the wider one, or f32 if both are ints)
            let target_fty = if left.is_float_value() && right.is_float_value() {
                let lw = left.into_float_value().get_type();
                let rw = right.into_float_value().get_type();
                let f32t = self.context.f32_type();
                // An untyped real literal (held as LREAL) next to a REAL operand is
                // a REAL: `2.0 * r` stays REAL arithmetic.
                if (lw == f32t && left_ty.is_some() && right_ty.is_none())
                    || (rw == f32t && right_ty.is_some() && left_ty.is_none())
                {
                    f32t
                } else if lw == self.context.f64_type() || rw == self.context.f64_type() {
                    self.context.f64_type()
                } else {
                    f32t
                }
            } else if left.is_float_value() {
                left.into_float_value().get_type()
            } else {
                right.into_float_value().get_type()
            };

            let l = if left.is_float_value() {
                let fv = left.into_float_value();
                if fv.get_type() != target_fty {
                    self.builder
                        .build_float_cast(fv, target_fty, "fcast")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                } else {
                    fv
                }
            } else if Self::signedness_of(left_ty) == Signedness::Unsigned {
                self.builder
                    .build_unsigned_int_to_float(left.into_int_value(), target_fty, "uitof")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            } else {
                self.builder
                    .build_signed_int_to_float(left.into_int_value(), target_fty, "itof")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            };
            let r = if right.is_float_value() {
                let fv = right.into_float_value();
                if fv.get_type() != target_fty {
                    self.builder
                        .build_float_cast(fv, target_fty, "fcast")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                } else {
                    fv
                }
            } else if Self::signedness_of(right_ty) == Signedness::Unsigned {
                self.builder
                    .build_unsigned_int_to_float(right.into_int_value(), target_fty, "uitof")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            } else {
                self.builder
                    .build_signed_int_to_float(right.into_int_value(), target_fty, "itof")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            };

            let result = match op {
                BinaryOp::Add => self
                    .builder
                    .build_float_add(l, r, "fadd")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Sub => self
                    .builder
                    .build_float_sub(l, r, "fsub")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Mul => self
                    .builder
                    .build_float_mul(l, r, "fmul")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Div => self
                    .builder
                    .build_float_div(l, r, "fdiv")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Equal => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::OEQ, l, r, "feq")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::NotEqual => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::UNE, l, r, "fne")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::Less => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::OLT, l, r, "flt")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::LessEqual => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::OLE, l, r, "fle")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::Greater => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::OGT, l, r, "fgt")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::GreaterEqual => {
                    return Ok(self
                        .builder
                        .build_float_compare(FloatPredicate::OGE, l, r, "fge")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                _ => {
                    return Err(CodegenError::UnsupportedType(format!("float op {:?}", op)));
                }
            };
            Ok(result.into())
        } else {
            let (l, r, unsigned) = self.prepare_int_operands(
                left.into_int_value(),
                left_ty,
                right.into_int_value(),
                right_ty,
            )?;
            // CODESYS computes temporary results "always with the native size that
            // is defined by the target device" — at least 32 bits — so `dw := w + 1`
            // with `w : WORD := 65535` is 65536, not 0, and `d := b + b` with
            // `b : BYTE := 255` is 510. The narrow type only applies when the result
            // is stored. (Help: "Operators", note on temporary results.) Without
            // this the answer depended on the literal's default width: `us * 2` was
            // 510 on a USINT but `ui * 2` wrapped to 65534 on a UINT.
            let (l, r) = if matches!(
                op,
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod
            ) && l.get_type().get_bit_width() < 32
            {
                let i32t = self.context.i32_type();
                let s = if unsigned {
                    Signedness::Unsigned
                } else {
                    Signedness::Signed
                };
                (self.widen_to(l, s, i32t)?, self.widen_to(r, s, i32t)?)
            } else {
                (l, r)
            };

            let result = match op {
                BinaryOp::Add => self
                    .builder
                    .build_int_add(l, r, "add")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Sub => self
                    .builder
                    .build_int_sub(l, r, "sub")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Mul => self
                    .builder
                    .build_int_mul(l, r, "mul")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                // ANY_BIT / ANY_UNSIGNED operands divide with udiv/urem. `BYTE 200 / 2`
                // is 100; sdiv reads the 200 as -56 and answers 228.
                BinaryOp::Div | BinaryOp::Mod => self.checked_int_div(op, l, r, unsigned)?,
                BinaryOp::And | BinaryOp::AndThen => self
                    .builder
                    .build_and(l, r, "and")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Or | BinaryOp::OrElse => self
                    .builder
                    .build_or(l, r, "or")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Xor => self
                    .builder
                    .build_xor(l, r, "xor")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?,
                BinaryOp::Equal => {
                    return Ok(self
                        .builder
                        .build_int_compare(IntPredicate::EQ, l, r, "eq")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::NotEqual => {
                    return Ok(self
                        .builder
                        .build_int_compare(IntPredicate::NE, l, r, "ne")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                // Ordering predicates follow the operator's signedness. On i8,
                // `SGT 200, 100` is `-56 > 100` — false — which is how
                // `IF b > 100` with `b : BYTE := 200` took the ELSE branch.
                BinaryOp::Less => {
                    let p = if unsigned {
                        IntPredicate::ULT
                    } else {
                        IntPredicate::SLT
                    };
                    return Ok(self
                        .builder
                        .build_int_compare(p, l, r, "lt")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::LessEqual => {
                    let p = if unsigned {
                        IntPredicate::ULE
                    } else {
                        IntPredicate::SLE
                    };
                    return Ok(self
                        .builder
                        .build_int_compare(p, l, r, "le")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::Greater => {
                    let p = if unsigned {
                        IntPredicate::UGT
                    } else {
                        IntPredicate::SGT
                    };
                    return Ok(self
                        .builder
                        .build_int_compare(p, l, r, "gt")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::GreaterEqual => {
                    let p = if unsigned {
                        IntPredicate::UGE
                    } else {
                        IntPredicate::SGE
                    };
                    return Ok(self
                        .builder
                        .build_int_compare(p, l, r, "ge")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into());
                }
                BinaryOp::Power => {
                    return Err(CodegenError::LlvmError(
                        "internal: `**` reached the integer operator path".into(),
                    ));
                }
            };
            Ok(result.into())
        }
    }

    /// Integer `/` and `MOD`, with no undefined behaviour left for LLVM.
    ///
    /// LLVM's `sdiv`/`udiv`/`srem`/`urem` are undefined behaviour for a zero divisor
    /// and for `MIN / -1`; on x86 both raise SIGFPE and kill the process, on ARM a
    /// division by zero quietly yields 0, and under optimization anything may
    /// happen. So:
    ///
    /// * a zero divisor is a **runtime fault**: `plcc_fault(PLCC_FAULT_DIV_BY_ZERO,
    ///   where)`, as CODESYS stops the task with an exception (see `fault.rs` and
    ///   docs/codesys-compatibility.md). A constant non-zero divisor pays nothing;
    /// * `MIN / -1` is MIN (two's complement wrap, like every other overflow here,
    ///   and what CODESYS gives on ARM) and `MIN MOD -1` is 0.
    fn checked_int_div(
        &self,
        op: BinaryOp,
        l: inkwell::values::IntValue<'ctx>,
        r: inkwell::values::IntValue<'ctx>,
        unsigned: bool,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let ty = r.get_type();
        let zero = ty.const_zero();
        let one = ty.const_int(1, false);
        let const_r = if r.is_const() { r.get_sign_extended_constant() } else { None };
        let mut safe_r = r;
        // Where no fault can be branched to (no enclosing function), fall back to
        // a defined 0.
        let mut select_zero = None;
        if const_r.is_none_or(|c| c == 0) {
            let is_zero = self
                .builder
                .build_int_compare(IntPredicate::EQ, r, zero, "div_by_zero")
                .map_err(err)?;
            if !self.fault_if(is_zero, plcc_runtime::fault::FaultCode::DivByZero)? {
                safe_r = self
                    .builder
                    .build_select(is_zero, one, r, "divisor")
                    .map_err(err)?
                    .into_int_value();
                select_zero = Some(is_zero);
            }
        }
        if !unsigned && const_r.is_none_or(|c| c == -1) {
            // MIN / -1 overflows; MIN / 1 is the wrapped answer, MIN MOD 1 is 0.
            let bits = ty.get_bit_width();
            let min = ty.const_int(1u64 << (bits - 1).min(63), false);
            let min = if bits > 64 { ty.const_zero() } else { min };
            let l_min = self
                .builder
                .build_int_compare(IntPredicate::EQ, l, min, "lhs_min")
                .map_err(err)?;
            let r_m1 = self
                .builder
                .build_int_compare(IntPredicate::EQ, r, ty.const_all_ones(), "rhs_m1")
                .map_err(err)?;
            let ovf = self.builder.build_and(l_min, r_m1, "div_ovf").map_err(err)?;
            safe_r = self
                .builder
                .build_select(ovf, one, safe_r, "divisor")
                .map_err(err)?
                .into_int_value();
        }
        let q = match (op, unsigned) {
            (BinaryOp::Div, true) => self.builder.build_int_unsigned_div(l, safe_r, "udiv"),
            (BinaryOp::Div, false) => self.builder.build_int_signed_div(l, safe_r, "div"),
            (_, true) => self.builder.build_int_unsigned_rem(l, safe_r, "umod"),
            (_, false) => self.builder.build_int_signed_rem(l, safe_r, "mod"),
        }
        .map_err(err)?;
        Ok(match select_zero {
            Some(is_zero) => self
                .builder
                .build_select(is_zero, zero, q, "div_result")
                .map_err(err)?
                .into_int_value(),
            None => q,
        })
    }

    /// A binary operator with at least one POINTER operand.
    ///
    /// Pointer arithmetic follows CODESYS, which OSCAT is written against: a pointer
    /// is an address counted in **bytes**, whatever it points to. `pt := pt + 1`
    /// steps a `POINTER TO BYTE` to the next byte, and `ADR(str) + pos - 1` is the
    /// address of a character. So `ptr ± int` offsets the address by that many bytes
    /// and stays a pointer; `ptr - ptr` is the byte distance; comparisons compare
    /// addresses (unsigned). Any other operator works on the address as an unsigned
    /// integer (LWORD).
    fn compile_pointer_binary_op(
        &self,
        op: BinaryOp,
        left: BasicValueEnum<'ctx>,
        left_ty: Option<&IecType>,
        right: BasicValueEnum<'ctx>,
        right_ty: Option<&IecType>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let byte_offset = |ptr: PointerValue<'ctx>,
                           off: BasicValueEnum<'ctx>,
                           off_ty: Option<&IecType>,
                           negate: bool|
         -> Result<BasicValueEnum<'ctx>, CodegenError> {
            let what = format!("the offset in pointer arithmetic ({op:?})");
            let off = self.int_operand(off, &what)?;
            let off = self.widen_to(off, Self::signedness_of(off_ty), self.addr_int_type())?;
            let off = if negate {
                self.builder
                    .build_int_neg(off, "neg_off")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            } else {
                off
            };
            let p = unsafe {
                self.builder
                    .build_gep(self.context.i8_type(), ptr, &[off], "ptr_off")
                    .map_err(|e| CodegenError::LlvmError(e.to_string()))?
            };
            Ok(p.into())
        };
        match (op, left, right) {
            (BinaryOp::Add, BasicValueEnum::PointerValue(p), off)
                if !off.is_pointer_value() =>
            {
                byte_offset(p, off, right_ty, false)
            }
            (BinaryOp::Add, off, BasicValueEnum::PointerValue(p))
                if !off.is_pointer_value() =>
            {
                byte_offset(p, off, left_ty, false)
            }
            (BinaryOp::Sub, BasicValueEnum::PointerValue(p), off)
                if !off.is_pointer_value() =>
            {
                byte_offset(p, off, right_ty, true)
            }
            _ => {
                // Everything else works on the addresses as unsigned integers:
                // `p1 - p2`, `pt < pend`, `pt = 0`, `ADR(x) AND 16#3`.
                let as_addr = |v: BasicValueEnum<'ctx>, ty: Option<&IecType>| {
                    if v.is_pointer_value() {
                        self.int_operand(v, "pointer arithmetic")
                            .map(|iv| (BasicValueEnum::from(iv), Some(IecType::Lword)))
                    } else {
                        Ok((v, ty.cloned()))
                    }
                };
                let (l, lt) = as_addr(left, left_ty)?;
                let (r, rt) = as_addr(right, right_ty)?;
                if l.is_float_value() || r.is_float_value() {
                    return Err(CodegenError::UnsupportedType(format!(
                        "operator {op:?} between a POINTER and a REAL/LREAL value"
                    )));
                }
                self.compile_binary_op(op, l, lt.as_ref(), r, rt.as_ref())
            }
        }
    }

    fn compile_unary_op(
        &self,
        op: UnaryOp,
        operand: BasicValueEnum<'ctx>,
        operand_ty: Option<&IecType>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        match op {
            UnaryOp::Neg => {
                if operand.is_float_value() {
                    Ok(self
                        .builder
                        .build_float_neg(operand.into_float_value(), "fneg")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into())
                } else if operand.is_int_value() {
                    // A native-width temporary, like the binary operators: `-us`
                    // with `us : USINT := 200` is -200, not 56.
                    let iv = operand.into_int_value();
                    let iv = if iv.get_type().get_bit_width() > 1
                        && iv.get_type().get_bit_width() < 32
                    {
                        self.widen_to(iv, Self::signedness_of(operand_ty), self.context.i32_type())?
                    } else {
                        iv
                    };
                    Ok(self
                        .builder
                        .build_int_neg(iv, "neg")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into())
                } else {
                    Err(CodegenError::UnsupportedType(
                        "unary minus needs a numeric operand, not a POINTER or aggregate value"
                            .into(),
                    ))
                }
            }
            UnaryOp::Not => {
                let int_val = self.int_operand(operand, "NOT")?;
                // NOT is logical on BOOL and bitwise on everything else. BOOL and
                // BYTE/SINT/USINT share the i8 storage type, so the choice is made by
                // the operand's *static IEC type*, never by LLVM width: deciding by
                // width made `NOT BYTE#16#0F` 0 instead of 16#F0. An i1 is always a
                // BOOL (a comparison result). An i8 with no static type is treated as
                // BOOL — the untyped i8 values codegen produces are BOOL literals.
                let is_bool = match operand_ty {
                    Some(t) => matches!(t, IecType::Bool),
                    None => int_val.get_type().get_bit_width() <= 8,
                } || int_val.get_type().get_bit_width() == 1;
                if is_bool {
                    // Boolean NOT: compare equal to zero, then extend back to i8
                    let zero = int_val.get_type().const_zero();
                    let is_zero = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, int_val, zero, "lnot")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    let result = self
                        .builder
                        .build_int_z_extend(is_zero, int_val.get_type(), "lnot_ext")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
                    Ok(result.into())
                } else {
                    // Bitwise NOT for every ANY_BIT / ANY_INT type but BOOL.
                    Ok(self
                        .builder
                        .build_not(int_val, "not")
                        .map_err(|e| CodegenError::LlvmError(e.to_string()))?
                        .into())
                }
            }
        }
    }

    /// Get a reference to the LLVM module.
    pub fn module(&self) -> &Module<'ctx> {
        &self.module
    }

    /// Emit LLVM IR to a string.
    pub fn emit_ir(&self) -> String {
        self.module.print_to_string().to_string()
    }

    /// Write object file to disk.
    pub fn emit_object(&self, path: &Path, triple: &str) -> Result<(), CodegenError> {
        let machine = self.target_machine(triple)?;
        self.set_target(triple)?;
        machine
            .write_to_file(&self.module, FileType::Object, path)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;

        Ok(())
    }

    /// The object file as bytes (no file system needed: the browser compiler).
    pub fn emit_object_bytes(&self, triple: &str) -> Result<Vec<u8>, CodegenError> {
        let machine = self.target_machine(triple)?;
        self.set_target(triple)?;
        let buf = machine
            .write_to_memory_buffer(&self.module, FileType::Object)
            .map_err(|e| CodegenError::LlvmError(e.to_string()))?;
        Ok(buf.as_slice().to_vec())
    }

    pub(crate) fn target_machine(&self, triple: &str) -> Result<TargetMachine, CodegenError> {
        Target::initialize_all(&InitializationConfig::default());
        let target_triple = TargetTriple::create(triple);
        let target = Target::from_triple(&target_triple)
            .map_err(|e| CodegenError::TargetError(e.to_string()))?;
        target
            .create_target_machine(
                &target_triple,
                &self.machine.cpu,
                &self.machine.features,
                OptimizationLevel::Default,
                RelocMode::Default,
                CodeModel::Default,
            )
            .ok_or_else(|| CodegenError::TargetError("failed to create target machine".into()))
    }

    /// Stamp the module with `triple` and that target's data layout.
    ///
    /// Without it an emitted `.ll`/`.bc` carries no data layout, and any optimizer
    /// that later runs over it (`opt -O2`, `clang -O2 x.ll`) folds struct accesses
    /// into byte offsets using LLVM's *default* layout, where an i64 is only 4-byte
    /// aligned: every LINT/LREAL/TIME field after a 4-byte one moves, and the code
    /// no longer agrees with the offsets in the generated C header.
    ///
    /// On WebAssembly it also turns `plcc_fault` into an import: a wasm module's
    /// host (JavaScript) cannot replace a weak definition at link time the way a
    /// C runtime does, so the weak trapping default would win and a fault would
    /// reach the host as a bare `unreachable` trap without its code and site.
    pub fn set_target(&self, triple: &str) -> Result<(), CodegenError> {
        if triple.starts_with("wasm") {
            self.use_external_fault_handler();
        }
        let machine = self.target_machine(triple)?;
        self.module.set_triple(&TargetTriple::create(triple));
        self.module
            .set_data_layout(&machine.get_target_data().get_data_layout());
        self.stamp_machine_attributes();
        Ok(())
    }

    /// `target-cpu` / `target-features` on every defined function, as clang
    /// writes them, when a CPU or features were chosen.
    fn stamp_machine_attributes(&self) {
        let m = &self.machine;
        if m.cpu == "generic" && m.features.is_empty() {
            return;
        }
        let kind = inkwell::attributes::AttributeLoc::Function;
        for f in self.module.get_functions() {
            if f.count_basic_blocks() == 0 {
                continue;
            }
            if m.cpu != "generic" {
                f.add_attribute(kind, self.context.create_string_attribute("target-cpu", &m.cpu));
            }
            if !m.features.is_empty() {
                f.add_attribute(
                    kind,
                    self.context.create_string_attribute("target-features", &m.features),
                );
            }
        }
    }

    /// Run LLVM's `default<O{level}>` pipeline over the module for `triple`
    /// (which also stamps the triple and data layout, see [`Self::set_target`]).
    pub fn optimize(&self, triple: &str, level: u8) -> Result<(), CodegenError> {
        self.set_target(triple)?;
        let machine = self.target_machine(triple)?;
        self.module
            .run_passes(
                &format!("default<O{}>", level.min(3)),
                &machine,
                inkwell::passes::PassBuilderOptions::create(),
            )
            .map_err(|e| CodegenError::LlvmError(e.to_string()))
    }

    /// Write bitcode to disk.
    pub fn emit_bitcode(&self, path: &Path) -> bool {
        self.module.write_bitcode_to_path(path)
    }
}
