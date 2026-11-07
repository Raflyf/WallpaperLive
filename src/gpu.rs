use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};

fn dedicated(desc: &DXGI_ADAPTER_DESC1) -> bool {
    desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 && desc.DedicatedVideoMemory > 0
}

pub fn preferred_adapter() -> Option<String> {
    // SAFETY: read-only DXGI enumeration; COM references released by the generated bindings.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1().ok()? };
    let mut best: Option<(usize, String)> = None;
    // ponytail: dedicated memory is the portable discrete-GPU heuristic; add explicit selection only if needed.
    for index in 0..64 {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if !dedicated(&desc) {
            continue;
        }
        let end = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        let Ok(name) = String::from_utf16(&desc.Description[..end]) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(bytes, _)| desc.DedicatedVideoMemory > *bytes)
        {
            best = Some((desc.DedicatedVideoMemory, name));
        }
    }
    best.map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_software_and_shared_memory_only_adapters() {
        let mut desc = DXGI_ADAPTER_DESC1 {
            SharedSystemMemory: 8 << 30,
            ..Default::default()
        };
        assert!(!dedicated(&desc));
        desc.DedicatedVideoMemory = 4 << 30;
        assert!(dedicated(&desc));
        desc.Flags = DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32;
        assert!(!dedicated(&desc));
    }
    #[test]
    #[ignore = "Read-only native DXGI enumeration; no window or device is created"]
    fn native_dedicated_adapter() {
        println!("Preferred dedicated adapter: {:?}", preferred_adapter());
    }
}
