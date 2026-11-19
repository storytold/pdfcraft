/// Tests that open a real GPU device take this first, so only one of them is inside the
/// graphics libraries at a time. The test threads otherwise run them together, and on
/// FreeBSD's CI machine the test process then died with SIGSEGV, in about half the runs:
/// the first device was still being set up when the second test began its own. The UI
/// tests take turns the same way (`crates/ui-egui/tests/it/main.rs`). Hold it for the whole
/// test (declared before the instance, so it's released after the device is dropped).
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> std::sync::MutexGuard<'static, ()> {
    // A test that panicked while holding it leaves nothing to clean up.
    GPU.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Installs the framework runtime on a harness's context, as the app's window does, with a system
/// that opens no OS windows and the interface fonts of English.
pub(crate) fn install_runtime(ctx: &egui::Context) {
    use pdfcraft_ui_egui::i18n::Lang;
    let spec = craft::ui::InstallSpec::new(craft::host::target::ShellInfo::default(), std::sync::Arc::new(craft::window::system::NoWindows));
    craft::ui::shell::embed::install(ctx, spec, &pdfcraft_ui_egui::fonts::ui_fonts(Lang::EN)).expect("the runtime installs on a fresh context");
}

#[test]
fn opengl_is_the_fallback_unless_a_renderer_was_chosen() {
    use super::{RendererChoice, native_options, renderer_choice, retry_with_gl};
    assert_eq!(renderer_choice(None), RendererChoice::Auto);
    assert_eq!(renderer_choice(Some("")), RendererChoice::Auto);
    assert_eq!(renderer_choice(Some("vulkan")), RendererChoice::Auto);
    assert_eq!(renderer_choice(Some(" WGPU ")), RendererChoice::Wgpu);
    for gl in ["gl", "OpenGL", "glow"] {
        assert_eq!(renderer_choice(Some(gl)), RendererChoice::Gl);
    }
    // Retried with OpenGL only when wgpu failed to start by default, never after the app ran.
    assert!(retry_with_gl(RendererChoice::Auto, false));
    assert!(!retry_with_gl(RendererChoice::Auto, true));
    assert!(!retry_with_gl(RendererChoice::Wgpu, false));
    assert!(!retry_with_gl(RendererChoice::Gl, false));
    // Each run states its renderer rather than relying on eframe's default.
    assert_eq!(native_options(false, eframe::Renderer::Wgpu).renderer, eframe::Renderer::Wgpu);
    assert_eq!(native_options(false, eframe::Renderer::Glow).renderer, eframe::Renderer::Glow);
}

#[test]
fn the_window_and_windows_taskbar_icons_load_from_the_bundled_png() {
    let icon = eframe::icon_data::from_png_bytes(super::APP_ICON_PNG).expect("the bundled PNG decodes as the window icon");
    assert!(icon.width > 0 && icon.height > 0);
    assert_eq!(icon.rgba.len(), (icon.width as usize) * (icon.height as usize) * 4);
    #[cfg(not(target_os = "windows"))]
    assert_eq!(super::native_options(false, eframe::Renderer::Glow).viewport.icon.as_deref(), Some(&icon));
    #[cfg(target_os = "windows")]
    {
        // None would load eframe's default icon; only the explicit empty sentinel prevents
        // its first-frame setter from overwriting our full-resolution ICON_BIG.
        for renderer in [eframe::Renderer::Glow, eframe::Renderer::Wgpu] {
            let viewport = super::native_options(false, renderer).viewport;
            assert_eq!(viewport.icon.as_deref(), Some(&egui::IconData::default()));
        }
        assert_eq!((icon.width, icon.height), (256, 256));
        let mut installed = None;
        super::install_taskbar_icon(super::APP_ICON_PNG, |icon| installed = Some(icon));
        assert!(installed.is_some(), "issue #3818: ICON_BIG loads and installs from APP_ICON_PNG");
        let mut bad = None;
        super::install_taskbar_icon(b"not a png", |icon| bad = Some(icon));
        assert!(bad.is_none());
    }
}

