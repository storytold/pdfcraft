//! App-managed print colour for `doc_print` (`color_management` app) and the installed printer
//! profiles it can name (`print_color_profiles`).

use pdfcraft_engine::print::colour::{self, Intent, ManagedColour, PrinterProfile};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed};

/// An invalid-arguments error saying `m`.
fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

impl Automation {
    /// `doc_print`'s colour handling: `None` for `printer` (the default), else the profile, intent
    /// and black point compensation the pictures are converted with.
    pub(crate) fn managed_colour(&self, a: &Args) -> Result<Option<ManagedColour>> {
        let app = match a.opt_str("color_management")?.unwrap_or("printer") {
            "printer" => false,
            "app" => true,
            m => return Err(bad(format!("unknown color_management {m:?}: printer (the printer manages colour) or app"))),
        };
        let asked = ["color_profile", "rendering_intent", "black_point_compensation"].into_iter().find(|k| a.get(k).is_some());
        if !app {
            return match asked {
                Some(k) => Err(bad(format!("{k} needs color_management app"))),
                None => Ok(None),
            };
        }
        let name = a
            .opt_str("color_profile")?
            .ok_or_else(|| bad("color_management app needs color_profile: an ICC file or a name from print_color_profiles"))?;
        let profile = self.printer_profile(name)?;
        let intent = match a.opt_str("rendering_intent")? {
            None => Intent::RelativeColorimetric,
            Some(i) => {
                Intent::parse(i).ok_or_else(|| bad(format!("unknown rendering_intent {i:?}: perceptual, relative, saturation or absolute")))?
            }
        };
        let black_point = a.opt_bool("black_point_compensation")?.unwrap_or(true);
        Ok(Some(ManagedColour { profile, intent, black_point }))
    }

    /// The profile `name` names: an ICC file, else an installed profile's name or file name. A
    /// file is tried first so naming one doesn't read every installed profile.
    fn printer_profile(&self, name: &str) -> Result<PrinterProfile> {
        let file = self.resolve_for(name, crate::file_access::Access::Read, true);
        if let Ok(path) = &file
            && path.is_file()
        {
            return PrinterProfile::read(path).map_err(bad);
        }
        let installed = colour::installed_profiles();
        if let Some(p) = colour::find_installed(&installed, name) {
            return PrinterProfile::read(&p.path).map_err(failed);
        }
        file?;
        Err(bad(format!("color_profile {name:?} is neither an installed profile (print_color_profiles) nor an ICC file")))
    }

    pub(crate) fn print_color_profiles(&self) -> Result<Value> {
        let list: Vec<Value> = colour::installed_profiles()
            .into_iter()
            .map(|p| json!({ "name": p.name, "path": p.path.to_string_lossy(), "color_space": if p.gray { "gray" } else { "rgb" } }))
            .collect();
        Ok(json!({ "count": list.len(), "profiles": list }))
    }
}
