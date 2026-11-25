use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use crate::config::{GraphicsPreset, GraphicsSettings, JvmGcMode};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub cpu_name: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub total_ram_mb: u64,
    pub available_ram_mb: u64,
    pub gpu_name: String,
    pub gpu_backend: String,
    pub gpu_device_type: GpuTier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuTier {
    Discrete,
    Integrated,
    VirtualOrSoftware,
}

impl GpuTier {
    pub fn label(self) -> &'static str {
        match self {
            Self::Discrete => "Discrete GPU",
            Self::Integrated => "Integrated GPU",
            Self::VirtualOrSoftware => "Software / Virtual GPU",
        }
    }
}

impl HardwareProfile {
    pub fn detect(adapter_info: &wgpu::AdapterInfo) -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        sys.refresh_cpu_all();
        sys.refresh_memory();

        let cpu_name = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Unknown CPU".to_string());

        let logical_cores = sys.cpus().len().max(1);
        let physical_cores = System::physical_core_count().unwrap_or(logical_cores).max(1);
        let total_ram_mb = sys.total_memory() / (1024 * 1024);
        let available_ram_mb = sys.available_memory() / (1024 * 1024);

        let gpu_device_type = match adapter_info.device_type {
            wgpu::DeviceType::DiscreteGpu => GpuTier::Discrete,
            wgpu::DeviceType::IntegratedGpu => GpuTier::Integrated,
            _ => GpuTier::VirtualOrSoftware,
        };

        let gpu_backend = format!("{:?}", adapter_info.backend);

