use crate::{Args, Automation, Result, failed};
use serde_json::Value;
#[cfg(not(target_arch = "wasm32"))]
use serde_json::json;

/// The environment variable a tool may read an FTP password from. Only the dedicated prefix is
/// readable, so a caller cannot send other process secrets (GITHUB_TOKEN, cloud keys) to an
/// FTP host it chose.
#[cfg(not(target_arch = "wasm32"))]
fn password_variable(variable: &str) -> Result<&str> {
    const PREFIX: &str = "PDFCRAFT_HOT_FOLDER_";
    let valid = variable.len() > PREFIX.len()
        && variable.len() <= 128
        && variable.starts_with(PREFIX)
        && variable.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if valid { Ok(variable) } else { Err(failed("password_env must name a PDFCRAFT_HOT_FOLDER_* environment variable")) }
}

impl Automation {
    #[cfg(not(target_arch = "wasm32"))]
    fn check_hot_folder_roots(&self) -> Result<()> {
        if self.roots.is_some() {
            return Err(failed(
                "Hot Folder does not support separate automation read/write roots; use --root to confine all task paths to one folder",
            ));
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn hot_folder_project(&self, args: &Args) -> Result<Value> {
        self.check_hot_folder_roots()?;
        use pdfcraft_engine::hot_folder_sources as source;
        let path = self.resolve(args.str("path")?, true)?;
        let parent = path.parent().ok_or_else(|| failed("project needs a parent folder"))?;
        let files = args
            .strs("files")?
            .into_iter()
            .map(|file| {
                let resolved = self.resolve(file, false)?;
                resolved
                    .strip_prefix(parent)
                    .map(std::path::Path::to_path_buf)
                    .map_err(|_| failed("save the project in a folder containing all source files"))
            })
            .collect::<Result<Vec<_>>>()?;
        let manifest = source::ProjectManifest { files, ..Default::default() };
        source::write_project_manifest(&path, &manifest, self.root.as_deref()).map_err(failed)?;
        Ok(json!({"path":path,"project":manifest}))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn hot_folder_source(&self, args: &Args) -> Result<Value> {
        self.check_hot_folder_roots()?;
        use pdfcraft_engine::{hot_folder as hf, hot_folder_sources as source};
        let path = self.resolve(args.str("config")?, false)?;
        let config = hf::load_config(&path).map_err(failed)?;
        let task_id = args.str("task")?;
        let task = config.tasks.iter().find(|task| task.id == task_id).ok_or_else(|| failed("unknown task"))?;
        let id = match &task.source {
            source::SourceConfig::Ftp(settings) => &settings.credential_id,
            source::SourceConfig::Outlook(settings) => &settings.credential_id,
            _ => return Err(failed("this task has no authenticated remote source")),
        };
        let action = args.str("action")?;
        if task.enabled && matches!(action, "save_ftp_password" | "forget_credentials" | "outlook_begin" | "outlook_poll") {
            return Err(failed("stop the task before changing its credentials"));
        }
        let store = source::CredentialStore::for_config(&path, self.root.as_deref()).map_err(failed)?;
        match action {
            "credential_status" => Ok(json!({"available":source::credential_status(&store,id).map_err(failed)?})),
            "forget_credentials" => {
                source::forget_credentials(&store, id).map_err(failed)?;
                Ok(json!({"removed":true}))
            }
            "save_ftp_password" => {
                if !matches!(task.source, source::SourceConfig::Ftp(_)) {
                    return Err(failed("passwords apply only to FTP tasks"));
                }
                // Tools accept a variable name, never a secret in replayable task arguments.
                let variable = password_variable(args.str("password_env")?)?;
                let password =
                    zeroize::Zeroizing::new(std::env::var(variable).map_err(|_| failed("the named password environment variable is unavailable"))?);
                source::save_ftp_password(&store, id, &password).map_err(failed)?;
                Ok(json!({"saved":true}))
            }
            "outlook_begin" | "outlook_poll" | "outlook_folders" => {
                let source::SourceConfig::Outlook(settings) = &task.source else {
                    return Err(failed("this task is not an Outlook source"));
                };
                match action {
                    "outlook_begin" => serde_json::to_value(source::outlook_begin_sign_in(&store, settings).map_err(failed)?).map_err(failed),
                    "outlook_poll" => {
                        let flow: source::DeviceSignIn =
                            serde_json::from_value(args.0.get("flow").cloned().ok_or_else(|| failed("flow is required"))?).map_err(failed)?;
                        serde_json::to_value(source::outlook_poll_sign_in(&store, settings, &flow).map_err(failed)?).map_err(failed)
                    }
                    _ => serde_json::to_value(source::outlook_folders(&store, settings, args.opt_str("parent")?, &|| false).map_err(failed)?)
                        .map_err(failed),
                }
            }
            _ => Err(failed("unknown source action")),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn hot_folder_source(&self, _args: &Args) -> Result<Value> {
        Err(failed("Hot Folder sources require native CLI or desktop"))
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn hot_folder_project(&self, _args: &Args) -> Result<Value> {
        Err(failed("Hot Folder projects require native CLI or desktop"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn hot_folder(&self, action: &str, args: &Args) -> Result<Value> {
        self.check_hot_folder_roots()?;
        use pdfcraft_engine::hot_folder as hf;
        let path = self.resolve(args.str("config")?, action == "hot_folder_configure")?;
        // Validate every nested path with the same root resolver as other automation tools.
        let mut config: hf::Config = if action == "hot_folder_configure" {
            serde_json::from_value(args.0.get("settings").cloned().unwrap_or(json!({}))).map_err(failed)?
        } else {
            hf::load_config(&path).map_err(failed)?
        };
        for t in &mut config.tasks {
            t.input = self.resolve(&t.input.to_string_lossy(), false)?;
            t.output = self.resolve(&t.output.to_string_lossy(), false)?;
            if let pdfcraft_engine::hot_folder_sources::SourceConfig::Project(source) = &mut t.source {
                source.manifest = self.resolve(&source.manifest.to_string_lossy(), false)?;
                if let pdfcraft_engine::hot_folder_sources::SourceAction::Move { destination } = &mut source.action {
                    *destination = self.resolve(destination, false)?.to_string_lossy().into_owned();
                }
            }
            if let Some(folder) = &mut t.blank_archive {
                *folder = self.resolve(&folder.to_string_lossy(), false)?;
            }
            for step in &mut t.output_plan.steps {
                if let Some(folder) = &mut step.folder {
                    *folder = self.resolve(&folder.to_string_lossy(), false)?;
                }
            }
            if t.originals == hf::Originals::Move {
                t.archive = self.resolve(&t.archive.to_string_lossy(), false)?;
            }
        }
        hf::validate(&config, self.root.as_deref()).map_err(failed)?;
        self.resolve(&hf::state_path(&path).to_string_lossy(), true)?;
        self.resolve(&path.with_extension("lock").to_string_lossy(), true)?;
        match action {
            "hot_folder_configure" => {
                hf::save_config(&path, &config, self.root.as_deref()).map_err(failed)?;
                Ok(json!({ "config": path, "settings": config }))
            }
            "hot_folder_poll" => serde_json::to_value(hf::poll(&path, self.root.as_deref()).map_err(failed)?).map_err(failed),
            "hot_folder_retry" => {
                hf::reset(&path, args.str("task")?, self.root.as_deref()).map_err(failed)?;
                Ok(json!({ "reset": true }))
            }
            "hot_folder_run_now" => {
                hf::run_now(&path, args.str("task")?, self.root.as_deref()).map_err(failed)?;
                Ok(json!({ "scheduled_now": true }))
            }
            _ => Ok(json!({ "settings": config, "state": hf::read_state(&path).map_err(failed)? })),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn hot_folder(&self, _action: &str, _args: &Args) -> Result<Value> {
        Err(failed("Hot Folder requires the desktop application or native CLI"))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn ftp_passwords_come_only_from_the_dedicated_variables() {
        assert!(password_variable("PDFCRAFT_HOT_FOLDER_FTP_PASSWORD").is_ok());
        for other in ["GITHUB_TOKEN", "AWS_SECRET_ACCESS_KEY", "PDFCRAFT_HOT_FOLDER_", "PDFCRAFT_HOT_FOLDER_A-B", "", "pdfcraft_hot_folder_x"] {
            assert!(password_variable(other).is_err(), "{other}");
        }
    }
}