#[test]
fn a_software_renderer_is_shown_to_the_user() {
    use eframe::wgpu::DeviceType;
    // The notices the window shows after one frame of `app`.
    fn shown(app: pdfcraft_ui_egui::PdfCraftApp) -> Vec<String> {
        let mut h = egui_kittest::Harness::builder().build_eframe(move |cc| {
            install_runtime(&cc.egui_ctx);
            app
        });
        h.run_steps(2);
        let cards = craft::ui::runtime::toast::cards(&h.ctx, &craft::ui::runtime::env::WindowKey::main());
        // A posted notice's text is the card's title; a message and bullet lines follow it.
        cards
            .iter()
            .map(|card| {
                let toast = &card.toast;
                let parts = toast.title.iter().chain(std::iter::once(&toast.message)).chain(&toast.lines);
                parts.map(|part| part.as_str()).filter(|text| !text.is_empty()).collect::<Vec<_>>().join("\n")
            })
            .collect()
    }
    // Issue #519: WARP or llvmpipe is announced in the app, not just in pdfcraft.log.
    let mut app = crate::startup::test_app();
    super::notify_software_renderer(&mut app, DeviceType::Cpu);
    let notices = shown(app);
    assert!(notices.iter().any(|n| n.contains("without a graphics processor")), "{notices:?}");
    for gpu in [DeviceType::IntegratedGpu, DeviceType::DiscreteGpu, DeviceType::VirtualGpu, DeviceType::Other] {
        let mut app = crate::startup::test_app();
        super::notify_software_renderer(&mut app, gpu);
        let notices = shown(app);
        assert!(notices.is_empty(), "{gpu:?}: {notices:?}");
    }
}

#[test]
fn gpu_setup_avoids_vulkan_and_stretched_resizes_on_windows_and_prefers_low_power() {
    let mut native = eframe::NativeOptions::default();
    super::configure_gpu(&mut native);
    let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &native.wgpu_options.wgpu_setup else { panic!("default setup creates its own instance") };
    if std::env::var_os("WGPU_POWER_PREF").is_none() {
        assert_eq!(setup.power_preference, eframe::wgpu::PowerPreference::LowPower);
    }
    if std::env::var_os("WGPU_VALIDATION_INDIRECT_CALL").is_none() {
        let flags = setup.instance_descriptor.flags;
        assert!(!flags.contains(eframe::wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL), "Intel Metal can't compile its shader: {flags:?}");
    }
    if cfg!(target_os = "windows") && std::env::var_os("WGPU_BACKEND").is_none() {
        let backends = setup.instance_descriptor.backends;
        assert!(backends.contains(eframe::wgpu::Backends::DX12), "{backends:?}");
        assert!(!backends.contains(eframe::wgpu::Backends::VULKAN), "issue #37: {backends:?}");
    }
    use eframe::wgpu::Dx12SwapchainKind::{DxgiFromHwnd, DxgiFromVisual};
    assert_eq!(super::dx12_presentation(None), DxgiFromVisual);
    assert_eq!(super::dx12_presentation(Some(DxgiFromHwnd)), DxgiFromHwnd, "WGPU_DX12_PRESENTATION_SYSTEM still decides");
    assert_eq!(super::dx12_presentation(Some(DxgiFromVisual)), DxgiFromVisual);
    let presentation = setup.instance_descriptor.backend_options.dx12.presentation_system;
    if cfg!(target_os = "windows") {
        // A composition visual unless WGPU_DX12_PRESENTATION_SYSTEM asks otherwise.
        let asked = eframe::wgpu::Dx12SwapchainKind::from_env();
        assert_eq!(presentation, asked.unwrap_or(DxgiFromVisual), "issue #2738: resizing stretches a window swap chain");
    } else {
        assert_eq!(presentation, eframe::wgpu::Dx12SwapchainKind::default().with_env(), "only Windows changes it");
    }
}

#[test]
fn restored_hidpi_window_fits_the_adapters_surface_limit() {
    // The Outlook launch crash: 3440 x 1369 saved points at 250% DPI requested this surface.
    let (width, height) = (8600, 3423);
    let required = eframe::wgpu::Limits::default();
    assert!(width > required.max_texture_dimension_2d);
    let supported = eframe::wgpu::Limits { max_texture_dimension_2d: 16384, ..required.clone() };
    let enabled = super::surface_texture_limits(required, &supported);
    assert!(width <= enabled.max_texture_dimension_2d);
    assert!(height <= enabled.max_texture_dimension_2d);
}

