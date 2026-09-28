// SPDX-License-Identifier: MPL-2.0

//! Type conversion functions between elementary types: `<SRC>_TO_<DST>` for every
//! pair of elementary types, and the IEC 61131-3 3rd edition overloaded `TO_<DST>`.
//!
//! One lowering for the whole family, driven by the two types rather than by a
//! hand-enumerated list of names (which left `DWORD_TO_WORD`, `DT_TO_DWORD`, … as
//! "unknown function").
//!
//! # Units
//!
//! Every TIME / date type is stored as i64 nanoseconds. Converting one to or from a
//! *number* uses the unit CODESYS uses (see docs/codesys-compatibility.md):
//!
//! | Type | Numeric unit |
//! |---|---|
//! | TIME, TIME_OF_DAY | milliseconds (TOD: since midnight) |
//! | DATE, DATE_AND_TIME | seconds since 1970-01-01 |
//! | LTIME, LTOD, LDATE, LDT | nanoseconds |
//!
//! Numeric → integer results truncate toward zero; REAL/LREAL → integer rounds
//! (halves away from zero) and saturates, like every such conversion (see
//! `float_to_int`).

use super::*;

const NS_PER_DAY: i64 = 86_400 * 1_000_000_000;

impl<'ctx> Compiler<'ctx> {
    /// `(source, destination)` of a conversion function name, when `name` is one:
    /// `DWORD_TO_DATE`, `TIME_OF_DAY_TO_DWORD`, `DT_TO_TOD`. `TO_<DST>` has no
    /// named source (`None`); it is the argument's own type.
    pub(super) fn parse_conversion(name: &str) -> Option<(Option<IecType>, IecType)> {
        let upper = name.to_uppercase();
        let convertible = |t: &IecType| {
            t.is_any_elementary()
                && !matches!(t, IecType::StringType { .. } | IecType::WstringType { .. })
        };
        if let Some(dst) = upper.strip_prefix("TO_") {
            let dst = plcc_hir::types::resolve_type_name(dst)?;
            return convertible(&dst).then_some((None, dst));
        }
        // Try every `_TO_` split: `TIME_OF_DAY_TO_DT` has only one that names two
        // types on either side.
        let mut from = 0;
        while let Some(pos) = upper[from..].find("_TO_") {
            let at = from + pos;
            let (src, dst) = (&upper[..at], &upper[at + 4..]);
            if let (Some(s), Some(d)) = (
                plcc_hir::types::resolve_type_name(src),
                plcc_hir::types::resolve_type_name(dst),
            ) {
                if convertible(&s) && convertible(&d) {
                    return Some((Some(s), d));
                }
            }
            from = at + 1;
        }
        None
    }

    /// Nanoseconds per numeric unit of a TIME / date type (see the module docs).
    fn temporal_scale(ty: &IecType) -> Option<i64> {
        match ty {
            IecType::Time | IecType::Tod => Some(1_000_000),
            IecType::Date | IecType::Dt => Some(1_000_000_000),
            IecType::Ltime | IecType::Ldate | IecType::Ltod | IecType::Ldt => Some(1),
            _ => None,
        }
    }

    /// An integer literal used where a TIME / date value is expected — `t := 5000;`,
    /// `ton(PT := 20)`, `t > 1000` — in nanoseconds, reading the number in the
    /// type's numeric unit (milliseconds for TIME, as CODESYS does; see
    /// `temporal_scale`). It used to be taken as raw nanoseconds, so
    /// `PT := 20` was a 20 ns timer.
    pub(super) fn temporal_literal_ns(expr: &Expression, ty: &IecType) -> Option<i64> {
        let scale = Self::temporal_scale(ty.base())?;
        let v = Self::const_int_of(expr)?;
        i64::try_from(v).ok()?.checked_mul(scale)
    }

