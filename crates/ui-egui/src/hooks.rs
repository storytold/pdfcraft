/// Told every frame whether any tab has unsaved work, or an OCR review is not yet accepted or
/// rejected, so a host page can warn before leaving.
pub type HostDirtyFn = Box<dyn FnMut(bool)>;
