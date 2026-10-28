//! What this computer is, as far as choosing a download goes: the processor, and which graphics
//! cards it has. Read from the kernel's own files (`/sys/class/drm`, the NVIDIA module's
//! version), never from a name this machine happens to carry; the person may force a backend.

use std::path::Path;

/// The processor family a build is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    /// 64-bit x86.
    X86_64,
    /// 64-bit ARM.
    Aarch64,
}

impl Arch {
    /// The architecture this program was built for; `None` for one no engine is built for.
    pub fn current() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Self::X86_64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Self::Aarch64)
        } else {
            None
        }
    }
}

/// What an engine's build computes with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    /// The processor alone.
    Cpu,
    /// Any graphics card with a Vulkan driver.
    Vulkan,
    /// NVIDIA's CUDA.
    Cuda,
    /// AMD's ROCm.
    Rocm,
}

impl Backend {
    /// The name in a directory, a request and a setting.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Vulkan => "vulkan",
            Self::Cuda => "cuda",
            Self::Rocm => "rocm",
        }
    }

    /// The backend a slug names; `None` for any other text.
    pub fn from_slug(text: &str) -> Option<Self> {
        [Self::Cpu, Self::Vulkan, Self::Cuda, Self::Rocm]
            .into_iter()
            .find(|backend| backend.slug() == text)
    }
}

/// Who made a graphics card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    /// NVIDIA (PCI vendor 0x10de).
    Nvidia,
    /// AMD (0x1002).
    Amd,
    /// Intel (0x8086).
    Intel,
    /// Any other: a virtual card, say, that no engine build is for.
    Other,
}

impl GpuVendor {
    fn of_pci(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "0x10de" => Self::Nvidia,
            "0x1002" => Self::Amd,
            "0x8086" => Self::Intel,
            _ => Self::Other,
        }
    }
}

/// The graphics cards of this computer, and the NVIDIA driver's major version when it is loaded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gpus {
    /// One entry per card the kernel lists.
    pub vendors: Vec<GpuVendor>,
    /// The loaded NVIDIA driver's major version (`580` for `580.126.09`).
    pub nvidia_driver: Option<u32>,
}

impl Gpus {
    /// Reads `drm` (`/sys/class/drm`) for the cards and `nvidia_version` (the module's
    /// `version` file under `/sys/module/nvidia`) for the driver. A file that is not there is
    /// no card and no driver.
    pub fn read(drm: &Path, nvidia_version: &Path) -> Self {
        let mut names: Vec<String> = std::fs::read_dir(drm)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| is_card(name))
            .collect();
        names.sort();
        let vendors = names
            .iter()
            .filter_map(|name| std::fs::read_to_string(drm.join(name).join("device/vendor")).ok())
            .map(|text| GpuVendor::of_pci(&text))
            .collect();
        let nvidia_driver = std::fs::read_to_string(nvidia_version)
            .ok()
            .and_then(|text| text.split('.').next()?.trim().parse().ok());
        Self {
            vendors,
            nvidia_driver,
        }
    }

    fn has(&self, vendor: GpuVendor) -> bool {
        self.vendors.contains(&vendor)
    }
}

/// `card0`, `card1`: a card, not one of its connectors (`card0-HDMI-A-1`) or a render node.
fn is_card(name: &str) -> bool {
    name.strip_prefix("card")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether the person forced a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendChoice {
    /// Pick by the cards found.
    #[default]
    Detect,
    /// Use this one, whatever is found.
    Force(Backend),
}

/// The backends to try, best first. A forced backend is the only one; otherwise NVIDIA cards
/// prefer CUDA, any card Vulkan, and the processor alone is the last resort. ROCm is only ever
/// forced: it wants a matching system runtime that cannot be told from here.
pub fn preference(gpus: &Gpus, choice: BackendChoice) -> Vec<Backend> {
    match choice {
        BackendChoice::Force(backend) => vec![backend],
        BackendChoice::Detect => {
            let mut order = Vec::new();
            if gpus.has(GpuVendor::Nvidia) {
                order.push(Backend::Cuda);
            }
            if gpus.has(GpuVendor::Nvidia) || gpus.has(GpuVendor::Amd) || gpus.has(GpuVendor::Intel)
            {
                order.push(Backend::Vulkan);
            }
            order.push(Backend::Cpu);
            order
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_kit::Scratch;

    fn card(drm: &Path, name: &str, vendor: &str) {
        let dir = drm.join(name).join("device");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("vendor"), format!("{vendor}\n")).expect("vendor");
    }

    #[test]
    fn cards_are_read_from_the_kernels_files_and_connectors_are_not_cards() {
        let dir = Scratch::new("host-drm");
        card(dir.path(), "card0", "0x1002");
        card(dir.path(), "card1", "0x10de");
        card(dir.path(), "card0-HDMI-A-1", "0x8086");
        card(dir.path(), "renderD128", "0x8086");
        let version = dir.path().join("version");
        std::fs::write(&version, "580.126.09\n").expect("version");
        let gpus = Gpus::read(dir.path(), &version);
        assert_eq!(gpus.vendors, [GpuVendor::Amd, GpuVendor::Nvidia]);
        assert_eq!(gpus.nvidia_driver, Some(580));
    }

    #[test]
    fn nothing_listed_is_no_card_and_no_driver() {
        let dir = Scratch::new("host-none");
        let gpus = Gpus::read(&dir.path().join("gone"), &dir.path().join("none"));
        assert_eq!(gpus, Gpus::default());
    }

    #[test]
    fn the_backend_order_follows_the_cards_and_a_forced_one_stands_alone() {
        let gpus = |vendors: &[GpuVendor]| Gpus {
            vendors: vendors.to_vec(),
            nvidia_driver: None,
        };
        let cases = [
            (gpus(&[]), vec![Backend::Cpu]),
            (gpus(&[GpuVendor::Other]), vec![Backend::Cpu]),
            (gpus(&[GpuVendor::Amd]), vec![Backend::Vulkan, Backend::Cpu]),
            (
                gpus(&[GpuVendor::Amd, GpuVendor::Nvidia]),
                vec![Backend::Cuda, Backend::Vulkan, Backend::Cpu],
            ),
        ];
        for (found, expected) in cases {
            assert_eq!(preference(&found, BackendChoice::Detect), expected);
        }
        assert_eq!(
            preference(&gpus(&[]), BackendChoice::Force(Backend::Rocm)),
            [Backend::Rocm]
        );
        assert_eq!(Backend::from_slug("vulkan"), Some(Backend::Vulkan));
        assert_eq!(Backend::from_slug("Vulkan"), None);
    }
}
