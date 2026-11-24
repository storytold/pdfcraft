use super::{Arg, CellError, Ctx, R, Value, num, num_val, text};

// ---------------------------------------------------------------------------------------------
// CONVERT

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dim {
    Mass,
    Length,
    Time,
    Pressure,
    Force,
    Energy,
    Power,
    Magnetism,
    Temperature,
    Volume,
    Area,
    Information,
    Speed,
}

/// How a unit accepts prefixes: none, SI (raised to `power` for areas/volumes) or SI + binary.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pre {
    No,
    Si(i32),
    Bin,
}

struct Unit {
    names: &'static [&'static str],
    dim: Dim,
    factor: f64,
    pre: Pre,
}

const fn u(names: &'static [&'static str], dim: Dim, factor: f64, pre: Pre) -> Unit {
    Unit { names, dim, factor, pre }
}

const IN: f64 = 0.0254;

const FT: f64 = 0.3048;

const YD: f64 = 0.9144;

const MI: f64 = 1609.344;

const NMI: f64 = 1852.0;

const LY: f64 = 9_460_730_472_580_800.0;

const PICA_PT: f64 = 0.0254 / 72.0;

const PICA: f64 = 0.0254 / 6.0;

const GAL: f64 = 0.003_785_411_784;

const UNITS: &[Unit] = &[
    // Mass (gram)
    u(&["g"], Dim::Mass, 1.0, Pre::Si(1)),
    u(&["sg"], Dim::Mass, 14_593.902_937_206_4, Pre::No),
    u(&["lbm"], Dim::Mass, 453.592_37, Pre::No),
    u(&["u"], Dim::Mass, 1.660_539_066_6e-24, Pre::Si(1)),
    u(&["ozm"], Dim::Mass, 28.349_523_125, Pre::No),
    u(&["grain"], Dim::Mass, 0.064_798_91, Pre::No),
    u(&["cwt", "shweight"], Dim::Mass, 45_359.237, Pre::No),
    u(&["uk_cwt", "lcwt", "hweight"], Dim::Mass, 50_802.345_44, Pre::No),
    u(&["stone"], Dim::Mass, 6_350.293_18, Pre::No),
    u(&["ton"], Dim::Mass, 907_184.74, Pre::No),
    u(&["uk_ton", "LTON", "brton"], Dim::Mass, 1_016_046.908_8, Pre::No),
    // Length (metre)
    u(&["m"], Dim::Length, 1.0, Pre::Si(1)),
    u(&["mi"], Dim::Length, MI, Pre::No),
    u(&["Nmi"], Dim::Length, NMI, Pre::No),
    u(&["in"], Dim::Length, IN, Pre::No),
    u(&["ft"], Dim::Length, FT, Pre::No),
    u(&["yd"], Dim::Length, YD, Pre::No),
    u(&["ang"], Dim::Length, 1e-10, Pre::Si(1)),
    u(&["ell"], Dim::Length, 1.143, Pre::No),
    u(&["ly"], Dim::Length, LY, Pre::Si(1)),
    u(&["parsec", "pc"], Dim::Length, 30_856_775_814_913_673.0, Pre::Si(1)),
    u(&["Picapt", "Pica"], Dim::Length, PICA_PT, Pre::No),
    u(&["pica"], Dim::Length, PICA, Pre::No),
    u(&["survey_mi"], Dim::Length, 1_609.347_218_694_437, Pre::No),
    // Time (second)
    u(&["yr"], Dim::Time, 31_557_600.0, Pre::No),
    u(&["day", "d"], Dim::Time, 86_400.0, Pre::No),
    u(&["hr"], Dim::Time, 3_600.0, Pre::No),
    u(&["mn", "min"], Dim::Time, 60.0, Pre::No),
    u(&["sec", "s"], Dim::Time, 1.0, Pre::Si(1)),
    // Pressure (pascal)
    u(&["Pa", "p"], Dim::Pressure, 1.0, Pre::Si(1)),
    u(&["atm", "at"], Dim::Pressure, 101_325.0, Pre::Si(1)),
    u(&["mmHg"], Dim::Pressure, 133.322_387_415, Pre::Si(1)),
    u(&["psi"], Dim::Pressure, 6_894.757_293_168_36, Pre::No),
    u(&["Torr"], Dim::Pressure, 101_325.0 / 760.0, Pre::No),
    // Force (newton)
    u(&["N"], Dim::Force, 1.0, Pre::Si(1)),
    u(&["dyn", "dy"], Dim::Force, 1e-5, Pre::Si(1)),
    u(&["lbf"], Dim::Force, 4.448_221_615_260_5, Pre::No),
    u(&["pond"], Dim::Force, 0.009_806_65, Pre::Si(1)),
    // Energy (joule)
    u(&["J"], Dim::Energy, 1.0, Pre::Si(1)),
    u(&["e"], Dim::Energy, 1e-7, Pre::Si(1)),
    u(&["c"], Dim::Energy, 4.184, Pre::Si(1)),
    u(&["cal"], Dim::Energy, 4.1868, Pre::Si(1)),
    u(&["eV", "ev"], Dim::Energy, 1.602_176_634e-19, Pre::Si(1)),
    u(&["HPh", "hh"], Dim::Energy, 2_684_519.537_696_17, Pre::No),
    u(&["Wh", "wh"], Dim::Energy, 3_600.0, Pre::Si(1)),
    u(&["flb"], Dim::Energy, 1.355_817_948_331_4, Pre::No),
    u(&["BTU", "btu"], Dim::Energy, 1_055.055_852_62, Pre::No),
    // Power (watt)
    u(&["W", "w"], Dim::Power, 1.0, Pre::Si(1)),
    u(&["HP", "h"], Dim::Power, 745.699_871_582_270_2, Pre::No),
    u(&["PS"], Dim::Power, 735.498_75, Pre::No),
    // Magnetism (tesla)
    u(&["T"], Dim::Magnetism, 1.0, Pre::Si(1)),
    u(&["ga"], Dim::Magnetism, 1e-4, Pre::Si(1)),
    // Temperature (handled specially; factor unused)
    u(&["C", "cel"], Dim::Temperature, 1.0, Pre::No),
    u(&["F", "fah"], Dim::Temperature, 1.0, Pre::No),
    u(&["K", "kel"], Dim::Temperature, 1.0, Pre::Si(1)),
    u(&["Rank"], Dim::Temperature, 1.0, Pre::No),
    u(&["Reau"], Dim::Temperature, 1.0, Pre::No),
    // Volume (cubic metre)
    u(&["l", "L", "lt"], Dim::Volume, 0.001, Pre::Si(1)),
    u(&["tsp"], Dim::Volume, GAL / 768.0, Pre::No),
    u(&["tspm"], Dim::Volume, 5e-6, Pre::No),
    u(&["tbs"], Dim::Volume, GAL / 256.0, Pre::No),
    u(&["oz"], Dim::Volume, GAL / 128.0, Pre::No),
    u(&["cup"], Dim::Volume, GAL / 16.0, Pre::No),
    u(&["pt", "us_pt"], Dim::Volume, GAL / 8.0, Pre::No),
    u(&["uk_pt"], Dim::Volume, 0.000_568_261_25, Pre::No),
    u(&["qt"], Dim::Volume, GAL / 4.0, Pre::No),
    u(&["uk_qt"], Dim::Volume, 0.001_136_522_5, Pre::No),
    u(&["gal"], Dim::Volume, GAL, Pre::No),
    u(&["uk_gal"], Dim::Volume, 0.004_546_09, Pre::No),
    u(&["m3"], Dim::Volume, 1.0, Pre::Si(3)),
    u(&["mi3"], Dim::Volume, MI * MI * MI, Pre::No),
    u(&["Nmi3"], Dim::Volume, NMI * NMI * NMI, Pre::No),
    u(&["yd3"], Dim::Volume, YD * YD * YD, Pre::No),
    u(&["ft3"], Dim::Volume, FT * FT * FT, Pre::No),
    u(&["in3"], Dim::Volume, IN * IN * IN, Pre::No),
    u(&["ang3"], Dim::Volume, 1e-30, Pre::Si(3)),
    u(&["ly3"], Dim::Volume, LY * LY * LY, Pre::No),
    u(&["Picapt3", "Pica3"], Dim::Volume, PICA_PT * PICA_PT * PICA_PT, Pre::No),
    u(&["barrel"], Dim::Volume, 0.158_987_294_928, Pre::No),
    u(&["bushel"], Dim::Volume, 0.035_239_070_166_88, Pre::No),
    u(&["MTON"], Dim::Volume, 1.132_673_863_68, Pre::No),
    u(&["GRT", "regton"], Dim::Volume, 2.831_684_659_2, Pre::No),
    // Area (square metre)
    u(&["m2"], Dim::Area, 1.0, Pre::Si(2)),
    u(&["ha"], Dim::Area, 10_000.0, Pre::No),
    u(&["ar"], Dim::Area, 100.0, Pre::Si(1)),
    u(&["ft2"], Dim::Area, FT * FT, Pre::No),
    u(&["in2"], Dim::Area, IN * IN, Pre::No),
    u(&["yd2"], Dim::Area, YD * YD, Pre::No),
    u(&["mi2"], Dim::Area, MI * MI, Pre::No),
    u(&["Nmi2"], Dim::Area, NMI * NMI, Pre::No),
    u(&["ang2"], Dim::Area, 1e-20, Pre::Si(2)),
    u(&["ly2"], Dim::Area, LY * LY, Pre::No),
    u(&["Picapt2", "Pica2"], Dim::Area, PICA_PT * PICA_PT, Pre::No),
    u(&["uk_acre"], Dim::Area, 4_046.856_422_4, Pre::No),
    u(&["us_acre"], Dim::Area, 4_046.872_609_874_252, Pre::No),
    u(&["Morgen"], Dim::Area, 2_500.0, Pre::No),
    // Information (bit)
    u(&["bit"], Dim::Information, 1.0, Pre::Bin),
    u(&["byte"], Dim::Information, 8.0, Pre::Bin),
    // Speed (metres per second)
    u(&["m/s", "m/sec"], Dim::Speed, 1.0, Pre::Si(1)),
    u(&["m/h", "m/hr"], Dim::Speed, 1.0 / 3600.0, Pre::Si(1)),
    u(&["mph"], Dim::Speed, MI / 3600.0, Pre::No),
    u(&["kn"], Dim::Speed, NMI / 3600.0, Pre::No),
    u(&["admkn"], Dim::Speed, 6080.0 * FT / 3600.0, Pre::No),
];

