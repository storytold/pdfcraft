//! A fake eSCL scanner on loopback, for tests here and in the crates above (feature
//! `fake-escl`). It has a flatbed and a duplex feeder, answers like a real device and records
//! every request.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// One request the fake scanner received: method, path and body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: String,
}

#[derive(Debug, Default)]
struct State {
    requests: Vec<Request>,
    /// Pages left in the feeder.
    feeder: usize,
    /// Pages the current job still has to deliver.
    pending: usize,
    /// The last job's size in pixels.
    size: (u32, u32),
    gray: bool,
}

/// A running fake scanner. It stops when the test process exits.
pub struct FakeEscl {
    /// `http://127.0.0.1:<port>/eSCL`.
    pub base: String,
    state: Arc<Mutex<State>>,
}

pub const CAPABILITIES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<scan:ScannerCapabilities xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm">
  <pwg:Version>2.63</pwg:Version>
  <pwg:MakeAndModel>PdfCraft Test Scanner</pwg:MakeAndModel>
  <scan:Platen>
    <scan:PlatenInputCaps>
      <scan:MinWidth>16</scan:MinWidth>
      <scan:MaxWidth>2550</scan:MaxWidth>
      <scan:MinHeight>16</scan:MinHeight>
      <scan:MaxHeight>3508</scan:MaxHeight>
      <scan:SettingProfiles>
        <scan:SettingProfile>
          <scan:ColorModes>
            <scan:ColorMode>BlackAndWhite1</scan:ColorMode>
            <scan:ColorMode>Grayscale8</scan:ColorMode>
            <scan:ColorMode>RGB24</scan:ColorMode>
          </scan:ColorModes>
          <scan:DocumentFormats>
            <pwg:DocumentFormat>image/jpeg</pwg:DocumentFormat>
            <pwg:DocumentFormat>application/pdf</pwg:DocumentFormat>
            <scan:DocumentFormatExt>image/png</scan:DocumentFormatExt>
          </scan:DocumentFormats>
          <scan:SupportedResolutions>
            <scan:DiscreteResolutions>
              <scan:DiscreteResolution><scan:XResolution>75</scan:XResolution><scan:YResolution>75</scan:YResolution></scan:DiscreteResolution>
              <scan:DiscreteResolution><scan:XResolution>150</scan:XResolution><scan:YResolution>150</scan:YResolution></scan:DiscreteResolution>
              <scan:DiscreteResolution><scan:XResolution>300</scan:XResolution><scan:YResolution>300</scan:YResolution></scan:DiscreteResolution>
              <scan:DiscreteResolution><scan:XResolution>600</scan:XResolution><scan:YResolution>600</scan:YResolution></scan:DiscreteResolution>
            </scan:DiscreteResolutions>
          </scan:SupportedResolutions>
        </scan:SettingProfile>
      </scan:SettingProfiles>
    </scan:PlatenInputCaps>
  </scan:Platen>
  <scan:Adf>
    <scan:AdfSimplexInputCaps>
      <scan:MaxWidth>2550</scan:MaxWidth>
      <scan:MaxHeight>4200</scan:MaxHeight>
      <scan:SettingProfiles><scan:SettingProfile>
        <scan:ColorModes><scan:ColorMode>Grayscale8</scan:ColorMode><scan:ColorMode>RGB24</scan:ColorMode></scan:ColorModes>
        <scan:DocumentFormats><pwg:DocumentFormat>image/jpeg</pwg:DocumentFormat><scan:DocumentFormatExt>image/png</scan:DocumentFormatExt></scan:DocumentFormats>
        <scan:SupportedResolutions><scan:ResolutionRange>
          <scan:XResolutionRange><scan:Min>75</scan:Min><scan:Max>300</scan:Max><scan:Step>75</scan:Step></scan:XResolutionRange>
          <scan:YResolutionRange><scan:Min>75</scan:Min><scan:Max>300</scan:Max><scan:Step>75</scan:Step></scan:YResolutionRange>
        </scan:ResolutionRange></scan:SupportedResolutions>
      </scan:SettingProfile></scan:SettingProfiles>
    </scan:AdfSimplexInputCaps>
    <scan:AdfOptions><scan:AdfOption>Duplex</scan:AdfOption></scan:AdfOptions>
  </scan:Adf>
</scan:ScannerCapabilities>
"#;

fn tag(body: &str, name: &str) -> Option<String> {
    let start = body.find(&format!("{name}>"))? + name.len() + 1;
    let end = body.get(start..)?.find('<')? + start;
    body.get(start..end).map(|s| s.trim().to_string())
}

