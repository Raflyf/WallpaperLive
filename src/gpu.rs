use crate::core::GpuPreference;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};

pub const LOW_MEMORY_UMA_LIMIT: usize = 512 << 20;
pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_INTEL: u32 = 0x8086;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeBackend {
    Nvenc,
    Amf,
    Qsv,
    CpuX264,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterChoice {
    pub name: Option<String>,
    pub low_memory_uma: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterCandidate {
    pub name: String,
    pub vendor_id: u32,
    pub dedicated_vram: usize,
    pub outputs: Vec<String>,
}

fn hardware(desc: &DXGI_ADAPTER_DESC1) -> bool {
    desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0
}

fn trim_utf16(slice: &[u16]) -> String {
    let end = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
    String::from_utf16_lossy(&slice[..end])
}

fn matches_monitor(candidate: &AdapterCandidate, monitor: Option<&str>) -> bool {
    monitor.is_some_and(|target| {
        candidate
            .outputs
            .iter()
            .any(|output| output.eq_ignore_ascii_case(target))
    })
}

pub fn select_adapter(
    candidates: &[AdapterCandidate],
    monitor: Option<&str>,
    pref: GpuPreference,
) -> AdapterChoice {
    if candidates.is_empty() {
        return AdapterChoice {
            name: None,
            low_memory_uma: true,
        };
    }
    let chosen = match pref {
        GpuPreference::Auto => candidates
            .iter()
            .filter(|c| c.dedicated_vram > LOW_MEMORY_UMA_LIMIT)
            .max_by_key(|c| (c.dedicated_vram, matches_monitor(c, monitor)))
            .or_else(|| candidates.iter().find(|c| matches_monitor(c, monitor)))
            .or_else(|| candidates.iter().max_by_key(|c| c.dedicated_vram)),
        GpuPreference::PowerSaving => candidates
            .iter()
            .min_by_key(|c| (c.dedicated_vram, !matches_monitor(c, monitor))),
        GpuPreference::HighPerformance => candidates
            .iter()
            .max_by_key(|c| (c.dedicated_vram, matches_monitor(c, monitor))),
    };
    match chosen {
        Some(adapter) => AdapterChoice {
            name: (!adapter.name.is_empty()).then(|| adapter.name.clone()),
            low_memory_uma: adapter.dedicated_vram <= LOW_MEMORY_UMA_LIMIT,
        },
        None => AdapterChoice {
            name: None,
            low_memory_uma: true,
        },
    }
}

fn vendor_encoder(vendor_id: u32) -> Option<EncodeBackend> {
    match vendor_id {
        VENDOR_NVIDIA => Some(EncodeBackend::Nvenc),
        VENDOR_AMD => Some(EncodeBackend::Amf),
        VENDOR_INTEL => Some(EncodeBackend::Qsv),
        _ => None,
    }
}

pub fn select_encode_backends(
    candidates: &[AdapterCandidate],
    pref: GpuPreference,
) -> Vec<EncodeBackend> {
    let mut ordered: Vec<&AdapterCandidate> = candidates.iter().collect();
    ordered.sort_by(|a, b| match pref {
        GpuPreference::Auto | GpuPreference::HighPerformance => {
            (b.dedicated_vram > LOW_MEMORY_UMA_LIMIT, b.dedicated_vram)
                .cmp(&(a.dedicated_vram > LOW_MEMORY_UMA_LIMIT, a.dedicated_vram))
        }
        GpuPreference::PowerSaving => (a.dedicated_vram > LOW_MEMORY_UMA_LIMIT, a.dedicated_vram)
            .cmp(&(b.dedicated_vram > LOW_MEMORY_UMA_LIMIT, b.dedicated_vram)),
    });
    let mut backends = Vec::new();
    for candidate in ordered {
        if let Some(backend) = vendor_encoder(candidate.vendor_id)
            && !backends.contains(&backend)
        {
            backends.push(backend);
        }
    }
    backends.push(EncodeBackend::CpuX264);
    backends
}

fn enumerate_adapters() -> Vec<AdapterCandidate> {
    // SAFETY: read-only DXGI adapter/output enumeration; COM references released by windows crate.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for index in 0..64 {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if !hardware(&desc) {
            continue;
        }
        let name = trim_utf16(&desc.Description);
        if name.is_empty() {
            continue;
        }
        let mut outputs = Vec::new();
        for out_idx in 0..32 {
            let Ok(output) = (unsafe { adapter.EnumOutputs(out_idx) }) else {
                break;
            };
            if let Ok(out_desc) = unsafe { output.GetDesc() } {
                let device_name = trim_utf16(&out_desc.DeviceName);
                if !device_name.is_empty() {
                    outputs.push(device_name);
                }
            }
        }
        candidates.push(AdapterCandidate {
            name,
            vendor_id: desc.VendorId,
            dedicated_vram: desc.DedicatedVideoMemory,
            outputs,
        });
    }
    candidates
}