    /// Lower `name(args)` if it is a conversion function.
    pub(super) fn compile_conversion_call(
        &mut self,
        name: &str,
        args: &[CallArg],
        function: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let Some((src, dst)) = Self::parse_conversion(name) else {
            return Ok(None);
        };
        let uname = name.to_uppercase();
        let [arg] = args else {
            return Err(CodegenError::ArgumentBinding {
                callee: uname,
                problem: format!("takes exactly 1 argument, got {}", args.len()),
            });
        };
        let arg_ty = self.rvalue_iec_type(&arg.value).map(|t| t.base().clone());
        let Some(val) = self.compile_expression(&arg.value, function)? else {
            return Err(self.no_value_error(format!("argument of `{uname}`"), &arg.value));
        };
        // The argument is first converted to the named source type, as for any
        // call; `TO_<DST>` converts from whatever the argument is.
        let (val, src) = match src {
            Some(src) => (self.coerce_value(val, arg_ty.as_ref(), &src)?, src),
            None => {
                let src = arg_ty.unwrap_or_else(|| match val {
                    BasicValueEnum::FloatValue(f) if f.get_type() == self.context.f32_type() => {
                        IecType::Real
                    }
                    BasicValueEnum::FloatValue(_) => IecType::Lreal,
                    _ => IecType::Lint,
                });
                (val, src)
            }
        };
        Ok(Some(self.convert_elementary(val, &src, &dst, &uname)?))
    }

