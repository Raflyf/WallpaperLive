use crate::core::GpuPreference;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};

pub const LOW_MEMORY_UMA_LIMIT: usize = 512 << 20;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterChoice {
    pub name: Option<String>,
    pub low_memory_uma: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterCandidate {
    pub name: String,
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
            .find(|c| matches_monitor(c, monitor))
            .or_else(|| candidates.iter().min_by_key(|c| c.dedicated_vram)),
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

pub fn choose_adapter(monitor: Option<&str>, pref: GpuPreference) -> AdapterChoice {
    // SAFETY: read-only DXGI adapter/output enumeration; COM references released by windows crate.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return AdapterChoice {
            name: None,
            low_memory_uma: true,
        };
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
            dedicated_vram: desc.DedicatedVideoMemory,
            outputs,
        });
    }
    select_adapter(&candidates, monitor, pref)
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
    fn auto_routes_hybrid_laptop_and_desktop_monitors_to_output_adapter() {
        let igpu = AdapterCandidate {
            name: "Intel UHD Graphics".into(),
            dedicated_vram: 128 << 20,
            outputs: vec![r"\\.\DISPLAY1".into()],
        };
        let dgpu = AdapterCandidate {
            name: "NVIDIA GeForce RTX".into(),
            dedicated_vram: 8 << 30,
            outputs: vec![r"\\.\DISPLAY2".into()],
        };
        let hybrid = vec![igpu.clone(), dgpu.clone()];

        // Internal laptop panel attached to iGPU uses iGPU (no PCIe copy, dGPU sleeps).
        assert_eq!(
            select_adapter(&hybrid, Some(r"\\.\display1"), GpuPreference::Auto),
            AdapterChoice {
                name: Some("Intel UHD Graphics".into()),
                low_memory_uma: true,
            }
        );

        // External display wired directly to dGPU uses dGPU with full pools.
        assert_eq!(
            select_adapter(&hybrid, Some(r"\\.\DISPLAY2"), GpuPreference::Auto),
            AdapterChoice {
                name: Some("NVIDIA GeForce RTX".into()),
                low_memory_uma: false,
            }
        );

        // Unmatched monitor in Auto mode avoids waking sleeping dGPU.
        assert_eq!(
            select_adapter(&hybrid, None, GpuPreference::Auto),
            AdapterChoice {
                name: Some("Intel UHD Graphics".into()),
                low_memory_uma: true,
            }
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
    #[ignore = "Read-only native DXGI enumeration; no window or device is created"]
    fn native_adapter_selection() {
        println!(
            "Auto adapter choice: {:?}",
            choose_adapter(None, GpuPreference::Auto)
        );
    }
}
