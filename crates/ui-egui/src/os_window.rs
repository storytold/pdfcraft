//! The app id every PdfCraft OS window carries — the main window (`main.rs`), the runtime's
//! windows (`InstallSpec::app_id`) and extra windows built with [`builder`] — so docks show the
//! PdfCraft icon on them and Wayland compositors' window rules can match them. Without one the
//! app id is empty.
//!
//! A Nightly build is a separate app beside the stable one, so it has two identities: the package
//! one (`ai.storyteller.pdfcraft.nightly`, the macOS bundle, the Linux `.desktop` file and the
//! Launcher's), and the window one (`ai.storyteller.pdfcraft.nightly-v1`, the Wayland app id and
//! the Windows AUMID), which is what tells the two taskbar icons apart.

/// The stable identity: the `.desktop` file name, hicolor icon name and macOS bundle id.
pub const APP_ID: &str = "ai.storyteller.pdfcraft";

/// The Nightly package, bundle and Linux identity.
pub const NIGHTLY_PACKAGE_ID: &str = "ai.storyteller.pdfcraft.nightly";

/// The Nightly window, Wayland and Windows AUMID identity.
pub const NIGHTLY_WINDOW_ID: &str = "ai.storyteller.pdfcraft.nightly-v1";

/// Immutable startup identity. The desktop selects it before any UI/runtime service is created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchIdentity {
    Stable,
    NightlyV1,
}

static IDENTITY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn select_identity(identity: LaunchIdentity) -> Result<(), String> {
    let value = match identity {
        LaunchIdentity::Stable => 1,
        LaunchIdentity::NightlyV1 => 2,
    };
    IDENTITY
        .compare_exchange(0, value, std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire)
        .map(|_| ())
        .map_err(|_| "the window launch identity was already selected".into())
}

pub fn identity() -> LaunchIdentity {
    if IDENTITY.load(std::sync::atomic::Ordering::Acquire) == 2 { LaunchIdentity::NightlyV1 } else { LaunchIdentity::Stable }
}

pub fn is_nightly() -> bool {
    matches!(identity(), LaunchIdentity::NightlyV1)
}

/// The window's app id: the Wayland `app_id`, the runtime's windows and the extra windows.
pub const fn id_for(identity: LaunchIdentity) -> &'static str {
    match identity {
        LaunchIdentity::Stable => APP_ID,
        LaunchIdentity::NightlyV1 => NIGHTLY_WINDOW_ID,
    }
}

/// The package identity (macOS bundle, Linux `.desktop`, Launcher row).
pub const fn package_id_for(identity: LaunchIdentity) -> &'static str {
    match identity {
        LaunchIdentity::Stable => APP_ID,
        LaunchIdentity::NightlyV1 => NIGHTLY_PACKAGE_ID,
    }
}

pub fn app_id() -> &'static str {
    id_for(identity())
}

/// The name this build shows: the window title, the menus' About/Hide/Quit items and About.
pub const fn name_for(identity: LaunchIdentity) -> &'static str {
    match identity {
        LaunchIdentity::Stable => "PdfCraft",
        LaunchIdentity::NightlyV1 => "PdfCraft Nightly",
    }
}

pub fn name() -> &'static str {
    name_for(identity())
}

/// Whether the stable app's services may run: the updater, the stable agent link and the stable
/// settings folder. A Nightly window never starts them.
pub const fn stable_services_for(identity: LaunchIdentity) -> bool {
    matches!(identity, LaunchIdentity::Stable)
}

pub fn stable_services_allowed() -> bool {
    stable_services_for(identity())
}

/// What the binary reports about itself: the version `--version` prints and the commit it was
/// built from. Installed once at startup, before the first frame or window; the defaults describe
/// the stable build the UI tests run as.
#[derive(Debug)]
struct Build {
    version: String,
    commit: Option<String>,
}

static BUILD: std::sync::OnceLock<Build> = std::sync::OnceLock::new();

pub fn install_build(version: impl Into<String>, commit: Option<&str>) -> Result<(), String> {
    BUILD
        .set(Build { version: version.into(), commit: commit.filter(|c| !c.is_empty()).map(str::to_owned) })
        .map_err(|_| "the build identity was already installed".into())
}

/// The version this build reports (the prepared Nightly identity in a Nightly build).
pub fn version() -> String {
    BUILD.get().map_or_else(pdfcraft_engine::build_info::version, |b| b.version.clone())
}

/// The source commit this build was made from, when it was recorded.
pub fn commit() -> Option<&'static str> {
    BUILD.get().and_then(|b| b.commit.as_deref())
}

/// A builder for an extra OS window, carrying [`app_id`].
pub fn builder() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default().with_app_id(app_id())
}

#[cfg(test)]
mod tests {
    #[test]
    fn extra_windows_carry_the_app_id() {
        assert_eq!(super::builder().app_id.as_deref(), Some("ai.storyteller.pdfcraft"));
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    #[test]
    fn nightly_identity_is_distinct_and_never_enables_stable_services() {
        assert_eq!(id_for(LaunchIdentity::Stable), APP_ID);
        assert_ne!(id_for(LaunchIdentity::NightlyV1), APP_ID);
        assert_ne!(id_for(LaunchIdentity::NightlyV1), package_id_for(LaunchIdentity::NightlyV1));
        assert_eq!(package_id_for(LaunchIdentity::Stable), APP_ID);
        assert!(stable_services_for(LaunchIdentity::Stable));
        assert!(!stable_services_for(LaunchIdentity::NightlyV1));
        assert_ne!(name_for(LaunchIdentity::Stable), name_for(LaunchIdentity::NightlyV1));
    }
}