    /// Convert `val`, a value of elementary type `src`, to `dst`.
    pub(super) fn convert_elementary(
        &self,
        val: BasicValueEnum<'ctx>,
        src: &IecType,
        dst: &IecType,
        what: &str,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let i64t = self.context.i64_type();
        let dst_llvm = self.iec_to_llvm_type(dst);

        // Anything → BOOL: non-zero is TRUE.
        if *dst == IecType::Bool {
            let nz = match val {
                BasicValueEnum::FloatValue(f) => self
                    .builder
                    .build_float_compare(FloatPredicate::UNE, f, f.get_type().const_zero(), "nz")
                    .map_err(err)?,
                _ => {
                    let iv = self.int_operand(val, what)?;
                    self.builder
                        .build_int_compare(IntPredicate::NE, iv, iv.get_type().const_zero(), "nz")
                        .map_err(err)?
                }
            };
            return Ok(self
                .builder
                .build_int_z_extend(nz, self.context.i8_type(), "to_bool")
                .map_err(err)?
                .into());
        }
        // BOOL → anything: 0 or 1 (a BOOL may arrive as an i1 comparison result).
        let (val, src) = if *src == IecType::Bool {
            let iv = self.int_operand(val, what)?;
            let nz = self
                .builder
                .build_int_compare(IntPredicate::NE, iv, iv.get_type().const_zero(), "b")
                .map_err(err)?;
            let v = self.builder.build_int_z_extend(nz, i64t, "b64").map_err(err)?;
            (BasicValueEnum::from(v), IecType::Ulint)
        } else {
            (val, src.clone())
        };

        match (Self::temporal_scale(&src), Self::temporal_scale(dst)) {
            // TIME/date → TIME/date: same nanoseconds, except that the date part or
            // the time-of-day part is taken where the destination has only one.
            (Some(_), Some(_)) => {
                let ns = self.resize_int(self.int_operand(val, what)?, i64t, true)?;
                let day = i64t.const_int(NS_PER_DAY as u64, true);
                let has_date = |t: &IecType| {
                    matches!(t, IecType::Dt | IecType::Ldt | IecType::Date | IecType::Ldate)
                };
                let is_date_only = |t: &IecType| matches!(t, IecType::Date | IecType::Ldate);
                let is_tod = |t: &IecType| matches!(t, IecType::Tod | IecType::Ltod);
                if has_date(&src) && is_tod(dst) {
                    // Euclidean remainder: the time of day of a pre-1970 DT is
                    // still in 0..24h.
                    let r = self.builder.build_int_signed_rem(ns, day, "tod").map_err(err)?;
                    let neg = self
                        .builder
                        .build_int_compare(IntPredicate::SLT, r, i64t.const_zero(), "neg")
                        .map_err(err)?;
                    let adj = self.builder.build_int_add(r, day, "tod_adj").map_err(err)?;
                    return Ok(self.builder.build_select(neg, adj, r, "tod").map_err(err)?);
                }
                if has_date(&src) && !is_date_only(&src) && is_date_only(dst) {
                    let r = self.builder.build_int_signed_rem(ns, day, "tod").map_err(err)?;
                    let neg = self
                        .builder
                        .build_int_compare(IntPredicate::SLT, r, i64t.const_zero(), "neg")
                        .map_err(err)?;
                    let adj = self.builder.build_int_add(r, day, "tod_adj").map_err(err)?;
                    let r = self.builder.build_select(neg, adj, r, "tod").map_err(err)?;
                    return Ok(self
                        .builder
                        .build_int_sub(ns, r.into_int_value(), "date")
                        .map_err(err)?
                        .into());
                }
                Ok(ns.into())
            }
            // TIME/date → number, in the source's numeric unit.
            (Some(scale), None) => {
                let ns = self.resize_int(self.int_operand(val, what)?, i64t, true)?;
                match dst_llvm {
                    BasicTypeEnum::FloatType(ft) => {
                        let f = self
                            .builder
                            .build_signed_int_to_float(ns, self.context.f64_type(), "ns_f")
                            .map_err(err)?;
                        let f = self
                            .builder
                            .build_float_div(f, self.context.f64_type().const_float(scale as f64), "units")
                            .map_err(err)?;
                        Ok(self.builder.build_float_cast(f, ft, "units_f").map_err(err)?.into())
                    }
                    BasicTypeEnum::IntType(it) => {
                        let q = self
                            .builder
                            .build_int_signed_div(ns, i64t.const_int(scale as u64, true), "units")
                            .map_err(err)?;
                        Ok(self.resize_int(q, it, true)?.into())
                    }
                    _ => Err(CodegenError::UnsupportedType(format!("{what}: unsupported target"))),
                }
            }
            // Number → TIME/date.
            (None, Some(scale)) => match val {
                BasicValueEnum::FloatValue(f) => {
                    let f64t = self.context.f64_type();
                    let f = self.builder.build_float_cast(f, f64t, "f64").map_err(err)?;
                    let ns = self
                        .builder
                        .build_float_mul(f, f64t.const_float(scale as f64), "ns_f")
                        .map_err(err)?;
                    Ok(self.float_to_int(ns, &IecType::Lint, true)?.into())
                }
                _ => {
                    let iv = self.int_operand(val, what)?;
                    let wide = self.resize_int(iv, i64t, !Self::widens_unsigned(&src))?;
                    Ok(self
                        .builder
                        .build_int_mul(wide, i64t.const_int(scale as u64, true), "ns")
                        .map_err(err)?
                        .into())
                }
            },
            // Number → number.
            (None, None) => match (val, dst_llvm) {
                (BasicValueEnum::FloatValue(f), BasicTypeEnum::FloatType(ft)) => {
                    Ok(self.builder.build_float_cast(f, ft, "fcast").map_err(err)?.into())
                }
                (BasicValueEnum::FloatValue(f), BasicTypeEnum::IntType(_)) => {
                    Ok(self.float_to_int(f, dst, true)?.into())
                }
                (v, BasicTypeEnum::FloatType(ft)) => {
                    let iv = self.int_operand(v, what)?;
                    Ok(if Self::widens_unsigned(&src) {
                        self.builder.build_unsigned_int_to_float(iv, ft, "uitof")
                    } else {
                        self.builder.build_signed_int_to_float(iv, ft, "sitof")
                    }
                    .map_err(err)?
                    .into())
                }
                (v, BasicTypeEnum::IntType(it)) => {
                    let iv = self.int_operand(v, what)?;
                    Ok(self.resize_int(iv, it, !Self::widens_unsigned(&src))?.into())
                }
                _ => Err(CodegenError::UnsupportedType(format!(
                    "{what}: cannot convert {src} to {dst}"
                ))),
            },
        }
    }

