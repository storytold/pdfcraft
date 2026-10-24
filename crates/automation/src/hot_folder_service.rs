use crate::{Args, Automation, Result, failed};
use serde_json::Value;

impl Automation {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn hot_folder_service(&self, args: &Args) -> Result<Value> {
        use pdfcraft_hot_folder_service as service;
        let action = args.str("action")?;
        // A filesystem root cannot confine system service changes. Never silently cross it.
        if action != "status" && (self.root.is_some() || self.roots.is_some()) {
            return Err(failed(
                "System service changes are unavailable in a filesystem-confined session; run an explicit unrestricted administration command",
            ));
        }
        let executable = self.resolve(args.str("executable")?, false)?;
        let config = self.resolve(args.str("config")?, false)?;
        match action {
            "status" => {}
            "start" => service::start(&executable, &config).map_err(failed)?,
            "stop" => service::stop(&executable, &config).map_err(failed)?,
            "enable" => service::set_enabled(true, &executable, &config).map_err(failed)?,
            "disable" => service::set_enabled(false, &executable, &config).map_err(failed)?,
            "install" => service::install(&executable, &config, args.str("account")?).map_err(failed)?,
            "uninstall" => service::uninstall(&executable, &config).map_err(failed)?,
            "prepare-account" => service::prepare_account(&executable, &config).map_err(failed)?,
            _ => return Err(failed("action must be status, start, stop, enable, disable, install, uninstall or prepare-account")),
        }
        serde_json::to_value(service::status(&executable, &config).map_err(failed)?).map_err(failed)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn hot_folder_service(&self, _: &Args) -> Result<Value> {
        Err(failed("System services require the native application or CLI"))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn system_changes_never_escape_separate_automation_roots() {
        let dir = tempfile::tempdir().unwrap();
        for (read, write) in [(true, false), (false, true), (true, true)] {
            let roots = crate::file_access::AutomationRoots::new(read.then_some(dir.path()), write.then_some(dir.path())).unwrap();
            let automation = Automation::new().with_automation_roots(roots);
            for action in ["start", "stop", "enable", "disable", "install", "uninstall", "prepare-account"] {
                let value = json!({"action":action,"executable":"unneeded","config":"unneeded","account":"root"});
                assert!(automation.hot_folder_service(&Args(&value)).unwrap_err().to_string().contains("confined"));
            }
        }
    }

    #[test]
    fn system_changes_never_escape_a_confined_automation_session() {
        let root = std::env::temp_dir();
        let mut automation = Automation::new();
        automation.root = Some(root);
        for action in ["start", "stop", "enable", "disable", "install", "uninstall", "prepare-account"] {
            let value = json!({"action":action,"executable":"unneeded","config":"unneeded","account":"root"});
            assert!(automation.hot_folder_service(&Args(&value)).unwrap_err().to_string().contains("confined"));
        }
    }
}