#[test]
fn surface_limits_preserve_other_requirements_and_never_overrequest() {
    for required in [eframe::wgpu::Limits::default(), eframe::wgpu::Limits::downlevel_webgl2_defaults()] {
        for extent in [4096, 8192, 16384] {
            let supported = eframe::wgpu::Limits {
                max_texture_dimension_2d: extent,
                max_bind_groups: 8,
                min_uniform_buffer_offset_alignment: 128,
                min_storage_buffer_offset_alignment: 128,
                ..required.clone()
            };
            let enabled = super::surface_texture_limits(required.clone(), &supported);
            let mut expected = required.clone();
            expected.max_texture_dimension_2d = extent;
            assert_eq!(enabled, expected);
            assert_eq!(enabled.max_texture_dimension_2d, supported.max_texture_dimension_2d);
            assert!(enabled.check_limits(&supported));
        }
    }
}

#[test]
fn surface_limits_fit_v3d_shader_variable_limit() {
    // Issue #2383: V3D supports 15 variables, below the default request of 16.
    let required = eframe::wgpu::Limits::default();
    let supported = eframe::wgpu::Limits { max_texture_dimension_2d: 7680, max_inter_stage_shader_variables: 15, ..required.clone() };
    let enabled = super::surface_texture_limits(required.clone(), &supported);
    assert!(enabled.check_limits(&supported), "unsupported device limits: {enabled:?}");
    let expected = eframe::wgpu::Limits { max_texture_dimension_2d: 7680, max_inter_stage_shader_variables: 15, ..required };
    assert_eq!(enabled, expected);
}

#[test]
fn surface_limits_fit_downlevel_maxima_and_minimum_alignments() {
    let required = eframe::wgpu::Limits::downlevel_webgl2_defaults();
    let supported = eframe::wgpu::Limits {
        max_texture_dimension_2d: 2048,
        max_bind_groups: 2,
        max_vertex_attributes: 8,
        min_uniform_buffer_offset_alignment: 512,
        min_storage_buffer_offset_alignment: 512,
        ..required.clone()
    };
    let enabled = super::surface_texture_limits(required, &supported);
    assert!(enabled.check_limits(&supported), "unsupported device limits: {enabled:?}");
    // Larger minimum alignments are weaker capabilities, unlike maximum limits.
    assert_eq!(enabled, supported);
}

#[test]
fn a_window_larger_than_the_gpu_limit_gets_a_surface_within_it() {
    use eframe::egui_wgpu::winit::surface_fit;
    // Issue #577: 3440 x 1369 points restored at 250% asked an 8192-pixel device for this.
    let (width, height, scale) = surface_fit(8600, 3423, 8192);
    assert_eq!((width, height), (8192, 3260));
    // Both sides shrink alike and egui draws at that factor: the whole window is drawn.
    assert!((8600.0 * scale - 8192.0).abs() < 0.01, "{scale}");
    assert!((3423.0 * scale - height as f32).abs() < 1.0, "{scale}");
    // Within the limit nothing changes, up to and including the limit itself.
    assert_eq!(surface_fit(8600, 3423, 16384), (8600, 3423, 1.0));
    assert_eq!(surface_fit(8192, 8192, 8192), (8192, 8192, 1.0));
    assert_eq!(surface_fit(0, 0, 8192), (0, 0, 1.0));
    // One side over the limit: the window as it would be stretched across monitors.
    assert_eq!(surface_fit(8193, 600, 8192).0, 8192);
    assert_eq!(surface_fit(600, 8193, 8192).1, 8192);
}

#[test]
fn surface_fit_never_empties_or_overflows_a_surface() {
    use eframe::egui_wgpu::winit::surface_fit;
    for (w, h, max) in
        [(1, u32::MAX, 8192), (u32::MAX, 1, 2048), (u32::MAX, u32::MAX, 16384), (0, u32::MAX, 8192), (u32::MAX, 0, 8192), (5, 7, 0), (40_000, 3, 1)]
    {
        let (fw, fh, scale) = surface_fit(w, h, max);
        assert!(fw <= max.max(1) && fh <= max.max(1), "{w} x {h} in {max}: {fw} x {fh}");
        // A side that wasn't zero stays non-zero (`Surface::configure` rejects an empty one),
        // and a zero side stays zero (egui-wgpu skips configuring it).
        assert_eq!((fw == 0, fh == 0), (w == 0, h == 0), "{w} x {h} in {max}: {fw} x {fh}");
        assert!(scale.is_finite() && scale > 0.0 && scale <= 1.0, "{w} x {h} in {max}: {scale}");
    }
    // The longer side lands exactly on the limit (rounding never leaves it a pixel short),
    // and the shorter side keeps the window's proportions to within a pixel.
    for max in [2048, 8192, 16384] {
        for long in (max + 1..=max * 5).step_by(997).chain([max * 2, max * 4, u32::MAX]) {
            for short in [1, 3, 600, max / 3, max - 1, max, long - 1, long] {
                let (fw, fh, scale) = surface_fit(long, short, max);
                assert_eq!(fw, max, "{long} x {short} in {max}");
                let expected = f64::from(short) * f64::from(max) / f64::from(long);
                assert!((f64::from(fh) - expected).abs() <= 1.0, "{long} x {short} in {max}: {fh}");
                assert_eq!(surface_fit(short, long, max), (fh, fw, scale), "transposed");
            }
        }
    }
}

