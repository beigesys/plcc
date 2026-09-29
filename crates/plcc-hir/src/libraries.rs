// SPDX-License-Identifier: MPL-2.0

//! Names from vendor libraries plcc does not provide, so that using one is
//! reported as a missing library ("`ADSLOGSTR` is part of the Beckhoff library
//! Tc2_System") instead of a bare "undefined". plcc implements the IEC
//! standard library (and the Tc2_Standard blocks with the same names); it does
//! not implement Beckhoff's system, utility, motion or I/O libraries, which
//! wrap TwinCAT runtime services.
//!
//! Only names are listed here — enough to recognise them.

/// (library, names). Names are matched without regard to case.
const LIBRARIES: &[(&str, &[&str])] = &[
    (
        "Tc2_System",
        &[
            "E_SEEKORIGIN",
            "SEEK_SET",
            "SEEK_CUR",
            "SEEK_END",
            "ADSLOGSTR",
            "ADSLOGDINT",
            "ADSLOGLREAL",
            "ADSLOG_MSGTYPE_HINT",
            "ADSLOG_MSGTYPE_WARN",
            "ADSLOG_MSGTYPE_ERROR",
            "ADSLOG_MSGTYPE_LOG",
            "ADSLOG_MSGTYPE_MSGBOX",
            "ADSLOG_MSGTYPE_RESOURCE",
            "ADSLOG_MSGTYPE_STRING",
            "ADSREAD",
            "ADSWRITE",
            "ADSRDWRT",
            "ADSREADEX",
            "ADSRDWRTEX",
            "ADSRDSTATE",
            "ADSWRTCTL",
            "ADSRDDEVINFO",
            "GETSYSTEMTIME",
            "GETCPUCOUNTER",
            "GETCPUACCOUNT",
            "GETCURTASKINDEX",
            "T_MAXSTRING",
            "T_AMSNETID",
            "T_AMSPORT",
            "FB_FILEOPEN",
            "FB_FILECLOSE",
            "FB_FILEREAD",
            "FB_FILEWRITE",
            "FB_FILEGETS",
            "FB_FILEPUTS",
            "FB_FILEDELETE",
            "FB_FILESEEK",
            "FB_FILETELL",
            "FB_FILERENAME",
            "FOPEN_MODEREAD",
            "FOPEN_MODEWRITE",
            "FOPEN_MODEAPPEND",
            "FOPEN_MODEPLUS",
            "FOPEN_MODEBINARY",
            "FOPEN_MODETEXT",
            "E_OPENPATH",
            "PATH_GENERIC",
            "PATH_BOOTPATH",
            "_TASKINFO",
            "TWINCAT_SYSTEMINFOVARLIST",
            "DRAND",
            "MEMCPY",
            "MEMSET",
            "MEMCMP",
            "MEMMOVE",
        ],
    ),
    (
        "Tc2_Utilities",
        &[
            "E_ENUMCMDTYPE",
            "EENUMCMD_FIRST",
            "EENUMCMD_NEXT",
            "EENUMCMD_ABORT",
            "DEFAULT_CSV_FIELD_SEP",
            "DEFAULT_CSV_RECORD_SEP",
            "FB_MEMRINGBUFFER",
            "MEM_RING_BUFFER_INTERNAL_USE_PER_DATA_RECORD",
            "FB_LOCALSYSTEMTIME",
            "FB_GETTIMEZONEINFORMATION",
            "FB_SYSTEMTIMETOTZSPECIFICLOCALTIME",
            "FB_TZSPECIFICLOCALTIMETOFILETIME",
            "FB_TZSPECIFICLOCALTIMETOSYSTEMTIME",
            "FB_FILETIMETOTZSPECIFICLOCALTIME",
            "SYSTEMTIME_TO_STRING",
            "SYSTEMTIME_TO_FILETIME",
            "FILETIME_TO_SYSTEMTIME",
            "SYSTEMTIME_TO_DT",
            "DT_TO_SYSTEMTIME",
            "T_FILETIME",
            "TIMESTRUCT",
            "ST_TIMEZONEINFORMATION",
            "NT_GETTIME",
            "NT_SETLOCALTIME",
            "NT_SETTIMETORTCTIME",
            "FB_FORMATSTRING",
            "F_TOLCASE",
            "F_TOUCASE",
            "F_LTRIM",
            "F_RTRIM",
            "FB_STRINGRINGBUFFER",
            "FB_MEMBUFFERMERGE",
            "FB_MEMRINGBUFFER",
            "FB_ENUMFINDFILELIST",
            "FB_CREATEDIR",
            "FB_REMOVEDIR",
            "BYTE_TO_HEXSTR",
            "WORD_TO_HEXSTR",
            "DWORD_TO_HEXSTR",
            "DWORD_TO_BINSTR",
            "DEFAULT_ADS_TIMEOUT",
        ],
    ),
    (
        "Tc2_MC2",
        &[
            "MC_AXISSTATE_UNDEFINED",
            "MC_AXISSTATE_DISABLED",
            "MC_AXISSTATE_STANDSTILL",
            "MC_AXISSTATE_ERRORSTOP",
            "MC_AXISSTATE_STOPPING",
            "MC_AXISSTATE_HOMING",
            "MC_AXISSTATE_DISCRETEMOTION",
            "MC_AXISSTATE_CONTINOUSMOTION",
            "MC_AXISSTATE_SYNCHRONIZEDMOTION",
            "MC_AXISPARAMETER",
            "MC_DIRECT",
            "MC_RESETCALIBRATION",
            "MC_DEFAULTHOMING",
            "MC_DISABLEMODE",
            "DISABLEMODEHOLD",
            "DISABLEMODEBRAKE",
            "AXIS_REF",
            "MC_POWER",
            "MC_RESET",
            "MC_STOP",
            "MC_HALT",
            "MC_HOME",
            "MC_MOVEABSOLUTE",
            "MC_MOVERELATIVE",
            "MC_MOVEADDITIVE",
            "MC_MOVEVELOCITY",
            "MC_MOVEMODULO",
            "MC_JOG",
            "MC_READSTATUS",
            "MC_READACTUALPOSITION",
            "MC_READACTUALVELOCITY",
            "MC_READAXISERROR",
            "MC_READPARAMETER",
            "MC_WRITEPARAMETER",
            "MC_SETPOSITION",
            "MC_GEARIN",
            "MC_GEAROUT",
            "MC_DIRECTION",
            "MC_BUFFERMODE",
            "MC_HOMINGMODE",
            "MC_AXISSTATES",
            "MC_ABORTING",
            "MC_BUFFERED",
            "MC_POSITIVE_DIRECTION",
            "MC_NEGATIVE_DIRECTION",
            "ST_AXISSTATUS",
            "E_JOGMODE",
        ],
    ),
    (
        "Tc2_EtherCAT",
        &[
            "EC_MAX_SLAVES",
            "FB_ECCOESDOREAD",
            "FB_ECCOESDOWRITE",
            "FB_ECGETALLSLAVESTATES",
            "FB_ECGETSLAVECOUNT",
            "FB_ECGETSLAVETOPOLOGYINFO",
            "FB_ECGETMASTERSTATE",
            "FB_ECREQMASTERSTATE",
            "FB_ECREQSLAVESTATE",
        ],
    ),
    (
        "Tc3_EventLogger",
        &[
            "FB_TCMESSAGE",
            "FB_TCALARM",
            "FB_TCSOURCEINFO",
            "FB_LISTENERBASE2",
            "I_TCSOURCEINFO",
            "I_TCMESSAGE",
            "I_TCARGUMENTS",
            "TCEVENTENTRY",
            "TCEVENTSEVERITY",
            "TC_EVENTS",
        ],
    ),
    (
        "Tc2_Math",
        &["FRAC", "LMOD", "MODABS", "MODTURNS", "F_GETVERSIONTCMATH"],
    ),
    ("Tc2_Standard (its LTIME timers)", &["LTON", "LTOF", "LTP"]),
    (
        "TcUnit (an open-source library: pass its .plcproj after the application's)",
        &[
            "TEST",
            "TEST_FINISHED",
            "TEST_FINISHED_NAMED",
            "TEST_ORDERED",
            "FB_TESTSUITE",
            "ASSERTTRUE",
            "ASSERTFALSE",
            "ASSERTEQUALS",
            "ASSERTEQUALS_BOOL",
            "ASSERTEQUALS_INT",
            "ASSERTEQUALS_DINT",
            "ASSERTEQUALS_REAL",
            "ASSERTEQUALS_LREAL",
            "ASSERTEQUALS_STRING",
            "ASSERTEQUALS_TIME",
            "ASSERTEQUALS_UDINT",
        ],
    ),
];

/// Beckhoff library namespaces (`Tc2_Utilities.X`): `Tc2_*`, `Tc3_*`.
fn is_library_namespace(name: &str) -> bool {
    let u = name.to_uppercase();
    u.starts_with("TC2_") || u.starts_with("TC3_")
}

/// The library a name that plcc does not define comes from, if it is a
/// well-known one. A namespace name itself (`Tc2_MC2`) is its own library.
pub fn library_of(name: &str) -> Option<String> {
    let bare = name.rsplit('.').next().unwrap_or(name);
    let u = bare.to_uppercase();
    for (lib, names) in LIBRARIES {
        if names.contains(&u.as_str()) {
            return Some(lib.to_string());
        }
    }
    if let Some((ns, _)) = name.split_once('.')
        && is_library_namespace(ns)
    {
        return Some(ns.to_string());
    }
    if is_library_namespace(bare) {
        return Some(bare.to_string());
    }
    None
}

/// Whether plcc provides (a compatible subset of) the library `name`, as
/// referenced by a TwinCAT project.
pub fn provided(name: &str) -> bool {
    matches!(
        name.to_uppercase().as_str(),
        "TC2_STANDARD" | "STANDARD" | "TC2_SYSTEM_BASE"
    )
}
