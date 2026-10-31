//! Supported pinned GigaSTT model layouts and configured language scope.
use crate::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelVariant {
    #[default]
    E2eRnnt,
    MlCtc,
    MlCtcLarge,
}
impl ModelVariant {
    pub const ALL: [Self; 3] = [Self::E2eRnnt, Self::MlCtc, Self::MlCtcLarge];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::E2eRnnt => "e2e_rnnt",
            Self::MlCtc => "ml_ctc",
            Self::MlCtcLarge => "ml_ctc_large",
        }
    }
    pub fn files(self) -> &'static [&'static str] {
        match self {
            Self::E2eRnnt => &super::MODEL_FILES,
            Self::MlCtc => &["multilingual_ctc.int8.onnx", "multilingual_vocab.txt"],
            Self::MlCtcLarge => &["multilingual_large_ctc.int8.onnx", "multilingual_vocab.txt"],
        }
    }
    pub fn recognizer_language(self) -> &'static str {
        match self {
            Self::E2eRnnt => "ru",
            Self::MlCtc | Self::MlCtcLarge => "mul",
        }
    }
}
impl std::str::FromStr for ModelVariant {
    type Err = Box<dyn std::error::Error + Send + Sync>;
    fn from_str(value: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|variant| variant.as_str() == value)
            .ok_or_else(|| "--model-variant must be e2e_rnnt, ml_ctc or ml_ctc_large".into())
    }
}