const SI_PREFIXES: &[(&str, f64)] = &[
    ("da", 1e1),
    ("Y", 1e24),
    ("Z", 1e21),
    ("E", 1e18),
    ("P", 1e15),
    ("T", 1e12),
    ("G", 1e9),
    ("M", 1e6),
    ("k", 1e3),
    ("h", 1e2),
    ("e", 1e1),
    ("d", 1e-1),
    ("c", 1e-2),
    ("m", 1e-3),
    ("u", 1e-6),
    ("n", 1e-9),
    ("p", 1e-12),
    ("f", 1e-15),
    ("a", 1e-18),
    ("z", 1e-21),
    ("y", 1e-24),
];

const BIN_PREFIXES: &[(&str, f64)] = &[
    ("ki", 1024.0),
    ("Mi", 1_048_576.0),
    ("Gi", 1_073_741_824.0),
    ("Ti", 1_099_511_627_776.0),
    ("Pi", 1_125_899_906_842_624.0),
    ("Ei", 1_152_921_504_606_846_976.0),
    ("Zi", 1_180_591_620_717_411_303_424.0),
    ("Yi", 1_208_925_819_614_629_174_706_176.0),
];

fn find_unit(name: &str) -> Option<&'static Unit> {
    UNITS.iter().find(|u| u.names.contains(&name))
}

