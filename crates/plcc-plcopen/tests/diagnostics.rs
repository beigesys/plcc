// SPDX-License-Identifier: MPL-2.0

//! Malformed and unsupported input: clear messages, spans on the XML element.

mod common;
use common::*;

fn errors_of(src: &str) -> Vec<(String, String)> {
    let (_, errors) = plcc_plcopen::parse(src);
    errors
        .into_iter()
        .map(|e| {
            let (s, l) = (e.span.offset(), e.span.len());
            (e.message, src[s..s + l].to_string())
        })
        .collect()
}

/// A project with one POU of `pou_type` whose interface is `iface` and whose
/// body is `body` (the language element included).
fn project(pou_type: &str, iface: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<project xmlns="http://www.plcopen.org/xml/tc6_0201">
  <types><dataTypes/><pous>
    <pou name="P" pouType="{pou_type}">
      <interface>{iface}</interface>
      <body>{body}</body>
    </pou>
  </pous></types>
</project>"#
    )
}

#[test]
fn bad_wiring_is_reported_per_element() {
    let src = read_fixture("errors/ld_bad_wiring.xml");
    let errs = errors_of(&src);
    let has = |msg: &str, at: &str| errs.iter().any(|(m, s)| m.contains(msg) && s.contains(at));
    assert!(has("unknown localId 99", "refLocalId=\"99\""), "{errs:#?}");
    assert!(has("incomplete contact variable", "A AND"), "{errs:#?}");
    assert!(
        has(
            "unsupported graphical element <vendorElement>",
            "<vendorElement"
        ),
        "{errs:#?}"
    );
}

#[test]
fn sfc_is_not_supported_yet() {
    let src = read_fixture("errors/sfc_body.xml");
    let errs = errors_of(&src);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert!(errs[0].0.contains("SFC bodies are not supported yet"));
    assert_eq!(errs[0].1, "<SFC>");
}

#[test]
fn sfc_elements_inside_ld_are_rejected() {
    let src = project(
        "program",
        "",
        r#"<LD><step localId="1" name="S"><position x="0" y="0"/></step></LD>"#,
    );
    let errs = errors_of(&src);
    assert!(
        errs[0]
            .0
            .contains("SFC element <step> is not supported yet"),
        "{errs:#?}"
    );
}

#[test]
fn malformed_xml_and_wrong_root() {
    let errs = errors_of("<project><types></project>");
    assert!(errs[0].0.starts_with("malformed XML"), "{errs:#?}");
    let errs = errors_of("<?xml version=\"1.0\"?><svg/>");
    assert!(errs[0].0.contains("not a PLCopen XML project"), "{errs:#?}");
}

#[test]
fn st_body_syntax_errors_point_into_the_xml() {
    let src = project("program", "", "<ST><![CDATA[x := ;]]></ST>");
    let errs = errors_of(&src);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    let at = src.find("x := ;").unwrap() + 5;
    let (_, errors) = plcc_plcopen::parse(&src);
    assert_eq!(
        errors[0].span.offset(),
        at,
        "error at the `;` inside the CDATA"
    );
}

#[test]
fn undeclared_variable_error_carries_the_xml_offset() {
    let src = read_fixture("errors/ld_undeclared.xml");
    let unit = with_stdlib(lower("errors/ld_undeclared.xml"));
    let ctx = inkwell::context::Context::create();
    let mut c = plcc_codegen::Compiler::new(&ctx, "undeclared");
    let err = c
        .compile(&unit)
        .expect_err("Nope is undeclared")
        .to_string();
    let at = src.find("Nope</variable>").unwrap();
    assert!(err.contains("Nope"), "{err}");
    assert!(err.contains(&format!("source offset {at}")), "{err}");
}

#[test]
fn edge_contact_in_a_function_needs_state() {
    let src = project(
        "function",
        r#"<returnType><BOOL/></returnType>
           <inputVars><variable name="a"><type><BOOL/></type></variable></inputVars>"#,
        r#"<LD>
            <leftPowerRail localId="1"><position x="0" y="0"/></leftPowerRail>
            <contact localId="2" edge="rising"><position x="10" y="0"/>
              <connectionPointIn><connection refLocalId="1"/></connectionPointIn>
              <variable>a</variable></contact>
            <coil localId="3"><position x="20" y="0"/>
              <connectionPointIn><connection refLocalId="2"/></connectionPointIn>
              <variable>P</variable></coil>
           </LD>"#,
    );
    let errs = errors_of(&src);
    assert!(
        errs.iter()
            .any(|(m, s)| m.contains("edge detection needs state") && s.starts_with("<contact")),
        "{errs:#?}"
    );
}

#[test]
fn jump_to_undefined_label_and_unconnected_coil() {
    let src = project(
        "program",
        r#"<localVars><variable name="q"><type><BOOL/></type></variable></localVars>"#,
        r#"<LD>
            <leftPowerRail localId="1"><position x="0" y="0"/></leftPowerRail>
            <jump localId="2" label="Nowhere"><position x="10" y="0"/>
              <connectionPointIn><connection refLocalId="1"/></connectionPointIn></jump>
            <coil localId="3"><position x="10" y="50"/><variable>q</variable></coil>
           </LD>"#,
    );
    let errs = errors_of(&src);
    assert!(
        errs.iter()
            .any(|(m, _)| m.contains("jump to undefined label `Nowhere`")),
        "{errs:#?}"
    );
    assert!(
        errs.iter().any(
            |(m, s)| m.contains("coil (localId 3): input is not connected")
                && s.starts_with("<coil")
        ),
        "{errs:#?}"
    );
}

#[test]
fn struct_initial_values_are_reported_not_dropped() {
    let src = project(
        "program",
        r#"<localVars><variable name="s"><type><derived name="T"/></type>
             <initialValue><structValue><value member="a"><simpleValue value="1"/></value></structValue></initialValue>
           </variable></localVars>"#,
        "<ST><![CDATA[;]]></ST>",
    );
    let errs = errors_of(&src);
    assert!(errs[0].0.contains("<structValue>"), "{errs:#?}");
}

#[test]
fn hidden_variables_are_declared_on_the_pou() {
    let unit = lower("ld_coils_edges.xml");
    let plcc_st::Declaration::Program(p) = &unit.declarations[0] else {
        panic!()
    };
    let hidden: Vec<(String, String)> = p
        .var_blocks
        .iter()
        .flat_map(|b| b.declarations.iter())
        .filter(|d| d.name.name.starts_with("_ld_"))
        .map(|d| {
            let plcc_st::ast::TypeSpecKind::Named(t) = &d.type_spec.kind else {
                panic!()
            };
            (d.name.name.clone(), t.name.clone())
        })
        .collect();
    let has = |n: &str, t: &str| hidden.iter().any(|(a, b)| a == n && b == t);
    assert!(has("_ld_rt40", "R_TRIG"), "{hidden:?}");
    assert!(has("_ld_ft50", "F_TRIG"), "{hidden:?}");
    assert!(has("_ld_rt61", "R_TRIG"), "{hidden:?}");
    assert!(has("_ld_ft71", "F_TRIG"), "{hidden:?}");
}