    /// The date and time functions of IEC 61131-3 Table 30 (`ADD_DT_TIME`,
    /// `SUB_DT_DT`, `MUL_TIME`, …), `CONCAT_DATE_TOD` / `CONCAT_DATE` / `CONCAT_TOD` /
    /// `CONCAT_DT`, `DAY_OF_WEEK`, and the CODESYS `TIME()`.
    pub(super) fn is_datetime_function(name: &str) -> bool {
        matches!(
            name,
            "ADD_TIME"
                | "ADD_LTIME"
                | "ADD_TOD_TIME"
                | "ADD_LTOD_LTIME"
                | "ADD_DT_TIME"
                | "ADD_LDT_LTIME"
                | "SUB_TIME"
                | "SUB_LTIME"
                | "SUB_DATE_DATE"
                | "SUB_LDATE_LDATE"
                | "SUB_TOD_TIME"
                | "SUB_LTOD_LTIME"
                | "SUB_TOD_TOD"
                | "SUB_LTOD_LTOD"
                | "SUB_DT_TIME"
                | "SUB_LDT_LTIME"
                | "SUB_DT_DT"
                | "SUB_LDT_LDT"
                | "MUL_TIME"
                | "MUL_LTIME"
                | "DIV_TIME"
                | "DIV_LTIME"
                | "CONCAT_DATE_TOD"
                | "CONCAT_LDATE_LTOD"
                | "CONCAT_DATE"
                | "CONCAT_TOD"
                | "CONCAT_DT"
                | "DAY_OF_WEEK"
                | "TIME"
        )
    }

    /// Result type of a [`Self::is_datetime_function`] function.
    pub(super) fn datetime_result_type(name: &str) -> Option<IecType> {
        Some(match name {
            "DAY_OF_WEEK" => IecType::Int,
            "CONCAT_DATE" => IecType::Date,
            "CONCAT_TOD" => IecType::Tod,
            "CONCAT_DT" | "CONCAT_DATE_TOD" | "ADD_DT_TIME" | "SUB_DT_TIME" => IecType::Dt,
            "CONCAT_LDATE_LTOD" | "ADD_LDT_LTIME" | "SUB_LDT_LTIME" => IecType::Ldt,
            "ADD_TOD_TIME" | "SUB_TOD_TIME" => IecType::Tod,
            "ADD_LTOD_LTIME" | "SUB_LTOD_LTIME" => IecType::Ltod,
            n if n.contains("LTIME") || n.ends_with("_LDATE") || n.ends_with("_LTOD")
                || n.ends_with("_LDT") =>
            {
                IecType::Ltime
            }
            n if Self::is_datetime_function(n) => IecType::Time,
            _ => return None,
        })
    }

