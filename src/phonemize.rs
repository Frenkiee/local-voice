use anyhow::{bail, Context, Result};
use std::process::Command;

/// Shared espeak-ng phonemizer used by Piper and Kokoro engines
pub struct Phonemizer {
    espeak_path: &'static str,
}

impl Phonemizer {
    pub fn new() -> Result<Self> {
        let path = find_espeak_ng();
        // Verify it actually works
        Command::new(path)
            .arg("--version")
            .output()
            .with_context(|| {
                "espeak-ng not found. Install it:\n  macOS:   brew install espeak-ng\n  Linux:   apt install espeak-ng\n  Windows: choco install espeak-ng"
            })?;

        Ok(Self { espeak_path: path })
    }

    /// Convert text to IPA phonemes using espeak-ng
    pub fn phonemize(&self, text: &str, voice: &str) -> Result<String> {
        let output = Command::new(self.espeak_path)
            .args(["--ipa=1", "-q", "-v", voice, text])
            .output()
            .with_context(|| "Failed to run espeak-ng")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("espeak-ng failed: {stderr}");
        }

        let phonemes = String::from_utf8(output.stdout)
            .with_context(|| "espeak-ng output is not valid UTF-8")?;

        Ok(phonemes.trim().to_string())
    }
}

fn find_espeak_ng() -> &'static str {
    static PATHS: &[&str] = &[
        "espeak-ng",
        "/opt/homebrew/bin/espeak-ng",
        "/usr/local/bin/espeak-ng",
        "/usr/bin/espeak-ng",
        "/opt/local/bin/espeak-ng",
        "C:\\Program Files\\eSpeak NG\\espeak-ng.exe",
        "C:\\Program Files (x86)\\eSpeak NG\\espeak-ng.exe",
    ];

    for path in PATHS {
        if Command::new(path).arg("--version").output().is_ok() {
            return path;
        }
    }
    "espeak-ng"
}
