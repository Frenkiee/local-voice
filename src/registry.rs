/// Model catalog — curated list of Piper TTS voices
///
/// Models are downloaded from HuggingFace: rhasspy/piper-voices

const HF_BASE: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0";

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ModelEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub language: &'static str,
    pub quality: &'static str,
    pub description: &'static str,
    pub size_mb: u32,
    pub sample_rate: u32,
}

impl ModelEntry {
    pub fn onnx_url(&self) -> String {
        let (lang, rest) = self.id.split_once('_').unwrap_or(("en", self.id));
        let (country_name, quality) = rest.rsplit_once('-').unwrap_or((rest, "medium"));
        let (country, name) = country_name.split_once('-').unwrap_or((country_name, "unknown"));
        let lang_country = format!("{lang}_{country}");
        format!("{HF_BASE}/{lang}/{lang_country}/{name}/{quality}/{lang_country}-{name}-{quality}.onnx")
    }

    pub fn config_url(&self) -> String {
        format!("{}.json", self.onnx_url())
    }
}

pub static MODELS: &[ModelEntry] = &[
    // English — US
    ModelEntry {
        id: "en_US-lessac-medium",
        name: "Lessac",
        language: "en-US",
        quality: "medium",
        description: "High-quality US English, balanced speed/quality",
        size_mb: 65,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-lessac-high",
        name: "Lessac HQ",
        language: "en-US",
        quality: "high",
        description: "Highest quality US English voice",
        size_mb: 100,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-amy-medium",
        name: "Amy",
        language: "en-US",
        quality: "medium",
        description: "US English female voice",
        size_mb: 65,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-ryan-medium",
        name: "Ryan",
        language: "en-US",
        quality: "medium",
        description: "US English male voice",
        size_mb: 65,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-arctic-medium",
        name: "Arctic",
        language: "en-US",
        quality: "medium",
        description: "US English multi-speaker dataset voice",
        size_mb: 65,
        sample_rate: 22050,
    },
    // English — GB
    ModelEntry {
        id: "en_GB-alan-medium",
        name: "Alan",
        language: "en-GB",
        quality: "medium",
        description: "British English male voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_GB-cori-medium",
        name: "Cori",
        language: "en-GB",
        quality: "medium",
        description: "British English female voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // German
    ModelEntry {
        id: "de_DE-thorsten-medium",
        name: "Thorsten",
        language: "de-DE",
        quality: "medium",
        description: "German male voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "de_DE-thorsten-high",
        name: "Thorsten HQ",
        language: "de-DE",
        quality: "high",
        description: "High-quality German male voice",
        size_mb: 90,
        sample_rate: 22050,
    },
    // French
    ModelEntry {
        id: "fr_FR-upmc-medium",
        name: "UPMC",
        language: "fr-FR",
        quality: "medium",
        description: "French voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Spanish
    ModelEntry {
        id: "es_ES-davefx-medium",
        name: "DaveFX",
        language: "es-ES",
        quality: "medium",
        description: "Spanish male voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Italian
    ModelEntry {
        id: "it_IT-riccardo-x_low",
        name: "Riccardo",
        language: "it-IT",
        quality: "x_low",
        description: "Italian male voice (compact)",
        size_mb: 18,
        sample_rate: 16000,
    },
    // Portuguese
    ModelEntry {
        id: "pt_BR-faber-medium",
        name: "Faber",
        language: "pt-BR",
        quality: "medium",
        description: "Brazilian Portuguese male voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Dutch
    ModelEntry {
        id: "nl_NL-mls-medium",
        name: "MLS",
        language: "nl-NL",
        quality: "medium",
        description: "Dutch voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Russian
    ModelEntry {
        id: "ru_RU-denis-medium",
        name: "Denis",
        language: "ru-RU",
        quality: "medium",
        description: "Russian male voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Chinese
    ModelEntry {
        id: "zh_CN-huayan-medium",
        name: "Huayan",
        language: "zh-CN",
        quality: "medium",
        description: "Mandarin Chinese female voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Ukrainian
    ModelEntry {
        id: "uk_UA-lada-x_low",
        name: "Lada",
        language: "uk-UA",
        quality: "x_low",
        description: "Ukrainian female voice (compact)",
        size_mb: 18,
        sample_rate: 16000,
    },
    // Norwegian
    ModelEntry {
        id: "no_NO-talesyntese-medium",
        name: "Talesyntese",
        language: "no-NO",
        quality: "medium",
        description: "Norwegian voice",
        size_mb: 55,
        sample_rate: 22050,
    },
];

pub fn find_model(id: &str) -> Option<&'static ModelEntry> {
    MODELS.iter().find(|m| m.id == id)
}

pub fn search_models(language: Option<&str>) -> Vec<&'static ModelEntry> {
    MODELS
        .iter()
        .filter(|m| {
            language
                .map(|l| {
                    m.language.to_lowercase().starts_with(&l.to_lowercase())
                        || m.id.to_lowercase().starts_with(&l.to_lowercase())
                })
                .unwrap_or(true)
        })
        .collect()
}