pub fn choose_adapter(monitor: Option<&str>, pref: GpuPreference) -> AdapterChoice {
    select_adapter(&enumerate_adapters(), monitor, pref)
}

pub fn choose_encode_backends(pref: GpuPreference) -> Vec<EncodeBackend> {
    select_encode_backends(&enumerate_adapters(), pref)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_software_adapters_and_detects_hardware_with_zero_vram() {
        let mut desc = DXGI_ADAPTER_DESC1 {
            SharedSystemMemory: 8 << 30,
            DedicatedVideoMemory: 0,
            ..Default::default()
        };
        assert!(hardware(&desc));
        desc.Flags = DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32;
        assert!(!hardware(&desc));
    }

    #[test]
    fn auto_prefers_dedicated_gpu_and_falls_back_to_integrated() {
        let igpu = AdapterCandidate {
            name: "Intel UHD Graphics".into(),
            vendor_id: VENDOR_INTEL,
            dedicated_vram: 128 << 20,
            outputs: vec![r"\\.\DISPLAY1".into()],
        };
        let dgpu = AdapterCandidate {
            name: "NVIDIA GeForce RTX".into(),
            vendor_id: VENDOR_NVIDIA,
            dedicated_vram: 8 << 30,
            outputs: vec![r"\\.\DISPLAY2".into()],
        };
        let hybrid = vec![igpu.clone(), dgpu.clone()];

        // Hybrid laptop with a dedicated GPU picks dGPU in Auto regardless of display wiring.
        for monitor in [Some(r"\\.\display1"), Some(r"\\.\DISPLAY2"), None] {
            assert_eq!(
                select_adapter(&hybrid, monitor, GpuPreference::Auto),
                AdapterChoice {
                    name: Some("NVIDIA GeForce RTX".into()),
                    low_memory_uma: false,
                }
            );
        }

        // Hybrid laptop where BIOS reserves 1 GiB UMA for APU still prefers the higher-VRAM dGPU.
        let apu_1g = AdapterCandidate {
            name: "AMD Radeon Graphics".into(),
            vendor_id: VENDOR_AMD,
            dedicated_vram: 1 << 30,
            outputs: vec![r"\\.\DISPLAY1".into()],
        };
        let dgpu_headless = AdapterCandidate {
            outputs: vec![],
            ..dgpu.clone()
        };
        assert_eq!(
            select_adapter(
                &[apu_1g, dgpu_headless],
                Some(r"\\.\DISPLAY1"),
                GpuPreference::Auto
            ),
            AdapterChoice {
                name: Some("NVIDIA GeForce RTX".into()),
                low_memory_uma: false,
            }
        );

        // Dual equal-VRAM dGPUs prefer the adapter wired to the target monitor.
        let dual_dgpu = vec![
            AdapterCandidate {
                name: "GPU A".into(),
                vendor_id: VENDOR_NVIDIA,
                dedicated_vram: 8 << 30,
                outputs: vec![r"\\.\DISPLAY1".into()],
            },
            AdapterCandidate {
                name: "GPU B".into(),
                vendor_id: VENDOR_AMD,
                dedicated_vram: 8 << 30,
                outputs: vec![r"\\.\DISPLAY2".into()],
            },
        ];
        assert_eq!(
            select_adapter(&dual_dgpu, Some(r"\\.\DISPLAY1"), GpuPreference::Auto).name,
            Some("GPU A".into())
        );
        assert_eq!(
            select_adapter(&dual_dgpu, Some(r"\\.\DISPLAY2"), GpuPreference::Auto).name,
            Some("GPU B".into())
        );

        // Manual overrides honor PowerSaving / HighPerformance even when display is on the other adapter.
        assert_eq!(
            select_adapter(&hybrid, Some(r"\\.\DISPLAY2"), GpuPreference::PowerSaving),
            AdapterChoice {
                name: Some("Intel UHD Graphics".into()),
                low_memory_uma: true,
            }
        );
        assert_eq!(
            select_adapter(
                &hybrid,
                Some(r"\\.\DISPLAY1"),
                GpuPreference::HighPerformance
            ),
            AdapterChoice {
                name: Some("NVIDIA GeForce RTX".into()),
                low_memory_uma: false,
            }
        );
    }

    #[test]
    fn handles_zero_vram_potato_igpu_and_empty_candidates() {
        let zero_vram_igpu = vec![AdapterCandidate {
            name: "Intel HD Graphics 4000".into(),
            vendor_id: VENDOR_INTEL,
            dedicated_vram: 0,
            outputs: vec![r"\\.\DISPLAY1".into()],
        }];
        assert_eq!(
            select_adapter(&zero_vram_igpu, Some(r"\\.\DISPLAY1"), GpuPreference::Auto),
            AdapterChoice {
                name: Some("Intel HD Graphics 4000".into()),
                low_memory_uma: true,
            }
        );
        let at_limit = vec![AdapterCandidate {
            name: "AMD Radeon Vega".into(),
            vendor_id: VENDOR_AMD,
            dedicated_vram: LOW_MEMORY_UMA_LIMIT,
            outputs: vec![],
        }];
        assert!(select_adapter(&at_limit, None, GpuPreference::Auto).low_memory_uma);
        assert_eq!(
            select_adapter(&[], None, GpuPreference::Auto),
            AdapterChoice {
                name: None,
                low_memory_uma: true,
            }
        );
    }

    #[test]
    fn encode_backend_order_prefers_dgpu_then_igpu_then_cpu() {
        let intel_igpu = AdapterCandidate {
            name: "Intel UHD Graphics".into(),
            vendor_id: VENDOR_INTEL,
            dedicated_vram: 128 << 20,
            outputs: vec![r"\\.\DISPLAY1".into()],
        };
        let nvidia_dgpu = AdapterCandidate {
            name: "NVIDIA GeForce RTX".into(),
            vendor_id: VENDOR_NVIDIA,
            dedicated_vram: 8 << 30,
            outputs: vec![r"\\.\DISPLAY2".into()],
        };
        let amd_apu = AdapterCandidate {
            name: "AMD Radeon Graphics".into(),
            vendor_id: VENDOR_AMD,
            dedicated_vram: 1 << 30,
            outputs: vec![r"\\.\DISPLAY1".into()],
        };

        let hybrid = [intel_igpu.clone(), nvidia_dgpu.clone()];
        assert_eq!(
            select_encode_backends(&hybrid, GpuPreference::Auto),
            vec![
                EncodeBackend::Nvenc,
                EncodeBackend::Qsv,
                EncodeBackend::CpuX264
            ]
        );
        assert_eq!(
            select_encode_backends(&hybrid, GpuPreference::HighPerformance),
            vec![
                EncodeBackend::Nvenc,
                EncodeBackend::Qsv,
                EncodeBackend::CpuX264
            ]
        );
        assert_eq!(
            select_encode_backends(&hybrid, GpuPreference::PowerSaving),
            vec![
                EncodeBackend::Qsv,
                EncodeBackend::Nvenc,
                EncodeBackend::CpuX264
            ]
        );

        let amd_hybrid = [amd_apu.clone(), nvidia_dgpu.clone()];
        assert_eq!(
            select_encode_backends(&amd_hybrid, GpuPreference::Auto),
            vec![
                EncodeBackend::Nvenc,
                EncodeBackend::Amf,
                EncodeBackend::CpuX264
            ]
        );
        assert_eq!(
            select_encode_backends(&amd_hybrid, GpuPreference::PowerSaving),
            vec![
                EncodeBackend::Amf,
                EncodeBackend::Nvenc,
                EncodeBackend::CpuX264
            ]
        );

        assert_eq!(
            select_encode_backends(std::slice::from_ref(&intel_igpu), GpuPreference::Auto),
            vec![EncodeBackend::Qsv, EncodeBackend::CpuX264]
        );
        assert_eq!(
            select_encode_backends(std::slice::from_ref(&amd_apu), GpuPreference::Auto),
            vec![EncodeBackend::Amf, EncodeBackend::CpuX264]
        );
        assert_eq!(
            select_encode_backends(&[nvidia_dgpu.clone(), nvidia_dgpu], GpuPreference::Auto),
            vec![EncodeBackend::Nvenc, EncodeBackend::CpuX264]
        );
        assert_eq!(
            select_encode_backends(
                &[AdapterCandidate {
                    name: "Unknown Adapter".into(),
                    vendor_id: 0x1234,
                    dedicated_vram: 2 << 30,
                    outputs: vec![],
                }],
                GpuPreference::Auto
            ),
            vec![EncodeBackend::CpuX264]
        );
        assert_eq!(
            select_encode_backends(&[], GpuPreference::Auto),
            vec![EncodeBackend::CpuX264]
        );
    }

    #[test]
    #[ignore = "Read-only native DXGI enumeration; no window or device is created"]
    fn native_adapter_selection() {
        println!(
            "Auto adapter choice: {:?}",
            choose_adapter(None, GpuPreference::Auto)
        );
    }
}
