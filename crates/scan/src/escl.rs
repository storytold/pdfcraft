//! eSCL (AirScan, Mopria): network scanners spoken to over plain HTTP with XML.
//!
//! A scan is: `GET ScannerCapabilities` (what the device can do), `POST ScanJobs` with the
//! wanted `ScanSettings` (the answer's `Location` is the job), then `GET <job>/NextDocument`
//! once per page until the device answers 404. The job is deleted when the scan is cancelled
//! or ends on the flatbed. `ScannerStatus` explains a refused job (busy, feeder empty, jam).
//!
//! Scanners announce themselves with mDNS as `_uscan._tcp`; the `rs` TXT key holds the path of
//! the eSCL root (usually `eSCL`) and `ty` the make and model.

#![cfg_attr(target_arch = "wasm32", allow(unused_imports, dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::{Backend, ColorMode, MAX_PAGES, Paper, ScanError, ScanSettings, ScannedPage, Scanner, Source};

/// The service type eSCL scanners announce.
pub const SERVICE: &str = "_uscan._tcp.local.";
/// The largest capabilities or status document read.
const MAX_XML: u64 = 1 << 20;

/// What one input source (flatbed, feeder, feeder both sides) supports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputCaps {
    /// Largest scan area, in 1/300 inch.
    pub max_width: u32,
    pub max_height: u32,
    /// eSCL colour modes: `BlackAndWhite1`, `Grayscale8`, `RGB24`…
    pub color_modes: Vec<String>,
    /// MIME types: `image/jpeg`, `image/png`, `application/pdf`…
    pub formats: Vec<String>,
    /// Discrete resolutions, or the ends of a range (with its step) as `range`.
    pub resolutions: Vec<u32>,
    pub range: Option<(u32, u32, u32)>,
}

/// A scanner's `ScannerCapabilities`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capabilities {
    pub make_and_model: String,
    pub platen: Option<InputCaps>,
    pub adf_simplex: Option<InputCaps>,
    pub adf_duplex: Option<InputCaps>,
}

impl Capabilities {
    /// The input caps for `source`. The duplex feeder falls back to the simplex caps when the
    /// device lists `AdfDuplexInputCaps` without details but announces duplex support.
    pub fn input(&self, source: Source) -> Option<&InputCaps> {
        match source {
            Source::Flatbed => self.platen.as_ref(),
            Source::Feeder => self.adf_simplex.as_ref(),
            Source::FeederDuplex => self.adf_duplex.as_ref(),
        }
    }
}

/// The settings of one scan job, as sent to the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// `Platen` or `Feeder`.
    pub input_source: &'static str,
    pub duplex: bool,
    pub color_mode: String,
    pub dpi: u32,
    pub format: String,
    /// Scan region in 1/300 inch.
    pub width: u32,
    pub height: u32,
}

fn local(n: &roxmltree::Node) -> String {
    n.tag_name().name().to_string()
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn num(n: roxmltree::Node, name: &str) -> Option<u32> {
    child(n, name).and_then(|c| c.text()).and_then(|t| t.trim().parse().ok())
}

fn input_caps(n: roxmltree::Node) -> InputCaps {
    let mut caps = InputCaps { max_width: num(n, "MaxWidth").unwrap_or(0), max_height: num(n, "MaxHeight").unwrap_or(0), ..InputCaps::default() };
    for d in n.descendants().filter(|d| d.is_element()) {
        let text = || d.text().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        match d.tag_name().name() {
            "ColorMode" => {
                if let Some(t) = text()
                    && !caps.color_modes.contains(&t)
                {
                    caps.color_modes.push(t);
                }
            }
            "DocumentFormat" | "DocumentFormatExt" => {
                if let Some(t) = text().map(|t| t.to_ascii_lowercase())
                    && !caps.formats.contains(&t)
                {
                    caps.formats.push(t);
                }
            }
            "DiscreteResolution" => {
                if let (Some(x), y) = (num(d, "XResolution"), num(d, "YResolution"))
                    && y.is_none_or(|y| y == x)
                    && !caps.resolutions.contains(&x)
                {
                    caps.resolutions.push(x);
                }
            }
            "XResolutionRange" => {
                if let (Some(min), Some(max)) = (num(d, "Min"), num(d, "Max"))
                    && min <= max
                {
                    caps.range = Some((min, max, num(d, "Step").unwrap_or(1).max(1)));
                }
            }
            _ => {}
        }
        if caps.color_modes.len() > 64 || caps.formats.len() > 64 || caps.resolutions.len() > 256 {
            break;
        }
    }
    caps.resolutions.sort_unstable();
    caps
}

/// Parse `ScannerCapabilities`. Namespaces are ignored: devices disagree on prefixes.
pub fn parse_capabilities(xml: &str) -> Result<Capabilities, ScanError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| ScanError::Failed(format!("the scanner's capabilities are unreadable ({e})")))?;
    let root = doc.root_element();
    if local(&root) != "ScannerCapabilities" {
        return Err(ScanError::Failed("the scanner's capabilities are unreadable (no ScannerCapabilities)".into()));
    }
    let mut caps = Capabilities {
        make_and_model: child(root, "MakeAndModel").and_then(|n| n.text()).map(crate::tidy).unwrap_or_default(),
        ..Capabilities::default()
    };
    for n in root.descendants().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "PlatenInputCaps" if caps.platen.is_none() => caps.platen = Some(input_caps(n)),
            "AdfSimplexInputCaps" if caps.adf_simplex.is_none() => caps.adf_simplex = Some(input_caps(n)),
            "AdfDuplexInputCaps" if caps.adf_duplex.is_none() => caps.adf_duplex = Some(input_caps(n)),
            _ => {}
        }
    }
    // Some devices announce duplex only as an `AdfOptions/AdfOption` of `Duplex`.
    if caps.adf_duplex.is_none() && root.descendants().any(|n| n.tag_name().name() == "AdfOption" && n.text().is_some_and(|t| t.trim() == "Duplex")) {
        caps.adf_duplex = caps.adf_simplex.clone();
    }
    Ok(caps)
}