#[test]
fn a_gpu_error_while_a_window_is_set_up_is_returned_not_a_panic() {
    // Issue #519: on a 2015 Intel GPU, wgpu's GL backend couldn't configure the window's
    // surface (`GpuWaitTimeout`), and wgpu's default error handler panicked before the app was
    // created, so PdfCraft never got to retry with OpenGL. vendor/egui-wgpu configures a new
    // window's surface inside `catch_errors`. A real device on PdfCraft's own GPU settings,
    // and an error wgpu raises before anything reaches the driver: a texture one pixel wider
    // than the device allows.
    use eframe::wgpu;
    let _gpu = gpu();
    let native = super::native_options(false, eframe::Renderer::Wgpu);
    let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &native.wgpu_options.wgpu_setup else { panic!("default setup creates its own instance") };
    let instance = pollster::block_on(native.wgpu_options.wgpu_setup.new_instance());
    let options = wgpu::RequestAdapterOptions { power_preference: setup.power_preference, ..Default::default() };
    // CI runners have a software adapter (WARP, llvmpipe); a machine without any skips.
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&options)) else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let Ok((device, _queue)) = pollster::block_on(adapter.request_device(&(setup.device_descriptor)(&adapter))) else {
        eprintln!("skipping: the adapter gives no device");
        return;
    };
    let texture = |width: u32| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("issue-519"),
            size: wgpu::Extent3d { width, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    };
    let too_wide = device.limits().max_texture_dimension_2d.saturating_add(1);
    let caught = pollster::block_on(eframe::egui_wgpu::winit::catch_errors(&device, || texture(too_wide)));
    assert!(matches!(caught, Err(wgpu::Error::Validation { .. })), "{caught:?}");
    // The device stays usable, and nothing is left behind to catch later errors by mistake.
    let fine = pollster::block_on(eframe::egui_wgpu::winit::catch_errors(&device, || texture(16)));
    assert!(fine.is_ok(), "{fine:?}");
    assert_eq!(pollster::block_on(eframe::egui_wgpu::winit::catch_errors(&device, || 7)).ok(), Some(7));
    // Nested: the inner call keeps its own error, and the outer one still catches what follows.
    let caught = pollster::block_on(eframe::egui_wgpu::winit::catch_errors(&device, || {
        let inner = pollster::block_on(eframe::egui_wgpu::winit::catch_errors(&device, || texture(too_wide)));
        assert!(inner.is_err(), "inner: {inner:?}");
        texture(too_wide)
    }));
    assert!(matches!(caught, Err(wgpu::Error::Validation { .. })), "outer: {caught:?}");
}

#[test]
fn pipeline_creation_error_is_returned_and_device_remains_usable() {
    use eframe::{egui_wgpu, wgpu};
    let _gpu = gpu();
    let mut native = super::native_options(false, eframe::Renderer::Wgpu);
    let egui_wgpu::WgpuSetup::CreateNew(setup) = &native.wgpu_options.wgpu_setup else { panic!("default setup creates its own instance") };
    let instance = pollster::block_on(native.wgpu_options.wgpu_setup.new_instance());
    let options = wgpu::RequestAdapterOptions { power_preference: setup.power_preference, ..Default::default() };
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&options)) else {
        assert!(std::env::var_os("PDFCRAFT_TEST_REQUIRE_GPU").is_none(), "a GPU adapter is required for this run");
        eprintln!("skipping pipeline regression: no GPU adapter");
        return;
    };
    let (device, queue) =
        pollster::block_on(adapter.request_device(&(setup.device_descriptor)(&adapter))).expect("the selected adapter must provide a test device");
    native.wgpu_options.wgpu_setup =
        egui_wgpu::WgpuSetup::Existing(egui_wgpu::WgpuSetupExisting { instance: instance.clone(), adapter, device, queue });
    // Three samples is invalid on every backend, so this reproduces a pipeline error
    // without depending on a particular driver. The unpatched constructor panics.
    for samples in [3, 1, 3, 1] {
        let options = egui_wgpu::RendererOptions { msaa_samples: samples, ..Default::default() };
        let result = pollster::block_on(egui_wgpu::RenderState::create(&native.wgpu_options, &instance, None, options));
        if samples == 3 {
            let error = result.err().expect("invalid pipeline must return an error");
            assert!(error.to_string().contains("egui_pipeline"), "{error}");
        } else {
            assert!(result.is_ok(), "the device must remain usable: {:?}", result.err());
        }
    }
}

