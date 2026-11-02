//! Original synthetic workflow: no private timesheet, employee, PIN or signature assets.
use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};
use pdfcraft_engine::{Edit, NewField, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn call(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(v) => v,
        _ => panic!("expected JSON"),
    }
}
fn fixture() -> Vec<u8> {
    let mut session = Session::new();
    let bytes = session.create_blank(600.0, 800.0, 1).unwrap();
    let id = session.open("synthetic.pdf", None, bytes, None).unwrap();
    for (i, name) in ["input", "double", "seen", "choice", "go", "source", "target"].into_iter().enumerate() {
        session
            .apply(
                id,
                Edit::AddField {
                    page: 0,
                    rect: [40.0, 700.0 - i as f64 * 45.0, 200.0, 730.0 - i as f64 * 45.0],
                    kind: if i >= 4 { NewField::Button { caption: name.into() } } else { NewField::Text { multiline: false } },
                    name: Some(name.into()),
                },
            )
            .unwrap();
    }
    let mut doc = Document::open(session.save_bytes(id).unwrap()).unwrap();
    let source = session.get(id).unwrap().form.iter().find(|f| f.name == "source").unwrap().obj;
    let mut gs = Dict::new();
    gs.set(b"ca".to_vec(), Object::Real(0.5));
    let mut states = Dict::new();
    states.set(b"Half".to_vec(), Object::Dict(gs));
    let mut resources = Dict::new();
    resources.set(b"ExtGState".to_vec(), Object::Dict(states));
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("XObject"));
    dict.set(b"Subtype".to_vec(), Object::name("Form"));
    dict.set(b"BBox".to_vec(), Object::Array([0, 0, 60, 30].map(Object::Int).to_vec()));
    dict.set(b"Resources".to_vec(), Object::Dict(resources));
    let icon = doc.add(Object::Stream(Stream::from_raw(dict, b"q /Half gs 1 0 0 rg 0 0 60 30 re f Q".to_vec())));
    let mut mk = Dict::new();
    mk.set(b"I".to_vec(), Object::Ref(icon));
    mk.set(b"TP".to_vec(), Object::Int(1));
    doc.update_dict(source, |d| d.set(b"MK".to_vec(), Object::Dict(mk))).unwrap();
    write_full(&doc, &SaveOptions::default()).unwrap()
}
const HELPERS: &str = r#"
var initialized=1, buffer='', savedIcon=null;
function openPad(){var b=getPageBox('Crop',0);var f=getField('go');f.rect=[b[0]+40,b[1]-400,b[0]+200,b[1]-450];f.textSize=16;f.fillColor=['RGB',0.9,0.8,0.7];f.strokeColor=['G',0.4];f.lineWidth=2;f.borderStyle='dashed';f.textColor=color.blue;f.userName='Synthetic keypad';f.display=display.noPrint;}
function digit(d){buffer+=d;getField('go').buttonSetCaption(buffer);if(buffer==='1234'){getField('target').buttonSetIcon(savedIcon);getField('go').display=display.hidden;}}
function selectTime(){var v=app.popUpMenuEx({cName:'Unavailable',cReturn:'bad',bEnabled:false},{cName:'Times',oSubMenu:[{cName:'Seven',cReturn:'07:00',bMarked:true},{cName:'Manual',cReturn:'manual'}]});if(v==='manual')v=app.response({cQuestion:'Time',cDefault:'07:00'});if(v!==null)getField('choice').value=v;}
"#;

