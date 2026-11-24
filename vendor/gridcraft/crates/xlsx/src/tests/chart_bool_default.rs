//! A `CT_Boolean` chart flag with no `val` attribute is on (ECMA-376), for series smoothing and value labels.

use crate::chart::read_chart;
use crate::xml::parse;

/// Reads a line chart with `ser` appended to its one series and `group` inside the group's `<c:dLbls>`.
fn flags(ser: &str, group: &str) -> (bool, bool) {
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea><c:lineChart>\
         <c:grouping val=\"standard\"/><c:ser><c:idx val=\"0\"/><c:order val=\"0\"/>\
         <c:val><c:numRef><c:f>Data!$A$1:$A$5</c:f></c:numRef></c:val>{ser}</c:ser><c:dLbls>{group}</c:dLbls>\
         <c:axId val=\"1\"/><c:axId val=\"2\"/></c:lineChart><c:catAx><c:axId val=\"1\"/></c:catAx><c:valAx><c:axId val=\"2\"/></c:valAx>\
         </c:plotArea></c:chart></c:chartSpace>"
    );
    let root = parse(xml.as_bytes()).unwrap();
    let ch = read_chart(&root).unwrap();
    (ch.series[0].smooth, ch.data_labels)
}

#[test]
fn omitted_val_means_on() {
    assert_eq!(flags("<c:smooth/>", "<c:showVal/>"), (true, true));
    assert_eq!(flags("<c:dLbls><c:showVal/></c:dLbls>", ""), (false, true), "series-level value labels");
}

#[test]
fn explicit_values_are_honoured() {
    assert_eq!(flags("<c:smooth val=\"1\"/>", "<c:showVal val=\"1\"/>"), (true, true));
    assert_eq!(flags("<c:smooth val=\"true\"/>", "<c:showVal val=\"true\"/>"), (true, true));
    assert_eq!(flags("<c:smooth val=\"0\"/>", "<c:showVal val=\"0\"/>"), (false, false));
    assert_eq!(flags("", ""), (false, false));
}