/// The scanner's state and its feeder's, from `ScannerStatus`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// `Idle`, `Processing`, `Testing`, `Stopped`, `Down`.
    pub state: String,
    /// `ScannerAdfLoaded`, `ScannerAdfEmpty`, `ScannerAdfJam`, `ScannerAdfHatchOpen`…
    pub adf_state: String,
}

pub fn parse_status(xml: &str) -> Status {
    let Ok(doc) = roxmltree::Document::parse(xml) else { return Status::default() };
    let text = |name: &str| {
        doc.descendants()
            .find(|n| n.is_element() && n.tag_name().name() == name)
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
            .unwrap_or_default()
    };
    Status { state: text("State"), adf_state: text("AdfState") }
}

impl Status {
    /// Why a job was refused, when the status says.
    pub fn error(&self) -> Option<ScanError> {
        let adf = self.adf_state.to_ascii_lowercase();
        if adf.contains("empty") {
            Some(ScanError::FeederEmpty)
        } else if adf.contains("jam") || adf.contains("mispick") {
            Some(ScanError::Jammed)
        } else if adf.contains("hatchopen") || adf.contains("open") {
            Some(ScanError::CoverOpen)
        } else if self.state == "Processing" || self.state == "Testing" {
            Some(ScanError::Busy)
        } else {
            None
        }
    }
}

/// The eSCL colour mode for `mode` among those the device lists (falling back to the nearest).
fn pick_color_mode(caps: &InputCaps, mode: ColorMode) -> String {
    let wanted: &[&str] = match mode {
        ColorMode::BlackWhite => &["BlackAndWhite1", "Grayscale8", "Grayscale16", "RGB24"],
        ColorMode::Gray => &["Grayscale8", "Grayscale16", "RGB24", "BlackAndWhite1"],
        ColorMode::Color => &["RGB24", "RGB48", "Grayscale8"],
    };
    wanted
        .iter()
        .find(|w| caps.color_modes.iter().any(|m| m.eq_ignore_ascii_case(w)))
        .map(|w| (*w).to_string())
        // A device that lists nothing gets the plain name for the mode.
        .unwrap_or_else(|| wanted.first().map_or("RGB24", |w| w).to_string())
}

/// The image format to ask for: lossless for black and white or gray, compact for colour.
fn pick_format(caps: &InputCaps, mode: ColorMode) -> String {
    let order: &[&str] = match mode {
        ColorMode::Color => &["image/jpeg", "image/png", "image/tiff"],
        _ => &["image/png", "image/tiff", "image/jpeg"],
    };
    order.iter().find(|f| caps.formats.iter().any(|g| g == *f)).map_or("image/jpeg", |f| f).to_string()
}

fn pick_dpi(caps: &InputCaps, dpi: u32) -> u32 {
    let mut options = caps.resolutions.clone();
    if let Some((min, max, step)) = caps.range {
        let clamped = dpi.clamp(min, max);
        let snapped = min.saturating_add((clamped - min) / step * step);
        options.push(snapped);
    }
    crate::closest_dpi(&options, dpi).unwrap_or(dpi)
}