/// A gray PNG of `w × h` pixels with a black frame (so a page is visibly there).
fn page_png(w: u32, h: u32, gray: bool) -> Vec<u8> {
    let (w, h) = (w.clamp(1, 4000), h.clamp(1, 6000));
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(if gray { png::ColorType::Grayscale } else { png::ColorType::Rgb });
    enc.set_depth(png::BitDepth::Eight);
    let channels = if gray { 1 } else { 3 };
    let mut data = Vec::with_capacity((w * h) as usize * channels);
    for y in 0..h {
        for x in 0..w {
            let edge = x < 4 || y < 4 || x + 4 >= w || y + 4 >= h;
            for c in 0..channels {
                data.push(if edge {
                    0
                } else if c == 0 {
                    230
                } else {
                    200
                });
            }
        }
    }
    if let Ok(mut writer) = enc.write_header() {
        let _ = writer.write_image_data(&data);
        let _ = writer.finish();
    }
    out
}

fn respond(stream: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn handle(mut stream: TcpStream, state: &Mutex<State>, port: u16) {
    let Ok(clone) = stream.try_clone() else { return };
    let mut reader = BufReader::new(clone);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; len.min(1 << 20)];
    let _ = reader.read_exact(&mut body);
    let body = String::from_utf8_lossy(&body).into_owned();
    let Ok(mut st) = state.lock() else { return };
    st.requests.push(Request { method: method.clone(), path: path.clone(), body: body.clone() });
    match (method.as_str(), path.as_str()) {
        ("GET", "/eSCL/ScannerCapabilities") => respond(&mut stream, "200 OK", &[("Content-Type", "text/xml".into())], CAPABILITIES.as_bytes()),
        ("GET", "/eSCL/ScannerStatus") => {
            let adf = if st.feeder > 0 { "ScannerAdfLoaded" } else { "ScannerAdfEmpty" };
            let xml = format!(
                "<scan:ScannerStatus xmlns:scan=\"s\" xmlns:pwg=\"p\"><pwg:State>Idle</pwg:State><scan:AdfState>{adf}</scan:AdfState></scan:ScannerStatus>"
            );
            respond(&mut stream, "200 OK", &[("Content-Type", "text/xml".into())], xml.as_bytes());
        }
        ("POST", "/eSCL/ScanJobs") => {
            let feeder = tag(&body, "InputSource").as_deref() == Some("Feeder");
            let dpi: u32 = tag(&body, "XResolution").and_then(|d| d.parse().ok()).unwrap_or(75);
            let w: u32 = tag(&body, "Width").and_then(|d| d.parse().ok()).unwrap_or(2550);
            let h: u32 = tag(&body, "Height").and_then(|d| d.parse().ok()).unwrap_or(3300);
            if feeder && st.feeder == 0 {
                respond(&mut stream, "409 Conflict", &[], b"");
                return;
            }
            let duplex = tag(&body, "Duplex").as_deref() == Some("true");
            st.pending = if feeder { st.feeder * if duplex { 2 } else { 1 } } else { 1 };
            if feeder {
                st.feeder = 0;
            }
            st.size = (w * dpi / 300, h * dpi / 300);
            st.gray = tag(&body, "ColorMode").is_some_and(|m| m != "RGB24");
            // Absolute, as most devices answer.
            respond(&mut stream, "201 Created", &[("Location", format!("http://127.0.0.1:{port}/eSCL/ScanJobs/1"))], b"");
        }
        ("GET", "/eSCL/ScanJobs/1/NextDocument") => {
            if st.pending == 0 {
                respond(&mut stream, "404 Not Found", &[], b"");
            } else {
                st.pending -= 1;
                let png = page_png(st.size.0, st.size.1, st.gray);
                respond(&mut stream, "200 OK", &[("Content-Type", "image/png".into())], &png);
            }
        }
        ("DELETE", "/eSCL/ScanJobs/1") => {
            st.pending = 0;
            respond(&mut stream, "200 OK", &[], b"");
        }
        _ => respond(&mut stream, "404 Not Found", &[], b""),
    }
}

impl FakeEscl {
    /// Start a fake scanner with `feeder_pages` sheets in its document feeder. `None` if no
    /// loopback port could be opened.
    pub fn start(feeder_pages: usize) -> Option<FakeEscl> {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let port = listener.local_addr().ok()?.port();
        let state = Arc::new(Mutex::new(State { feeder: feeder_pages, ..State::default() }));
        let shared = state.clone();
        std::thread::Builder::new()
            .name("fake-escl".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    handle(stream, &shared, port);
                }
            })
            .ok()?;
        Some(FakeEscl { base: format!("http://127.0.0.1:{port}/eSCL"), state })
    }

    /// The scanner id (`escl:http://127.0.0.1:<port>/eSCL`).
    pub fn id(&self) -> String {
        format!("escl:{}", self.base)
    }

    /// Put `pages` sheets in the feeder.
    pub fn load_feeder(&self, pages: usize) {
        if let Ok(mut st) = self.state.lock() {
            st.feeder = pages;
        }
    }

    /// Every request so far.
    pub fn requests(&self) -> Vec<Request> {
        self.state.lock().map(|s| s.requests.clone()).unwrap_or_default()
    }
}
