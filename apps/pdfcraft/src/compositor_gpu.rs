//! The GPU the Wayland compositor draws with.
//!
//! A window drawn on another GPU reaches the compositor as that GPU's buffers, and the compositor
//! may not show them: it can refuse them and end the window's connection (#96), or take them and
//! show nothing, with no error on either side. The second happened on a hybrid laptop whose
//! Hyprland runs on the NVIDIA GPU (`AQ_DRM_DEVICES`) while the built-in panel hangs off the Intel
//! one: the panel rule picked Intel and the window stayed empty. Which GPU drives a display is a
//! guess at the compositor's GPU; the compositor states it, as the `main_device` of its
//! linux-dmabuf feedback (protocol version 4): the device it imports client buffers on.

#[cfg(target_os = "linux")]
use wayland_client::globals::{GlobalListContents, registry_queue_init};
#[cfg(target_os = "linux")]
use wayland_client::protocol::wl_registry::{self, WlRegistry};
#[cfg(target_os = "linux")]
use wayland_client::{Connection, Dispatch, QueueHandle};
#[cfg(target_os = "linux")]
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_feedback_v1::{self, ZwpLinuxDmabufFeedbackV1};
#[cfg(target_os = "linux")]
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::{self, ZwpLinuxDmabufV1};

/// PCI `(vendor, device)` ids of the GPU the Wayland compositor draws with. `None` outside a
/// Wayland session, when the compositor doesn't say (linux-dmabuf older than version 4), when its
/// device isn't a PCI GPU, and on other systems.
#[cfg(target_os = "linux")]
pub(crate) fn compositor_gpu() -> Option<(u32, u32)> {
    let device = match main_device() {
        Ok(device) => device,
        Err(why) => {
            log::debug!("the compositor's GPU is unknown: {why}");
            return None;
        }
    };
    let pci = pci_ids(std::path::Path::new("/sys/dev/char"), device);
    match pci {
        Some((vendor, id)) => log::info!("the compositor draws with {vendor:04x}:{id:04x}"),
        None => log::debug!("the compositor's device {:?} has no PCI ids", major_minor(device)),
    }
    pci
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn compositor_gpu() -> Option<(u32, u32)> {
    None
}

/// The `main_device` of the compositor's default linux-dmabuf feedback, asked on a connection of
/// its own that is closed again before the window opens.
#[cfg(target_os = "linux")]
fn main_device() -> Result<u64, String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland compositor: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<Feedback>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    // Version 4 brought the feedback; older compositors only list formats.
    let dmabuf: ZwpLinuxDmabufV1 = globals.bind(&qh, 4..=4, ()).map_err(|e| format!("linux-dmabuf version 4: {e}"))?;
    let _feedback = dmabuf.get_default_feedback(&qh, ());
    let mut state = Feedback::default();
    // The compositor sends the whole feedback, ending with `done`, in reply; the bound only keeps
    // a compositor that never ends it from holding up the start.
    for _ in 0..4 {
        if state.done {
            break;
        }
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    }
    // Dropping the connection releases the objects.
    let device = state.main_device.ok_or("the compositor named no main device")?;
    // A `dev_t`, in this machine's byte order.
    let device: [u8; 8] = device.try_into().map_err(|d: Vec<u8>| format!("a {}-byte device number", d.len()))?;
    Ok(u64::from_ne_bytes(device))
}

/// What [`main_device`] collects from the feedback events.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct Feedback {
    main_device: Option<Vec<u8>>,
    done: bool,
}

#[cfg(target_os = "linux")]
impl Dispatch<WlRegistry, GlobalListContents> for Feedback {
    fn event(_: &mut Self, _: &WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

#[cfg(target_os = "linux")]
impl Dispatch<ZwpLinuxDmabufV1, ()> for Feedback {
    // Version 4 sends no format or modifier events: the feedback replaces them.
    fn event(_: &mut Self, _: &ZwpLinuxDmabufV1, _: zwp_linux_dmabuf_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

#[cfg(target_os = "linux")]
impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for Feedback {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_dmabuf_feedback_v1::Event::MainDevice { device } => state.main_device = Some(device),
            zwp_linux_dmabuf_feedback_v1::Event::Done => state.done = true,
            _ => {}
        }
    }
}

/// The major and minor numbers of a Linux `dev_t`, as glibc's and musl's `major()` and `minor()`
/// split it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn major_minor(device: u64) -> (u64, u64) {
    let major = ((device & 0x0000_0000_000f_ff00) >> 8) | ((device & 0xffff_f000_0000_0000) >> 32);
    let minor = (device & 0x0000_0000_0000_00ff) | ((device & 0x0000_0fff_fff0_0000) >> 12);
    (major, minor)
}

/// The PCI ids of the GPU behind DRM device `device` (its `cardN` or `renderDN` node), from
/// `<sys_dev_char>/MAJOR:MINOR/device/{vendor,device}`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn pci_ids(sys_dev_char: &std::path::Path, device: u64) -> Option<(u32, u32)> {
    let read_hex = |p: std::path::PathBuf| -> Option<u32> {
        let s = std::fs::read_to_string(p).ok()?;
        u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
    };
    let (major, minor) = major_minor(device);
    let gpu = sys_dev_char.join(format!("{major}:{minor}")).join("device");
    Some((read_hex(gpu.join("vendor"))?, read_hex(gpu.join("device"))?))
}

#[cfg(test)]
mod tests {
    /// glibc's `makedev()`.
    fn makedev(major: u64, minor: u64) -> u64 {
        ((major & 0x0000_0fff) << 8) | ((major & 0xffff_f000) << 32) | (minor & 0x0000_00ff) | ((minor & 0xffff_ff00) << 12)
    }

    #[test]
    fn device_numbers_split_into_major_and_minor() {
        // DRM nodes: card1 is 226:1, renderD128 is 226:128.
        assert_eq!(super::major_minor(0xe201), (226, 1));
        assert_eq!(super::major_minor(0xe280), (226, 128));
        for (major, minor) in [(0, 0), (226, 129), (511, 1_048_575), (0xfff, 0xff), (0x1000, 0x100), (0xffff_ffff, 0xffff_ffff)] {
            assert_eq!(super::major_minor(makedev(major, minor)), (major, minor));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_gpu_behind_a_device_number_comes_from_sysfs() -> std::io::Result<()> {
        let dir = std::env::temp_dir().join(format!("pdfcraft-dev-char-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // /sys/dev/char/226:128 links to the render node, whose `device` is the PCI GPU.
        std::fs::create_dir_all(dir.join("226:128/device"))?;
        std::fs::write(dir.join("226:128/device/vendor"), "0x10de\n")?;
        std::fs::write(dir.join("226:128/device/device"), "0x25ac\n")?;
        // A device that isn't a PCI GPU (a virtual one) has no ids.
        std::fs::create_dir_all(dir.join("226:0/device"))?;
        let render = super::pci_ids(&dir, makedev(226, 128));
        let virtual_gpu = super::pci_ids(&dir, makedev(226, 0));
        let missing = super::pci_ids(&dir, makedev(226, 1));
        std::fs::remove_dir_all(&dir)?;
        assert_eq!(render, Some((0x10de, 0x25ac)));
        assert_eq!(virtual_gpu, None);
        assert_eq!(missing, None);
        Ok(())
    }

    #[test]
    fn the_compositor_names_a_pci_gpu_or_nothing() {
        // Whatever this session has: no compositor (CI), one that doesn't say, or a PCI GPU.
        if let Some((vendor, _)) = super::compositor_gpu() {
            assert_ne!(vendor, 0);
        }
    }
}