const MM_PER_300TH: f64 = 25.4 / 300.0;

/// Plan a job: the closest settings the device supports.
pub fn plan(caps: &Capabilities, s: &ScanSettings) -> Result<Job, ScanError> {
    let input = caps.input(s.source).ok_or(match s.source {
        Source::Flatbed => ScanError::NoSource("flatbed"),
        Source::Feeder => ScanError::NoSource("document feeder"),
        Source::FeederDuplex => ScanError::NoSource("double-sided document feeder"),
    })?;
    let max =
        (input.max_width > 0 && input.max_height > 0).then_some((input.max_width as f64 * MM_PER_300TH, input.max_height as f64 * MM_PER_300TH));
    // Letter is the default when the device states no maximum and Full is asked for.
    let (w_mm, h_mm) = crate::area_mm(s.paper, max).or(Paper::Letter.size_mm()).unwrap_or((215.9, 279.4));
    let to_300 = |mm: f64| (mm / MM_PER_300TH).round().clamp(1.0, 100_000.0) as u32;
    Ok(Job {
        input_source: if s.source.is_feeder() { "Feeder" } else { "Platen" },
        duplex: s.source == Source::FeederDuplex,
        color_mode: pick_color_mode(input, s.color),
        dpi: pick_dpi(input, s.dpi),
        format: pick_format(input, s.color),
        width: to_300(w_mm),
        height: to_300(h_mm),
    })
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// The `ScanSettings` document for `job`. The format is given both as `pwg:DocumentFormat`
/// and `scan:DocumentFormatExt`, which older and newer devices read respectively.
pub fn settings_xml(job: &Job) -> String {
    let fmt = xml_escape(&job.format);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<scan:ScanSettings xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm">
  <pwg:Version>2.0</pwg:Version>
  <pwg:ScanRegions>
    <pwg:ScanRegion>
      <pwg:ContentRegionUnits>escl:ThreeHundredthsOfInches</pwg:ContentRegionUnits>
      <pwg:XOffset>0</pwg:XOffset>
      <pwg:YOffset>0</pwg:YOffset>
      <pwg:Width>{w}</pwg:Width>
      <pwg:Height>{h}</pwg:Height>
    </pwg:ScanRegion>
  </pwg:ScanRegions>
  <pwg:InputSource>{src}</pwg:InputSource>
  <scan:Duplex>{duplex}</scan:Duplex>
  <scan:ColorMode>{mode}</scan:ColorMode>
  <scan:XResolution>{dpi}</scan:XResolution>
  <scan:YResolution>{dpi}</scan:YResolution>
  <pwg:DocumentFormat>{fmt}</pwg:DocumentFormat>
  <scan:DocumentFormatExt>{fmt}</scan:DocumentFormatExt>
</scan:ScanSettings>
"#,
        w = job.width,
        h = job.height,
        src = job.input_source,
        duplex = job.duplex,
        mode = xml_escape(&job.color_mode),
        dpi = job.dpi,
    )
}

/// `http://host[:port]/path` with no trailing slash, from what a user or agent typed after
/// `escl:` (a bare host or address gets `http://` and `/eSCL`). `None` if it isn't usable.
pub fn normalize_base(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() || s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let address = if s.contains("://") { s.to_owned() } else { format!("http://{s}") };
    let mut base = local_url(&address)?;
    if base.path() == "/" {
        base.set_path("/eSCL");
    }
    Some(base.as_str().trim_end_matches('/').to_owned())
}

/// Numeric local addresses only: no DNS lookup/rebinding or proxy can widen this boundary.
/// mDNS ids use the advertised numeric address and follow the same restriction.
fn local_url(s: &str) -> Option<url::Url> {
    let u = url::Url::parse(s).ok()?;
    if !matches!(u.scheme(), "http" | "https")
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || s.contains('\\')
    {
        return None;
    }
    let allowed = match u.host()? {
        url::Host::Ipv4(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        url::Host::Ipv6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            None => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
        },
        url::Host::Domain(_) => false,
    };
    allowed.then_some(u)
}

/// The job URL from a `Location` header (absolute, or a path on the scanner's host).
pub fn resolve_location(base: &str, location: &str) -> Option<String> {
    let base = local_url(base)?;
    let location = location.trim();
    if location.is_empty() || location.chars().any(|c| c.is_control()) {
        return None;
    }
    let root = url::Url::parse(&format!("{}/", base.as_str().trim_end_matches('/'))).ok()?;
    let job = local_url(root.join(location).ok()?.as_str())?;
    (job.origin() == base.origin()).then(|| job.as_str().trim_end_matches('/').to_owned())
}