/// Resolves a unit name to (unit, prefix multiplier). Exact names win over prefixed ones.
fn resolve(name: &str) -> Option<(&'static Unit, f64)> {
    let name = name.replace("^2", "2").replace("^3", "3");
    if let Some(u) = find_unit(&name) {
        return Some((u, 1.0));
    }
    for (p, f) in BIN_PREFIXES {
        if let Some(rest) = name.strip_prefix(p)
            && let Some(u) = find_unit(rest)
            && u.pre == Pre::Bin
        {
            return Some((u, *f));
        }
    }
    for (p, f) in SI_PREFIXES {
        if let Some(rest) = name.strip_prefix(p)
            && let Some(u) = find_unit(rest)
        {
            match u.pre {
                Pre::Si(pow) => return Some((u, f.powi(pow))),
                Pre::Bin => return Some((u, *f)),
                Pre::No => {}
            }
        }
    }
    None
}

fn to_kelvin(name: &str, x: f64) -> f64 {
    match name {
        "C" | "cel" => x + 273.15,
        "F" | "fah" => (x - 32.0) * 5.0 / 9.0 + 273.15,
        "Rank" => x * 5.0 / 9.0,
        "Reau" => x * 1.25 + 273.15,
        _ => x,
    }
}

fn from_kelvin(name: &str, k: f64) -> f64 {
    match name {
        "C" | "cel" => k - 273.15,
        "F" | "fah" => (k - 273.15) * 9.0 / 5.0 + 32.0,
        "Rank" => k * 9.0 / 5.0,
        "Reau" => (k - 273.15) / 1.25,
        _ => k,
    }
}

pub(super) fn convert(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = num(a, 0)?;
    let from = text(a, 1)?;
    let to = text(a, 2)?;
    let (fu, fp) = resolve(&from).ok_or(CellError::NA)?;
    let (tu, tp) = resolve(&to).ok_or(CellError::NA)?;
    if fu.dim != tu.dim {
        return Err(CellError::NA);
    }
    if fu.dim == Dim::Temperature {
        let fname = fu.names.first().copied().unwrap_or("K");
        let tname = tu.names.first().copied().unwrap_or("K");
        if fname == tname && fp == tp {
            return num_val(x);
        }
        let k = to_kelvin(fname, x * fp);
        return num_val(from_kelvin(tname, k) / tp);
    }
    let base = x * fu.factor * fp;
    num_val(base / (tu.factor * tp))
}
