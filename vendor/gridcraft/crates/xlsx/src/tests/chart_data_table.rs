//! Synthetic DrawingML only: table flags, default-valued booleans and full XLSX round trips.

use crate::{chart::read_chart, read_xlsx, write_xlsx, xml::parse};
use gridcraft_model::ChartDataTable;

fn chart(table: &str) -> gridcraft_model::Chart {
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea>\
        <c:barChart><c:barDir val=\"col\"/><c:grouping val=\"clustered\"/><c:ser><c:idx val=\"0\"/><c:order val=\"0\"/>\
        <c:val><c:numLit><c:formatCode>General</c:formatCode><c:ptCount val=\"1\"/><c:pt idx=\"0\"><c:v>12</c:v></c:pt></c:numLit></c:val>\
        </c:ser><c:axId val=\"1\"/><c:axId val=\"2\"/></c:barChart><c:catAx/><c:valAx/>{table}</c:plotArea></c:chart></c:chartSpace>"
    );
    read_chart(&parse(xml.as_bytes()).unwrap()).unwrap()
}

#[test]
fn data_table_boolean_defaults_and_absence() {
    assert_eq!(chart("").data_table, None);
    assert_eq!(
        chart("<c:dTable/>").data_table,
        Some(ChartDataTable { show_keys: false, horizontal_border: false, vertical_border: false, outline: false })
    );
    assert_eq!(
        chart("<c:dTable><c:showKeys/><c:showHorzBorder/><c:showVertBorder/><c:showOutline/></c:dTable>").data_table,
        Some(ChartDataTable::default())
    );
}

#[test]
fn every_data_table_flag_combination_survives_xlsx_save_reopen() {
    for flags in 0..16u8 {
        let settings =
            ChartDataTable { show_keys: flags & 1 != 0, horizontal_border: flags & 2 != 0, vertical_border: flags & 4 != 0, outline: flags & 8 != 0 };
        let mut c = chart("");
        c.id = 1;
        c.data_table = Some(settings.clone());
        let mut wb = gridcraft_model::Workbook::new();
        wb.sheet_mut(0).unwrap().charts.push(c);
        let bytes = write_xlsx(&wb).unwrap();
        let (back, _) = read_xlsx(&bytes).unwrap();
        assert_eq!(back.sheets[0].charts[0].data_table.as_ref(), Some(&settings), "flags {flags}");
        let bytes = write_xlsx(&back).unwrap();
        let (again, _) = read_xlsx(&bytes).unwrap();
        assert_eq!(again.sheets[0].charts[0].data_table.as_ref(), Some(&settings));
    }
}