const NVIDIA: (u32, u32) = (0x10de, 0x2684);
const NVIDIA_3090: (u32, u32) = (0x10de, 0x2204);
const AMD_IGPU: (u32, u32) = (0x1002, 0x164e);

fn adapters() -> Vec<(u32, u32, eframe::wgpu::DeviceType)> {
    use eframe::wgpu::DeviceType;
    vec![(NVIDIA.0, NVIDIA.1, DeviceType::DiscreteGpu), (AMD_IGPU.0, AMD_IGPU.1, DeviceType::IntegratedGpu), (0, 0, DeviceType::Cpu)]
}

const INTEL_IGPU: (u32, u32) = (0x8086, 0xa780);

fn monitor(pci: (u32, u32)) -> super::DisplayGpu {
    super::DisplayGpu { pci, primary: false }
}

fn panel(pci: (u32, u32)) -> super::DisplayGpu {
    super::DisplayGpu { pci, primary: true }
}

#[test]
fn pick_adapter_prefers_the_gpu_driving_the_monitors() {
    // A desktop whose monitors are all on the discrete GPU: the integrated one shows black.
    assert_eq!(super::pick_adapter(&adapters(), &[monitor(NVIDIA)], None), Some(0));
    // Issue #378: the same on Windows, where the Ryzen integrated GPU without a monitor crashed
    // the display driver. Windows also says which display is the primary one.
    assert_eq!(super::pick_adapter(&adapters(), &[panel(NVIDIA)], None), Some(0));
}

#[test]
fn pick_adapter_prefers_the_gpu_driving_the_primary_display() {
    // A Windows desktop with a monitor on each GPU: the window opens on the primary display.
    assert_eq!(super::pick_adapter(&adapters(), &[monitor(AMD_IGPU), panel(NVIDIA)], None), Some(0));
    assert_eq!(super::pick_adapter(&adapters(), &[panel(AMD_IGPU), monitor(NVIDIA)], None), Some(1));
}

