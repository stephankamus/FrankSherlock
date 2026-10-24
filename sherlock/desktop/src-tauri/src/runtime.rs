use crate::models::RuntimeStatus;
use crate::platform::gpu::GpuInfo;

pub fn gather_runtime_status(gpu: &GpuInfo) -> RuntimeStatus {
    let (ollama_available, loaded_models) = crate::llm::list_loaded_models();

    // `gpu` is the startup snapshot used for model selection; refresh usage so a
    // model loaded during a scan is reflected in the status bar.
    let vram_used_mib =
        crate::platform::gpu::current_vram_used_mib(gpu.vendor).or(gpu.vram_used_mib);

    RuntimeStatus {
        os: crate::platform::current_os(),
        current_model: loaded_models.first().cloned(),
        loaded_models,
        vram_used_mib,
        vram_total_mib: gpu.vram_total_mib,
        gpu_vendor: gpu.vendor,
        unified_memory: gpu.unified_memory,
        system_ram_mib: gpu.system_ram_mib,
        ollama_available,
    }
}