#[test]
fn timesheet_keypad_geometry_icons_calculation_and_persistence() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-timesheet-compat-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("synthetic.pdf"), fixture()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let id = call(&mut a, "doc_open", json!({"path":"synthetic.pdf"}))["doc"].as_u64().unwrap();
    call(&mut a, "form_set_script", json!({"doc":id,"field":"double","event":"calculate","script":"event.value=Number(getField('input').value)*2;"}));
    call(&mut a, "js_set_document_script", json!({"doc":id,"name":"helpers","script":HELPERS}));
    let out = call(
        &mut a,
        "js_run",
        json!({"doc":id,"script":"getField('input').value=3;calculateNow();getField('seen').value=getField('double').value;getField('seen').value;"}),
    );
    assert_eq!(out["error"], Value::Null);
    assert_eq!(out["result"], "6");
    let out = call(&mut a, "js_run", json!({"doc":id,"script":"openPad();savedIcon=getField('source').buttonGetIcon();initialized;"}));
    assert_eq!(out["error"], Value::Null);
    assert_eq!(out["result"], "1");
    let fields = call(&mut a, "form_fields", json!({"doc":id}));
    let go = fields["fields"].as_array().unwrap().iter().find(|f| f["name"] == "go").unwrap();
    assert_eq!(go["tooltip"], "Synthetic keypad");
    let model = a.session().get(pdfcraft_engine::DocId(id)).unwrap().form.iter().find(|f| f.name == "go").unwrap();
    assert_eq!(model.widgets[0].rect, [40.0, 350.0, 200.0, 400.0]);
    assert!(model.da.contains("16 Tf"), "{}", model.da);
    for (key, expected) in [("1", "1"), ("2", "12"), ("3", "123")] {
        let out = call(&mut a, "js_run", json!({"doc":id,"field":"go","script":format!("digit('{key}');buffer;")}));
        assert_eq!(out["error"], Value::Null);
        assert_eq!(out["result"], expected);
    }
    let (_, _, before) = a.render_pixels(id, 1, 1.0).unwrap();
    let out = call(&mut a, "js_run", json!({"doc":id,"field":"go","script":"digit('4');buffer;"}));
    assert_eq!(out["error"], Value::Null);
    assert_eq!(out["result"], "1234");
    let (_, _, after) = a.render_pixels(id, 1, 1.0).unwrap();
    assert_ne!(before, after);
    let blended = after
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > p[1].saturating_add(50) && (50..180).contains(&p[1]) && (p[1] as i16 - p[2] as i16).abs() <= 2)
        .count();
    assert!(blended > 500, "half-opacity red icon must blend across its area with the button's grey background, got {blended} pixels");
    call(&mut a, "edit_undo", json!({"doc":id}));
    assert_eq!(a.render_pixels(id, 1, 1.0).unwrap().2, before);
    call(&mut a, "edit_redo", json!({"doc":id}));
    assert_eq!(a.render_pixels(id, 1, 1.0).unwrap().2, after);
    let rejected = call(&mut a, "js_run", json!({"doc":id,"script":"getField('target').buttonSetIcon({});"}));
    assert!(rejected["error"].as_str().unwrap().contains("icon"));
    call(&mut a, "doc_save", json!({"doc":id,"path":"saved.pdf"}));
    let saved = Document::open(Arc::new(std::fs::read(dir.join("saved.pdf")).unwrap())).unwrap();
    let root = saved.get(saved.root().unwrap());
    let af = saved.resolve(root.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = saved.resolve(af.as_dict().unwrap().get(b"Fields").unwrap());
    let target = fields
        .as_array()
        .unwrap()
        .iter()
        .map(|f| saved.resolve(f))
        .find(|f| f.as_dict().unwrap().get(b"T").and_then(Object::as_string).is_some_and(|s| s.to_text() == "target"))
        .unwrap();
    let mk = saved.resolve(target.as_dict().unwrap().get(b"MK").unwrap());
    let icon = saved.resolve(mk.as_dict().unwrap().get(b"I").unwrap());
    let Object::Stream(icon) = &*icon else { panic!("icon stream") };
    assert!(String::from_utf8_lossy(&icon.decoded().unwrap()).contains("/Half gs"));
    let reopened = call(&mut a, "doc_open", json!({"path":"saved.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(a.render_pixels(reopened, 1, 1.0).unwrap().2, after);
    let reset = call(&mut a, "js_run", json!({"doc":reopened,"script":"buffer;"}));
    assert_eq!(reset["result"], "");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn popup_and_manual_response_resume_the_same_script_and_validate_choices() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-timesheet-menu-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("synthetic.pdf"), fixture()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let id = call(&mut a, "doc_open", json!({"path":"synthetic.pdf"}))["doc"].as_u64().unwrap();
    call(&mut a, "js_set_document_script", json!({"doc":id,"name":"helpers","script":HELPERS}));
    let out = call(&mut a, "js_run", json!({"doc":id,"script":"selectTime();getField('choice').value;","answers":["manual","07:45"]}));
    assert_eq!(out["error"], Value::Null);
    assert_eq!(out["result"], "07:45");
    let cancel = call(&mut a, "js_run", json!({"doc":id,"script":"selectTime();getField('choice').value;","answers":[null]}));
    assert_eq!(cancel["result"], "07:45");
    let invalid = call(&mut a, "js_run", json!({"doc":id,"script":"selectTime();","answers":["bad"]}));
    assert!(invalid["error"].as_str().unwrap().contains("invalid popup"));
    let alert = call(&mut a, "js_run", json!({"doc":id,"script":"app.alert({cMsg:'Continue?',nType:2});","answers":["4"]}));
    assert_eq!(alert["result"], "4");
    std::fs::remove_dir_all(dir).unwrap();
}