    pub(super) fn compile_datetime_call(
        &self,
        name: &str,
        args: &[BasicValueEnum<'ctx>],
        arg_tys: &[Option<IecType>],
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let i64t = self.context.i64_type();
        let arity = match name {
            "TIME" => 0,
            "DAY_OF_WEEK" => 1,
            "CONCAT_DATE" => 3,
            "CONCAT_TOD" => 4,
            "CONCAT_DT" => 7,
            _ => 2,
        };
        if args.len() != arity {
            return Err(CodegenError::ArgumentBinding {
                callee: name.to_string(),
                problem: format!("takes {arity} argument(s), got {}", args.len()),
            });
        }
        // Argument `i` as an i64, extended by its own signedness.
        let wide = |i: usize| -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
            let v = match args[i] {
                BasicValueEnum::FloatValue(f) => {
                    return self.float_to_int(f, &IecType::Lint, true);
                }
                v => self.int_operand(v, name)?,
            };
            self.widen_to(v, Self::signedness_of(arg_tys[i].as_ref()), i64t)
        };
        let c = |v: i64| i64t.const_int(v as u64, true);
        match name {
            // CODESYS: TIME() is the time since the PLC started, in milliseconds.
            "TIME" => {
                let clock = self.get_or_declare_monotonic_ns();
                let now = match self
                    .builder
                    .build_call(clock, &[], "now")
                    .map_err(err)?
                    .try_as_basic_value()
                {
                    inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
                    inkwell::values::ValueKind::Instruction(_) => {
                        return Err(CodegenError::LlvmError("clock returned no value".into()));
                    }
                };
                let ms = self.builder.build_int_signed_div(now, c(1_000_000), "ms").map_err(err)?;
                Ok(Some(self.builder.build_int_mul(ms, c(1_000_000), "t").map_err(err)?.into()))
            }
            "MUL_TIME" | "MUL_LTIME" | "DIV_TIME" | "DIV_LTIME" => {
                let op = if name.starts_with("MUL") {
                    BinaryOp::Mul
                } else {
                    BinaryOp::Div
                };
                let t = wide(0)?;
                let r = self.compile_binary_op(
                    op,
                    t.into(),
                    Some(&IecType::Time),
                    args[1],
                    arg_tys[1].as_ref(),
                )?;
                Ok(Some(match r {
                    BasicValueEnum::FloatValue(f) => self.float_to_int(f, &IecType::Lint, true)?.into(),
                    v => self.resize_int(self.int_operand(v, name)?, i64t, true)?.into(),
                }))
            }
            "DAY_OF_WEEK" => {
                // 1970-01-01 was a Thursday; IEC numbers Sunday 0.
                let days = self.floor_div(wide(0)?, c(86_400 * 1_000_000_000))?;
                let d = self.builder.build_int_add(days, c(4), "d4").map_err(err)?;
                let r = self.floor_mod(d, c(7))?;
                Ok(Some(self.resize_int(r, self.context.i16_type(), true)?.into()))
            }
            "CONCAT_DATE" => {
                let days = self.days_from_civil(wide(0)?, wide(1)?, wide(2)?)?;
                Ok(Some(self.builder.build_int_mul(days, c(86_400 * 1_000_000_000), "date").map_err(err)?.into()))
            }
            "CONCAT_TOD" => Ok(Some(self.hms_ns(wide(0)?, wide(1)?, wide(2)?, wide(3)?)?.into())),
            "CONCAT_DT" => {
                let days = self.days_from_civil(wide(0)?, wide(1)?, wide(2)?)?;
                let date = self.builder.build_int_mul(days, c(86_400 * 1_000_000_000), "date").map_err(err)?;
                let tod = self.hms_ns(wide(3)?, wide(4)?, wide(5)?, wide(6)?)?;
                Ok(Some(self.builder.build_int_add(date, tod, "dt").map_err(err)?.into()))
            }
            n if n.starts_with("SUB_") => {
                Ok(Some(self.builder.build_int_sub(wide(0)?, wide(1)?, "sub_t").map_err(err)?.into()))
            }
            // ADD_* and CONCAT_DATE_TOD: nanoseconds add.
            _ => Ok(Some(self.builder.build_int_add(wide(0)?, wide(1)?, "add_t").map_err(err)?.into())),
        }
    }

    /// `a / b` rounded toward negative infinity (b > 0).
    fn floor_div(
        &self,
        a: inkwell::values::IntValue<'ctx>,
        b: inkwell::values::IntValue<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let q = self.builder.build_int_signed_div(a, b, "q").map_err(err)?;
        let r = self.builder.build_int_signed_rem(a, b, "r").map_err(err)?;
        let neg = self
            .builder
            .build_int_compare(IntPredicate::SLT, r, r.get_type().const_zero(), "rneg")
            .map_err(err)?;
        let q1 = self
            .builder
            .build_int_sub(q, q.get_type().const_int(1, false), "q1")
            .map_err(err)?;
        Ok(self.builder.build_select(neg, q1, q, "fdiv").map_err(err)?.into_int_value())
    }

