//! Opening the window, on whatever graphics the machine has.
//!
//! The window draws through OpenGL first: it is quick to start and small, and
//! nearly every machine has OpenGL 2.0. Some do not. An Intel GMA 4 series
//! under Windows 11 has a WDDM 1.1 driver with OpenGL 1.x at most, and eframe
//! then stops with "egui_glow requires opengl 2.0+" before anything is drawn
//! (issue 52). On non-x86 Windows / Unix targets, when OpenGL fails before
//! the window is up, it is opened again through wgpu: Direct3D 12 or Vulkan
//! on the graphics card where they work, and otherwise WARP, the software
//! Direct3D every Windows 10 and 11 machine has, which needs no driver at all.
//!
//! On Windows x86 / XP targets, wgpu is omitted so the binary stays clear of
//! post-XP graphics APIs and DLLs; if OpenGL 2.0+ is not available from the
//! driver, a compatible software `opengl32.dll` can be placed alongside the
//! executable.
//!
//! `BINMODEM_RENDERER` picks one outright: `glow`, or (where enabled) `wgpu`
//! or `warp` for wgpu on the software adapter.

use std::cell::RefCell;
use std::rc::Rc;
#[cfg(not(all(target_os = "windows", target_arch = "x86")))]
use std::sync::Arc;

#[cfg(not(all(target_os = "windows", target_arch = "x86")))]
use eframe::egui_wgpu::{WgpuSetup, WgpuSetupCreateNew};
#[cfg(not(all(target_os = "windows", target_arch = "x86")))]
use eframe::wgpu;

/// What to draw through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Renderer {
    Glow,
    #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
    Wgpu { software: bool },
}

impl Renderer {
    /// `BINMODEM_RENDERER`, if it names one.
    fn from_env() -> Option<Self> {
        Self::named(&std::env::var("BINMODEM_RENDERER").ok()?)
    }

    fn named(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "glow" | "opengl" | "gl" => Some(Self::Glow),
            #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
            "wgpu" => Some(Self::Wgpu { software: false }),
            #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
            "warp" | "software" => Some(Self::Wgpu { software: true }),
            _ => None,
        }
    }

    fn apply(self, options: &mut eframe::NativeOptions) {
        match self {
            Self::Glow => {
                options.renderer = eframe::Renderer::Glow;
            }
            #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
            Self::Wgpu { software } => {
                options.renderer = eframe::Renderer::Wgpu;
                let mut setup = WgpuSetupCreateNew::without_display_handle();
                setup.native_adapter_selector =
                    Some(Arc::new(move |adapters, surface| choose(adapters, surface, software)));
                options.wgpu_options.wgpu_setup = WgpuSetup::CreateNew(setup);
            }
        }
    }
}

/// A graphics card that can draw to the window if there is one, and the
/// software adapter if not (or first, when `software` asks for it).
#[cfg(not(all(target_os = "windows", target_arch = "x86")))]
fn choose(
    adapters: &[wgpu::Adapter],
    surface: Option<&wgpu::Surface<'_>>,
    software: bool,
) -> Result<wgpu::Adapter, String> {
    let usable: Vec<&wgpu::Adapter> = adapters
        .iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .collect();
    let is_software = |a: &&wgpu::Adapter| a.get_info().device_type == wgpu::DeviceType::Cpu;
    let hardware = usable.iter().find(|a| !is_software(a));
    let fallback = usable.iter().find(|a| is_software(a));
    let pick = if software {
        fallback.or(hardware)
    } else {
        hardware.or(fallback)
    };
    match pick {
        Some(a) => {
            let info = a.get_info();
            eprintln!(
                "drawing through {:?} on {} ({:?})",
                info.backend, info.name, info.device_type
            );
            Ok((*a).clone())
        }
        None => Err(format!(
            "no graphics adapter can draw to the window (found: {})",
            adapters
                .iter()
                .map(|a| a.get_info().name)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// `eframe::run_native`, falling back from OpenGL to wgpu where supported.
///
/// `options` is asked for twice when the first attempt fails, since a
/// `NativeOptions` is used up by a run.
pub fn run<'app>(
    title: &str,
    options: impl Fn() -> eframe::NativeOptions,
    app_creator: impl FnOnce(&eframe::CreationContext<'_>) -> Result<Box<dyn eframe::App + 'app>, Box<dyn std::error::Error + Send + Sync>>
        + 'app,
) -> eframe::Result {
    let chosen = Renderer::from_env();
    let first = chosen.unwrap_or(Renderer::Glow);
    // The creator runs once the window is up. Held here, it is still here
    // when a renderer fails before that, for the next one to use.
    let slot = Rc::new(RefCell::new(Some(app_creator)));
    let attempt = |renderer: Renderer| {
        let mut o = options();
        renderer.apply(&mut o);
        let slot = slot.clone();
        eframe::run_native(
            title,
            o,
            Box::new(move |cc| match slot.borrow_mut().take() {
                Some(create) => create(cc),
                None => Err("the window was already made".into()),
            }),
        )
    };
    match attempt(first) {
        #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
        Err(e) if first == Renderer::Glow && chosen.is_none() && slot.borrow().is_some() => {
            eprintln!(
                "OpenGL would not open the window ({e}); trying Direct3D or Vulkan through wgpu instead"
            );
            attempt(Renderer::Wgpu { software: false })
        }
        #[cfg(all(target_os = "windows", target_arch = "x86"))]
        Err(e) if first == Renderer::Glow => {
            eprintln!(
                "OpenGL 2.0+ could not be initialized ({e}). On Windows x86 / XP without OpenGL 2.0+ support, place a compatible opengl32.dll next to the executable."
            );
            Err(e)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderers_are_known_by_these_names() {
        for (name, want) in [
            ("glow", Some(Renderer::Glow)),
            ("OpenGL", Some(Renderer::Glow)),
            #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
            ("wgpu", Some(Renderer::Wgpu { software: false })),
            #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
            (" warp ", Some(Renderer::Wgpu { software: true })),
            ("vulkan", None),
        ] {
            assert_eq!(Renderer::named(name), want, "{name}");
        }
    }
}