#[test]
fn pci_ids_come_from_windows_device_ids() {
    assert_eq!(super::pci_ids("PCI\\VEN_10DE&DEV_2204&SUBSYS_40421458&REV_A1"), Some(NVIDIA_3090));
    assert_eq!(super::pci_ids("PCI\\VEN_1002&DEV_164E&SUBSYS_88771043&REV_C1"), Some(AMD_IGPU));
    assert_eq!(super::pci_ids("pci\\ven_1002&dev_164e"), Some(AMD_IGPU));
    // Remote Desktop and other non-PCI display devices, and malformed ids.
    assert_eq!(super::pci_ids("ROOT\\BasicDisplay\\0000"), None);
    assert_eq!(super::pci_ids("PCI\\VEN_10DE&SUBSYS_40421458"), None);
    assert_eq!(super::pci_ids("PCI\\VEN_10DE&DEV_ZZZZ"), None);
    assert_eq!(super::pci_ids("VEN_&DEV_"), None);
    assert_eq!(super::pci_ids(""), None);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_display_gpus_lists_pci_gpus_with_a_monitor() {
    // Whatever this machine has: every entry is a PCI GPU, at most one drives the primary
    // display, and no GPU is listed twice. (A headless CI runner may list none.)
    let gpus = super::windows_display_gpus();
    assert!(gpus.iter().filter(|g| g.primary).count() <= 1, "{gpus:?}");
    for (i, g) in gpus.iter().enumerate() {
        assert!(g.pci.0 != 0, "{gpus:?}");
        assert!(!gpus[..i].iter().any(|h| h.pci == g.pci), "{gpus:?}");
    }
}

#[test]
fn pick_adapter_keeps_the_integrated_gpu_on_hybrid_laptops() {
    // Issue #8: the panel is on the integrated GPU, an external monitor on the discrete one.
    assert_eq!(super::pick_adapter(&adapters(), &[monitor(NVIDIA), panel(AMD_IGPU)], None), Some(1));
    assert_eq!(super::pick_adapter(&adapters(), &[panel(AMD_IGPU), monitor(NVIDIA)], None), Some(1));
    assert_eq!(super::pick_adapter(&adapters(), &[panel(AMD_IGPU)], None), Some(1));
    assert_eq!(super::pick_adapter(&adapters(), &[monitor(AMD_IGPU)], None), Some(1));
}

#[test]
fn pick_adapter_prefers_the_discrete_gpu_on_desktops_with_a_monitor_on_each() {
    // Issue #445: one monitor on the Intel iGPU, one on the NVIDIA card, no built-in panel.
    use eframe::wgpu::DeviceType;
    let desktop = [(INTEL_IGPU.0, INTEL_IGPU.1, DeviceType::IntegratedGpu), (NVIDIA.0, NVIDIA.1, DeviceType::DiscreteGpu), (0, 0, DeviceType::Cpu)];
    assert_eq!(super::pick_adapter(&desktop, &[monitor(INTEL_IGPU), monitor(NVIDIA)], None), Some(1));
    assert_eq!(super::pick_adapter(&desktop, &[monitor(NVIDIA), monitor(INTEL_IGPU)], None), Some(1));
    // A GPU driving no display still loses to one that does.
    assert_eq!(super::pick_adapter(&adapters(), &[monitor(AMD_IGPU), monitor(INTEL_IGPU)], None), Some(1));
}

#[test]
fn pick_adapter_prefers_the_gpu_the_compositor_draws_with() {
    // A hybrid laptop whose Wayland compositor runs on the discrete GPU (Hyprland with
    // `AQ_DRM_DEVICES` naming it first): the integrated GPU drives the built-in panel, yet the
    // compositor shows nothing it draws, and reports no error either.
    assert_eq!(super::pick_adapter(&adapters(), &[panel(AMD_IGPU)], Some(NVIDIA)), Some(0));
    assert_eq!(super::pick_adapter(&adapters(), &[panel(AMD_IGPU), monitor(NVIDIA)], Some(NVIDIA)), Some(0));
    // The usual hybrid laptop, with the compositor on the integrated GPU: as before (#8).
    assert_eq!(super::pick_adapter(&adapters(), &[panel(AMD_IGPU), monitor(NVIDIA)], Some(AMD_IGPU)), Some(1));
    // No GPU drives a display as far as sysfs says (a container), but the compositor knows.
    assert_eq!(super::pick_adapter(&adapters(), &[], Some(NVIDIA)), Some(0));
    // A compositor GPU no adapter matches (a virtual one) leaves the displays in charge.
    assert_eq!(super::pick_adapter(&adapters(), &[panel(AMD_IGPU)], Some((0x1af4, 0x1050))), Some(1));
}

#[test]
fn pick_adapter_falls_back_to_low_power_without_a_match() {
    assert_eq!(super::pick_adapter(&adapters(), &[monitor((0x8086, 0x1234))], None), Some(1));
    assert_eq!(super::pick_adapter(&adapters(), &[monitor((0x8086, 0x1234)), monitor((0x8086, 0x5678))], None), Some(1));
    assert_eq!(super::pick_adapter(&[], &[monitor(NVIDIA)], None), None);
}

#[test]
fn internal_panels_are_edp_lvds_and_dsi_connectors() {
    for name in ["eDP-1", "LVDS-1", "DSI-1"] {
        assert!(super::is_internal_panel(name), "{name}");
    }
    for name in ["DP-3", "HDMI-A-1", "DVI-D-1", "VGA-1", "Writeback-1", ""] {
        assert!(!super::is_internal_panel(name), "{name}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_display_gpus_reads_connected_connectors() -> std::io::Result<()> {
    let dir = std::env::temp_dir().join(format!("pdfcraft-drm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let card = |name: &str, (vendor, device): (u32, u32)| -> std::io::Result<()> {
        std::fs::create_dir_all(dir.join(name).join("device"))?;
        std::fs::write(dir.join(name).join("device/vendor"), format!("{vendor:#06x}\n"))?;
        std::fs::write(dir.join(name).join("device/device"), format!("{device:#06x}\n"))
    };
    let connector = |name: &str, status: &str| -> std::io::Result<()> {
        std::fs::create_dir_all(dir.join(name))?;
        std::fs::write(dir.join(name).join("status"), format!("{status}\n"))
    };
    card("card1", NVIDIA)?;
    card("card2", AMD_IGPU)?;
    connector("card1-DP-3", "connected")?;
    connector("card1-DP-4", "connected")?;
    connector("card2-HDMI-A-1", "disconnected")?;
    connector("card2-Writeback-1", "unknown")?;
    let desktop = super::linux_display_gpus(&dir);
    // Issue #445: a monitor on the integrated GPU too, still no built-in panel.
    connector("card2-HDMI-A-1", "connected")?;
    let mut both = super::linux_display_gpus(&dir);
    // A hybrid laptop: the integrated GPU drives the built-in panel as well.
    connector("card2-eDP-1", "connected")?;
    let mut laptop = super::linux_display_gpus(&dir);
    std::fs::remove_dir_all(&dir)?;
    assert_eq!(desktop, vec![monitor(NVIDIA)]);
    both.sort_by_key(|g| g.pci);
    assert_eq!(both, vec![monitor(AMD_IGPU), monitor(NVIDIA)]);
    laptop.sort_by_key(|g| g.pci);
    assert_eq!(laptop, vec![panel(AMD_IGPU), monitor(NVIDIA)]);
    assert!(super::linux_display_gpus(std::path::Path::new("/nonexistent/drm")).is_empty());
    Ok(())
}

/// X11 startup notification and docks match a window to its launcher by `StartupWMClass`,
/// so it has to be the class the window carries: the app id, on Wayland and X11 alike.
#[test]
fn the_desktop_entry_names_the_window_class() {
    let entry = include_str!("../../../packaging/linux/ai.storyteller.pdfcraft.desktop");
    let class = entry.lines().find_map(|line| line.strip_prefix("StartupWMClass="));
    assert_eq!(class, Some(super::APP_ID));
}

/// Both flatpak manifests build with the craft-i18n commit `craft-i18n.rev` pins: CI and
/// releases read the file, and the manifests have to name the commit themselves. The pin may
/// have a trailing newline or CRLF (a Windows checkout), so it is trimmed, as every reader does.
#[test]
fn the_flatpak_manifests_pin_the_craft_i18n_rev() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let rev = std::fs::read_to_string(root.join("craft-i18n.rev")).unwrap();
    let rev = rev.trim();
    assert!(rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()), "craft-i18n.rev holds one full SHA: {rev:?}");
    for manifest in ["ai.storyteller.pdfcraft.yml", "ai.storyteller.pdfcraft.bundle.yml"] {
        let text = std::fs::read_to_string(root.join("packaging/linux/flatpak").join(manifest)).unwrap();
        let source =
            text.split("url: https://github.com/storytold/craft-i18n").nth(1).unwrap_or_else(|| panic!("{manifest}: a craft-i18n git source"));
        let source = source.split("- type:").next().unwrap_or(source);
        let commit = source
            .lines()
            .find_map(|l| l.trim().strip_prefix("commit:"))
            .unwrap_or_else(|| panic!("{manifest}: the craft-i18n source pins a commit"));
        assert_eq!(commit.trim().trim_matches(['\'', '"']), rev, "{manifest}");
        assert!(text.contains("CRAFT_I18N_DIR: /run/build/pdfcraft/craft-i18n"), "{manifest}: the build names the catalogs");
    }
}

// Regression test for #246: `eprintln!` panics when standard error is a broken pipe (a GUI
// launch on Windows), and a panic inside the panic hook aborts the process. The start-up
// path and the hook must write to standard error without it.
#[test]
fn start_up_and_the_panic_hook_never_use_eprintln() {
    let source = include_str!("main.rs");
    // Everything before the top-level test module is product code.
    let product = source.split("\n#[cfg(test)]\nmod tests;").next().unwrap_or(source);
    assert!(product.len() < source.len(), "found the end of the product code");
    let offenders: Vec<_> = product
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//") && line.contains("eprintln!"))
        .map(|(n, line)| format!("main.rs:{}: {}", n + 1, line.trim()))
        .collect();
    assert!(
        offenders.is_empty(),
        "`eprintln!` panics on a broken standard error pipe; use `log::warn!` or `write_fmt` on `stderr()` and ignore the error:\n{}",
        offenders.join("\n")
    );
}