    /// `a mod b` in `0..b` (b > 0).
    fn floor_mod(
        &self,
        a: inkwell::values::IntValue<'ctx>,
        b: inkwell::values::IntValue<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let r = self.builder.build_int_signed_rem(a, b, "r").map_err(err)?;
        let neg = self
            .builder
            .build_int_compare(IntPredicate::SLT, r, r.get_type().const_zero(), "rneg")
            .map_err(err)?;
        let adj = self.builder.build_int_add(r, b, "radj").map_err(err)?;
        Ok(self.builder.build_select(neg, adj, r, "fmod").map_err(err)?.into_int_value())
    }

    /// Days since 1970-01-01 of the proleptic Gregorian date y-m-d (Howard
    /// Hinnant's `days_from_civil`), in IR.
    fn days_from_civil(
        &self,
        y: inkwell::values::IntValue<'ctx>,
        m: inkwell::values::IntValue<'ctx>,
        d: inkwell::values::IntValue<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let i64t = self.context.i64_type();
        let c = |v: i64| i64t.const_int(v as u64, true);
        let b = &self.builder;
        let early = b.build_int_compare(IntPredicate::SLE, m, c(2), "early").map_err(err)?;
        let early64 = b.build_int_z_extend(early, i64t, "e64").map_err(err)?;
        let y = b.build_int_sub(y, early64, "y").map_err(err)?;
        let era = self.floor_div(y, c(400))?;
        let yoe = b.build_int_sub(y, b.build_int_mul(era, c(400), "e400").map_err(err)?, "yoe").map_err(err)?;
        let mp_hi = b.build_int_sub(m, c(3), "m3").map_err(err)?;
        let mp_lo = b.build_int_add(m, c(9), "m9").map_err(err)?;
        let mp = b.build_select(early, mp_lo, mp_hi, "mp").map_err(err)?.into_int_value();
        let doy = b.build_int_mul(mp, c(153), "t").map_err(err)?;
        let doy = b.build_int_add(doy, c(2), "t").map_err(err)?;
        let doy = b.build_int_signed_div(doy, c(5), "t").map_err(err)?;
        let doy = b.build_int_add(doy, d, "t").map_err(err)?;
        let doy = b.build_int_sub(doy, c(1), "doy").map_err(err)?;
        let doe = b.build_int_mul(yoe, c(365), "t").map_err(err)?;
        let doe = b.build_int_add(doe, b.build_int_signed_div(yoe, c(4), "y4").map_err(err)?, "t").map_err(err)?;
        let doe = b.build_int_sub(doe, b.build_int_signed_div(yoe, c(100), "y100").map_err(err)?, "t").map_err(err)?;
        let doe = b.build_int_add(doe, doy, "doe").map_err(err)?;
        let days = b.build_int_mul(era, c(146_097), "t").map_err(err)?;
        let days = b.build_int_add(days, doe, "t").map_err(err)?;
        b.build_int_sub(days, c(719_468), "days").map_err(err)
    }

    /// Nanoseconds of h:m:s.ms.
    fn hms_ns(
        &self,
        h: inkwell::values::IntValue<'ctx>,
        m: inkwell::values::IntValue<'ctx>,
        s: inkwell::values::IntValue<'ctx>,
        ms: inkwell::values::IntValue<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, CodegenError> {
        let err = |e: inkwell::builder::BuilderError| CodegenError::LlvmError(e.to_string());
        let i64t = self.context.i64_type();
        let c = |v: i64| i64t.const_int(v as u64, true);
        let b = &self.builder;
        let t = b.build_int_mul(h, c(60), "t").map_err(err)?;
        let t = b.build_int_add(t, m, "t").map_err(err)?;
        let t = b.build_int_mul(t, c(60), "t").map_err(err)?;
        let t = b.build_int_add(t, s, "t").map_err(err)?;
        let t = b.build_int_mul(t, c(1000), "t").map_err(err)?;
        let t = b.build_int_add(t, ms, "t").map_err(err)?;
        b.build_int_mul(t, c(1_000_000), "tod").map_err(err)
    }
}