/// The scanner's name for `escl:` ids found by discovery: `ty`, else the instance name.
pub fn scanner_name(instance: &str, ty: Option<&str>) -> String {
    match ty.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => crate::tidy(t),
        None => crate::tidy(instance.split("._uscan").next().unwrap_or(instance)),
    }
}

/// The id of an announced scanner at `ip:port` whose eSCL root is `rs`.
pub fn id_for(ip: std::net::IpAddr, port: u16, rs: Option<&str>) -> String {
    let host = match ip {
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
    };
    let rs = rs.map(|r| r.trim_matches('/')).filter(|r| !r.is_empty()).unwrap_or("eSCL");
    format!("{}http://{host}:{port}/{rs}", Backend::Escl.prefix())
}

#[cfg(not(target_arch = "wasm32"))]
mod net {
    use super::*;

    pub(super) struct Http {
        quick: ureq::Agent,
        /// For `NextDocument`, which returns only when the page is scanned.
        slow: ureq::Agent,
    }

    pub(super) struct Reply {
        pub status: u16,
        pub location: Option<String>,
        pub body: Vec<u8>,
    }

    fn agent(timeout: Duration) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .timeout_connect(Some(Duration::from_secs(5)))
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .user_agent(concat!("PdfCraft/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent()
    }

    impl Http {
        pub fn new() -> Http {
            Http { quick: agent(Duration::from_secs(20)), slow: agent(Duration::from_secs(300)) }
        }

        fn reply(r: Result<ureq::http::Response<ureq::Body>, ureq::Error>, limit: u64) -> Result<Reply, ScanError> {
            let mut r = r.map_err(|e| ScanError::NotFound(crate::tidy(&e.to_string())))?;
            let status = r.status().as_u16();
            if (300..400).contains(&status) {
                return Err(ScanError::Failed("scanner redirects are not allowed".into()));
            }
            let location = r.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
            let body = if (200..300).contains(&status) {
                r.body_mut()
                    .with_config()
                    .limit(limit)
                    .read_to_vec()
                    .map_err(|e| ScanError::Failed(format!("the scanner's answer was cut off ({})", crate::tidy(&e.to_string()))))?
            } else {
                Vec::new()
            };
            Ok(Reply { status, location, body })
        }

        pub fn get(&self, url: &str, limit: u64) -> Result<Reply, ScanError> {
            local_url(url).ok_or_else(|| ScanError::Failed("use a loopback, link-local or private numeric scanner address".into()))?;
            Self::reply(self.quick.get(url).call(), limit)
        }

        pub fn get_page(&self, url: &str) -> Result<Reply, ScanError> {
            local_url(url).ok_or_else(|| ScanError::Failed("invalid scanner page address".into()))?;
            Self::reply(self.slow.get(url).call(), crate::MAX_PAGE_BYTES)
        }

        pub fn post_xml(&self, url: &str, body: &str) -> Result<Reply, ScanError> {
            local_url(url).ok_or_else(|| ScanError::Failed("invalid scanner job address".into()))?;
            Self::reply(self.quick.post(url).header("Content-Type", "text/xml").send(body), MAX_XML)
        }

        pub fn delete(&self, url: &str) {
            if local_url(url).is_some() {
                let _ = self.quick.delete(url).call();
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn status_error(http: &net::Http, base: &str) -> Option<ScanError> {
    let r = http.get(&format!("{base}/ScannerStatus"), MAX_XML).ok()?;
    parse_status(&String::from_utf8_lossy(&r.body)).error()
}

/// The device's capabilities.
#[cfg(not(target_arch = "wasm32"))]
pub fn capabilities(base: &str) -> Result<Capabilities, ScanError> {
    let http = net::Http::new();
    let r = http.get(&format!("{base}/ScannerCapabilities"), MAX_XML)?;
    if r.status != 200 {
        return Err(ScanError::NotFound(format!("{base} answered HTTP {} (is it an eSCL scanner?)", r.status)));
    }
    parse_capabilities(&String::from_utf8_lossy(&r.body))
}

/// Scan with the eSCL scanner whose root is `base` (from [`normalize_base`]).
#[cfg(not(target_arch = "wasm32"))]
pub fn scan(base: &str, s: &ScanSettings, cancel: &AtomicBool) -> Result<Vec<ScannedPage>, ScanError> {
    let http = net::Http::new();
    let caps = capabilities(base)?;
    let job = plan(&caps, s)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(ScanError::Cancelled);
    }
    // A device warming up answers 503 for a few seconds.
    let mut created = None;
    for attempt in 0..10 {
        let r = http.post_xml(&format!("{base}/ScanJobs"), &settings_xml(&job))?;
        match r.status {
            200..=299 => {
                created = Some(r);
                break;
            }
            503 if attempt < 9 && !cancel.load(Ordering::Relaxed) => std::thread::sleep(Duration::from_millis(500)),
            409 | 503 => {
                return Err(status_error(&http, base).unwrap_or(if job.input_source == "Feeder" { ScanError::FeederEmpty } else { ScanError::Busy }));
            }
            code => return Err(status_error(&http, base).unwrap_or(ScanError::Failed(format!("the scanner refused the job (HTTP {code})")))),
        }
    }
    let created = created.ok_or(ScanError::Busy)?;
    let job_url = created
        .location
        .as_deref()
        .and_then(|l| resolve_location(base, l))
        .ok_or_else(|| ScanError::Failed("the scanner accepted the job but didn't say where it is".into()))?;
    let mut pages = Vec::new();
    let mut retries = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            http.delete(&job_url);
            return Err(ScanError::Cancelled);
        }
        let r = match http.get_page(&format!("{job_url}/NextDocument")) {
            Ok(r) => r,
            Err(e) => {
                http.delete(&job_url);
                return Err(e);
            }
        };
        match r.status {
            200 => {
                retries = 0;
                if r.body.is_empty() {
                    break;
                }
                pages.push(ScannedPage { bytes: r.body, dpi: job.dpi as f64 });
                // The flatbed holds one page; asking again would scan it again on some devices.
                if job.input_source == "Platen" || pages.len() >= MAX_PAGES {
                    http.delete(&job_url);
                    break;
                }
            }
            404 | 410 => break,
            503 if retries < 120 => {
                retries += 1;
                std::thread::sleep(Duration::from_millis(500));
            }
            code => {
                let why = status_error(&http, base);
                http.delete(&job_url);
                if pages.is_empty() {
                    return Err(why.unwrap_or(ScanError::Failed(format!("the scanner answered HTTP {code}"))));
                }
                break;
            }
        }
    }
    if pages.is_empty() {
        return Err(status_error(&http, base).unwrap_or(if job.input_source == "Feeder" { ScanError::FeederEmpty } else { ScanError::NoPages }));
    }
    Ok(pages)
}

#[cfg(target_arch = "wasm32")]
pub fn scan(_base: &str, _s: &ScanSettings, _cancel: &AtomicBool) -> Result<Vec<ScannedPage>, ScanError> {
    Err(ScanError::BackendMissing("Scanning"))
}

/// eSCL scanners announced on the local network within `wait`.
#[cfg(not(target_arch = "wasm32"))]
pub fn discover(wait: Duration) -> Vec<Scanner> {
    let Ok(daemon) = mdns_sd::ServiceDaemon::new() else { return Vec::new() };
    let Ok(rx) = daemon.browse(SERVICE) else {
        let _ = daemon.shutdown();
        return Vec::new();
    };
    let deadline = std::time::Instant::now() + wait;
    let mut found: Vec<Scanner> = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let mut ips: Vec<std::net::IpAddr> = info.addresses.iter().map(|a| a.to_ip_addr()).collect();
                // IPv4 first: link-local IPv6 needs a scope that URLs can't carry portably.
                ips.sort_by_key(|ip| (!ip.is_ipv4(), ip.to_string()));
                let Some(ip) =
                    ips.into_iter().find(|ip| ip.is_ipv4() || !matches!(ip, std::net::IpAddr::V6(v6) if v6.segments()[0] & 0xffc0 == 0xfe80))
                else {
                    continue;
                };
                let id = id_for(ip, info.port, info.txt_properties.get_property_val_str("rs"));
                if normalize_base(id.trim_start_matches(Backend::Escl.prefix())).is_none() {
                    continue;
                }
                if !found.iter().any(|s| s.id == id) && found.len() < 256 {
                    found.push(Scanner {
                        id,
                        name: scanner_name(&info.fullname, info.txt_properties.get_property_val_str("ty")),
                        backend: Backend::Escl,
                    });
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.stop_browse(SERVICE);
    let _ = daemon.shutdown();
    found
}

#[cfg(target_arch = "wasm32")]
pub fn discover(_wait: Duration) -> Vec<Scanner> {
    Vec::new()
}
