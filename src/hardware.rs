use std::process::Command;

/// Detected hardware capabilities for auto-selecting TTS engine
#[derive(Debug)]
pub struct HardwareProfile {
    pub total_ram_mb: u64,
    pub cpu_cores: usize,
    pub os: String,
    pub arch: String,
}

impl HardwareProfile {
    pub fn detect() -> Self {
        Self {
            total_ram_mb: detect_ram_mb(),
            cpu_cores: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }
    }

    /// Pick the best engine given this hardware
    pub fn recommended_engine(&self) -> super::engine::EngineKind {
        use super::engine::EngineKind;

        if self.total_ram_mb >= 8192 {
            // Chatterbox quantized needs ~4GB working set + model
            EngineKind::Chatterbox
        } else if self.total_ram_mb >= 2048 {
            EngineKind::Kokoro
        } else {
            EngineKind::Piper
        }
    }

    /// Pick the best model variant for a given engine
    pub fn recommended_variant(&self, engine: super::engine::EngineKind) -> &'static str {
        use super::engine::EngineKind;

        match engine {
            EngineKind::Kokoro => {
                if self.total_ram_mb >= 16384 {
                    "kokoro-fp32"
                } else if self.total_ram_mb >= 8192 {
                    "kokoro-fp16"
                } else if self.total_ram_mb >= 4096 {
                    "kokoro-q8f16"
                } else {
                    "kokoro-q4f16"
                }
            }
            EngineKind::Chatterbox => {
                if self.total_ram_mb >= 16384 {
                    "chatterbox-full"
                } else {
                    "chatterbox-quantized"
                }
            }
            EngineKind::Piper => "en_US-lessac-medium",
        }
    }

    pub fn display(&self) {
        println!();
        println!("  Hardware Profile:");
        println!("    OS:       {} ({})", self.os, self.arch);
        println!(
            "    RAM:      {:.1} GB",
            self.total_ram_mb as f64 / 1024.0
        );
        println!("    CPU:      {} cores", self.cpu_cores);
        println!();
    }
}

#[cfg(target_os = "macos")]
fn detect_ram_mb() -> u64 {
    Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|bytes| bytes / (1024 * 1024))
        .unwrap_or(4096)
}

#[cfg(target_os = "linux")]
fn detect_ram_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|line| {
                    line.split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse::<u64>().ok())
                })
        })
        .map(|kb| kb / 1024)
        .unwrap_or(4096)
}

#[cfg(target_os = "windows")]
fn detect_ram_mb() -> u64 {
    Command::new("wmic")
        .args(["OS", "get", "TotalVisibleMemorySize", "/value"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("TotalVisibleMemorySize="))
                .and_then(|l| l.split('=').nth(1))
                .and_then(|v| v.trim().parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(4096)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn detect_ram_mb() -> u64 {
    4096
}
