use crate::{Status, Worker};
use std::path::Path;

fn unavailable<T>() -> Result<T, String> {
    Err("System services are supported on Windows, macOS and Linux with systemd".into())
}
pub fn status(_: &Path, _: &Path) -> Result<Status, String> {
    unavailable()
}
pub fn set_enabled(_: bool, _: &Path, _: &Path) -> Result<(), String> {
    unavailable()
}
pub fn start() -> Result<(), String> {
    unavailable()
}
pub fn stop() -> Result<(), String> {
    unavailable()
}
pub fn install(_: &Path, _: &Path, _: &str) -> Result<(), String> {
    unavailable()
}
pub fn uninstall() -> Result<(), String> {
    unavailable()
}
pub fn run(_: &Path, _: Worker) -> Result<(), String> {
    unavailable()
}
