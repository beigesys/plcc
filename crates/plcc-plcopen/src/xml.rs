// SPDX-License-Identifier: MPL-2.0

//! Small helpers over `roxmltree`. PLCopen files come with the TC6 namespace
//! (`tc6_0200`, `tc6_0201`) or none at all, so elements are matched by local
//! name only.

use crate::embed::Fragment;
use plcc_st::Span;
use roxmltree::Node;

pub(crate) type XNode<'a, 'i> = Node<'a, 'i>;

pub(crate) fn name<'a>(n: XNode<'a, '_>) -> &'a str {
    n.tag_name().name()
}

pub(crate) fn span(n: XNode) -> Span {
    Span::from(n.range())
}

/// Span of just the start tag (`<contact ... >`), which reads better in a
/// diagnostic than the whole element with all its children.
pub(crate) fn tag_span(src: &str, n: XNode) -> Span {
    let r = n.range();
    let end = src[r.clone()].find('>').map_or(r.end, |p| r.start + p + 1);
    Span::new(r.start, end)
}

pub(crate) fn elements<'a, 'i>(n: XNode<'a, 'i>) -> impl Iterator<Item = XNode<'a, 'i>> {
    n.children().filter(|c| c.is_element())
}

pub(crate) fn child<'a, 'i>(n: XNode<'a, 'i>, local: &str) -> Option<XNode<'a, 'i>> {
    elements(n).find(|c| name(*c) == local)
}

pub(crate) fn children<'a, 'i>(
    n: XNode<'a, 'i>,
    local: &'static str,
) -> impl Iterator<Item = XNode<'a, 'i>> {
    elements(n).filter(move |c| name(*c) == local)
}

pub(crate) fn attr<'a>(n: XNode<'a, '_>, a: &str) -> Option<&'a str> {
    n.attribute(a)
}

/// `true` / `1` (xsd:boolean).
pub(crate) fn attr_bool(n: XNode, a: &str) -> bool {
    matches!(n.attribute(a).map(str::trim), Some("true" | "1"))
}

/// An attribute value as an embeddable fragment (entities decoded, mapped).
pub(crate) fn attr_fragment(src: &str, n: XNode, a: &str) -> Option<Fragment> {
    let at = n.attribute_node(a)?;
    Some(Fragment::decode(src, at.range_value()))
}

/// Span of an attribute's value, or of the element's start tag without one.
pub(crate) fn attr_span(src: &str, n: XNode, a: &str) -> Span {
    n.attribute_node(a)
        .map(|at| Span::from(at.range_value()))
        .unwrap_or_else(|| tag_span(src, n))
}

/// The raw content of an element (between its start and end tags) as a fragment.
pub(crate) fn content_fragment(src: &str, n: XNode) -> Fragment {
    let r = n.range();
    let Some(first) = n.first_child() else {
        return Fragment::decode(src, r.end..r.end);
    };
    let start = first.range().start;
    let end = src[r.clone()].rfind("</").map_or(r.end, |p| r.start + p).max(start);
    Fragment::decode(src, start..end)
}

/// `<position x=".." y=".."/>` of an element, (0, 0) when absent.
pub(crate) fn position(n: XNode) -> (f64, f64) {
    child(n, "position")
        .map(|p| {
            let f = |a| p.attribute(a).and_then(|v| v.trim().parse().ok()).unwrap_or(0.0);
            (f("x"), f("y"))
        })
        .unwrap_or((0.0, 0.0))
}