        Self {
            cpu_name,
            physical_cores,
            logical_cores,
            total_ram_mb,
            available_ram_mb,
            gpu_name: adapter_info.name.clone(),
            gpu_backend,
            gpu_device_type,
        }
    }

    fn query_adapter_info_safe() -> wgpu::AdapterInfo {
        use std::sync::mpsc;
        use std::time::Duration;

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let probed = std::panic::catch_unwind(|| {
                let probe_backend = |backends: wgpu::Backends| -> Option<wgpu::AdapterInfo> {
                    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
                    desc.backends = backends;
                    let instance = wgpu::Instance::new(desc);
                    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::HighPerformance,
                        compatible_surface: None,
                        force_fallback_adapter: false,
                    }))
                    .ok()
                    .map(|adapter| adapter.get_info())
                };
                if cfg!(target_os = "windows") {
                    // Probe DX12 first (avoids NVOpenGLPbuffer/Vulkan ICD startup cost on modern GPUs),
                    // then fall back to Vulkan for VMs, RDP, or older GPUs without DX12 hardware drivers.
                    probe_backend(wgpu::Backends::DX12)
                        .or_else(|| probe_backend(wgpu::Backends::VULKAN))
                } else {
                    probe_backend(wgpu::Backends::PRIMARY)
                }
            });
            let _ = tx.send(probed.ok().flatten());
        });

        if let Ok(Some(info)) = rx.recv_timeout(Duration::from_millis(3500)) {
            return info;
        }

        wgpu::AdapterInfo {
            name: "Unknown Graphics Adapter".to_string(),
            vendor: 0,
            device: 0,
            device_type: wgpu::DeviceType::Other,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Noop,
            subgroup_min_size: 4,
            subgroup_max_size: 128,
            transient_saves_memory: false,
        }
    }

    pub fn detect_headless() -> Self {
        let info = Self::query_adapter_info_safe();
        Self::detect(&info)
    }

    fn safe_jvm_ram_mb(&self, target_mb: u32) -> u32 {
        let half_total = ((self.total_ram_mb / 2) as u32).max(1024);
        target_mb.min(half_total).clamp(1024, 8192)
    }

    /// Computes balanced Minecraft 1.21.4 + JVM parameters based on actual detected hardware.
    pub fn recommend_settings(&self, current: &GraphicsSettings) -> (GraphicsSettings, String) {
        let (preset, render_distance, simulation_distance, smooth_lighting, fancy_graphics, target_ram, jvm_gc, reason) =
            match self.gpu_device_type {
                GpuTier::Discrete if self.total_ram_mb >= 12_000 && self.logical_cores >= 8 => {
                    let ram = self.safe_jvm_ram_mb(4096);
                    (
                        GraphicsPreset::Ultra,
                        16,
                        12,
                        true,
                        true,
                        ram,
                        JvmGcMode::ZGC,
                        format!(
                            "{} (Discrete GPU), {} logical cores, {:.1} GB RAM detected: selected 16 chunks render distance, {} MB JVM memory, and ZGC.",
                            self.gpu_name,
                            self.logical_cores,
                            self.total_ram_mb as f64 / 1024.0,
                            ram
                        ),
                    )
                }
                GpuTier::Discrete => {
                    let ram = self.safe_jvm_ram_mb(3072);
                    (
                        GraphicsPreset::High,
                        12,
                        10,
                        true,
                        true,
                        ram,
                        JvmGcMode::G1GC,
                        format!(
                            "{} (Discrete GPU) detected: selected 12 chunks render distance, {} MB JVM memory, and G1GC.",
                            self.gpu_name, ram
                        ),
                    )
                }
                GpuTier::Integrated if self.total_ram_mb >= 8_000 && self.logical_cores >= 4 => {
                    let ram = self.safe_jvm_ram_mb(2560);
                    (
                        GraphicsPreset::Medium,
                        8,
                        8,
                        true,
                        true,
                        ram,
                        JvmGcMode::G1GC,
                        format!(
                            "{} (Integrated GPU) detected: selected balanced 8 chunks render distance, {} MB JVM memory, and G1GC.",
                            self.gpu_name, ram
                        ),
                    )
                }
                _ => {
                    let ram = self.safe_jvm_ram_mb(1536);
                    (
                        GraphicsPreset::Low,
                        6,
                        6,
                        false,
                        false,
                        ram,
                        JvmGcMode::G1GC,
                        format!(
                            "Limited GPU/memory resources ({}, {:.1} GB RAM): selected 6 chunks, Fast graphics, and {} MB JVM for steady frame pacing.",
                            self.gpu_name,
                            self.total_ram_mb as f64 / 1024.0,
                            ram
                        ),
                    )
                }
            };

        let recommended = GraphicsSettings {
            preset,
            render_distance,
            simulation_distance,
            fov_degrees: current.fov_degrees,
            vsync: false,
            max_fps: 260,
            mouse_sensitivity: current.mouse_sensitivity,
            smooth_lighting,
            fancy_graphics,
            jvm_ram_mb: target_ram,
            jvm_gc,
            show_fps_hud: current.show_fps_hud,
            auto_configured: true,
        };

        (recommended, reason)
    }

    pub fn apply_preset(&self, preset: GraphicsPreset, current: &GraphicsSettings) -> GraphicsSettings {
        match preset {
            GraphicsPreset::Low => GraphicsSettings {
                preset,
                render_distance: 6,
                simulation_distance: 6,
                smooth_lighting: false,
                fancy_graphics: false,
                jvm_ram_mb: self.safe_jvm_ram_mb(1536),
                jvm_gc: JvmGcMode::G1GC,
                ..current.clone()
            },
            GraphicsPreset::Medium => GraphicsSettings {
                preset,
                render_distance: 8,
                simulation_distance: 8,
                smooth_lighting: true,
                fancy_graphics: true,
                jvm_ram_mb: self.safe_jvm_ram_mb(2560),
                jvm_gc: JvmGcMode::G1GC,
                ..current.clone()
            },
            GraphicsPreset::High => GraphicsSettings {
                preset,
                render_distance: 12,
                simulation_distance: 10,
                smooth_lighting: true,
                fancy_graphics: true,
                jvm_ram_mb: self.safe_jvm_ram_mb(3072),
                jvm_gc: JvmGcMode::G1GC,
                ..current.clone()
            },
            GraphicsPreset::Ultra => GraphicsSettings {
                preset,
                render_distance: 16,
                simulation_distance: 12,
                smooth_lighting: true,
                fancy_graphics: true,
                jvm_ram_mb: self.safe_jvm_ram_mb(4096),
                jvm_gc: JvmGcMode::ZGC,
                ..current.clone()
            },
            GraphicsPreset::Custom => current.clone(),
        }
    }
}
