//! Synthetic fixtures for tests in this and other crates (never real forms).

#![doc(hidden)]

/// A small dynamic form: a letter page master with a page-number draw, a title, a text field
/// with a caption on top, a check box, a radio group beside a question, a date field, a table
/// with a repeated header row and `rows` data rows, and a last section that starts a new page.
pub fn template(rows: usize) -> String {
    let mut t = String::from(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<config xmlns="http://www.xfa.org/schema/xci/3.0/"><present><pdf><version>1.7</version></pdf></present><template><base/></template></config>
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb" locale="en_US">
 <pageSet>
  <pageArea name="front" id="front"><occur min="1" max="1"/>
   <contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/>
   <medium stock="letter" short="8.5in" long="11in"/>
   <draw name="pn" x="7in" y="10.6in" w="1in" h="0.2in"><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p>Page <span xfa:embed="#pageNo" xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"/> of <span xfa:embed="#pageCount" xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"/></p></body></exData></value><font typeface="Arial" size="8pt"/></draw>
   <field name="pageNo" id="pageNo" presence="hidden" w="1in" h="0.2in"><ui><textEdit/></ui><event activity="ready" ref="$layout"><script contentType="application/x-javascript">this.rawValue = xfa.layout.page(this);</script></event></field>
   <field name="pageCount" id="pageCount" presence="hidden" w="1in" h="0.2in"><ui><textEdit/></ui><event activity="ready" ref="$layout"><script contentType="application/x-javascript">this.rawValue = xfa.layout.pageCount();</script></event></field>
  </pageArea>
  <pageArea name="rest"><occur max="-1"/>
   <contentArea x="0.5in" y="1in" w="7.5in" h="9.5in"/>
   <medium stock="letter" short="8.5in" long="11in"/>
  </pageArea>
 </pageSet>
 <subform name="head" layout="lr-tb">
  <draw name="title" w="7.5in" h="0.4in"><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p style="font-weight:bold">Sample Form</p><p>Second line</p></body></exData></value><font typeface="Arial" size="12pt"/><para hAlign="center"/></draw>
  <draw name="hidden" presence="hidden" w="7.5in" h="3in"><value><text>never shown</text></value></draw>
  <field name="familyName" w="3in" h="0.5in"><ui><textEdit><border><edge presence="hidden"/><edge presence="hidden"/><edge/><edge presence="hidden"/></border></textEdit></ui><font typeface="Courier New" size="9pt"/><caption placement="top" reserve="0.2in"><value><text>Family name</text></value><font typeface="Arial" size="7pt"/></caption><assist><toolTip>Your family name</toolTip></assist><value><text maxChars="30"/></value></field>
  <field name="agree" w="2in" h="0.25in"><ui><checkButton><border><edge/><fill/></border></checkButton></ui><caption placement="right" reserve="1.5in"><value><text>I agree</text></value></caption><items><integer>1</integer><integer>0</integer></items></field>
  <field name="born" w="2in" h="0.5in"><ui><dateTimeEdit/><picture>date{YYYY-MM-DD}</picture></ui><caption placement="top" reserve="0.2in"><value><text>Date of birth</text></value></caption><value><date/></value></field>
  <draw name="q" w="6.5in" h="0.25in"><value><text>Have you ever?</text></value></draw>
  <exclGroup name="answer" layout="lr-tb">
   <field name="yes" w="0.5in" h="0.25in"><ui><checkButton/></ui><items><text>Y</text></items></field>
   <field name="no" w="0.5in" h="0.25in"><ui><checkButton/></ui><items><text>N</text></items></field>
  </exclGroup>
  <field name="go" w="1in" h="0.3in"><ui><button/></ui><caption><value><text>Reset</text></value></caption><border><edge stroke="raised"/><fill><color value="212,208,200"/></fill></border><event activity="click"><script contentType="application/x-javascript">xfa.host.resetData();</script></event></field>
 </subform>
 <subform name="table" layout="table" columnWidths="2in 3in 2.5in">
  <overflow leader="header"/>
  <subform name="header" layout="row"><occur max="-1"/>
   <draw w="2in" h="0.3in"><value><text>From</text></value><border><edge/><fill><color value="200,200,200"/></fill></border></draw>
   <draw h="0.3in"><value><text>Activity</text></value><border><edge/></border></draw>
   <draw h="0.3in"><value><text>Place</text></value><border><edge/></border></draw>
  </subform>
"##,
    );
    for i in 0..rows {
        t.push_str(&format!(
            r##"  <subform name="row" layout="row"><field name="from{i}" h="0.4in"><ui><textEdit/></ui></field><field name="what{i}" h="0.4in"><ui><textEdit/></ui></field><field name="where{i}" h="0.4in"><ui><textEdit/></ui></field></subform>
"##
        ));
    }
    t.push_str(
        r##" </subform>
 <subform name="last" layout="tb"><breakBefore targetType="pageArea"/>
  <draw name="sig" w="7.5in" h="0.3in"><value><text>Signature</text></value></draw>
 </subform>
</subform>
</template>
</xdp:xdp>
"##,
    );
    t
}

/// A PDF shell around `xdp`: one placeholder page, `/NeedsRendering true`, no fields.
pub fn shell(xdp: &str) -> Vec<u8> {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /NeedsRendering true /AcroForm << /XFA 5 0 R /Fields [] >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
        b"<< /Length 44 >>\nstream\nBT /F1 12 Tf 72 700 Td (Please wait...) Tj ET\nendstream".to_vec(),
        format!("<< /Length {} >>\nstream\n{xdp}\nendstream", xdp.len()).into_bytes(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}
