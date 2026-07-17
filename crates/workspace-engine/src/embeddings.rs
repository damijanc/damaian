use crate::error::{ClientError, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig, DTYPE};
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use tokenizers::{PaddingParams, Tokenizer};

/// A small (~90MB), widely-used sentence-embedding model. Chosen over a
/// larger model for fast local inference on a developer laptop; 384
/// dimensions is enough for repository-scale semantic search.
const MODEL_REPO: &str = "sentence-transformers/all-MiniLM-L6-v2";
const MODEL_FILES: &[&str] = &["config.json", "tokenizer.json", "model.safetensors"];

/// Wraps a local BERT-family embedding model. Loading downloads the model
/// once into `<data_dir>/models/all-MiniLM-L6-v2` (skipped on subsequent
/// loads); every embedding computed afterward runs fully offline.
pub struct EmbeddingModel {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl EmbeddingModel {
    pub fn load(data_dir: &Path) -> Result<Self> {
        let model_dir = data_dir.join("models").join("all-MiniLM-L6-v2");
        std::fs::create_dir_all(&model_dir)?;
        for file in MODEL_FILES {
            download_if_missing(&model_dir.join(file), file)?;
        }

        let config_json = std::fs::read_to_string(model_dir.join("config.json"))?;
        let config: BertConfig = serde_json::from_str(&config_json).map_err(|error| {
            ClientError::Io(format!("Invalid embedding model config.json: {error}"))
        })?;

        let mut tokenizer =
            Tokenizer::from_file(model_dir.join("tokenizer.json")).map_err(|error| {
                ClientError::Io(format!("Failed to load embedding tokenizer: {error}"))
            })?;
        tokenizer.with_padding(Some(PaddingParams::default()));

        let device = Device::Cpu;
        let weights_path = model_dir.join("model.safetensors");
        // Safe here: `weights_path` is a file this process just downloaded
        // (or previously downloaded) from a fixed, known model repository,
        // not attacker-controlled input.
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[&weights_path], DTYPE, &device)
                .map_err(|error| ClientError::Io(format!("Failed to load model weights: {error}")))?
        };
        let model = BertModel::load(vb, &config)
            .map_err(|error| ClientError::Io(format!("Failed to build embedding model: {error}")))?;

        Ok(Self {
            model,
            tokenizer,
            device,
        })
    }

    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.embed_batch(&[text])?.remove(0))
    }

    /// Mean-pools token embeddings (masked so padding tokens are excluded)
    /// and L2-normalizes each row, matching how `sentence-transformers`
    /// models are meant to be used for cosine-similarity search.
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|error| ClientError::Io(format!("Failed to tokenize text: {error}")))?;

        let candle_result: candle_core::Result<Vec<f32>> = (|| {
            let input_ids = encodings
                .iter()
                .map(|encoding| Tensor::new(encoding.get_ids(), &self.device))
                .collect::<candle_core::Result<Vec<_>>>()?;
            let attention_mask = encodings
                .iter()
                .map(|encoding| Tensor::new(encoding.get_attention_mask(), &self.device))
                .collect::<candle_core::Result<Vec<_>>>()?;
            let input_ids = Tensor::stack(&input_ids, 0)?;
            let attention_mask = Tensor::stack(&attention_mask, 0)?;
            let token_type_ids = input_ids.zeros_like()?;

            let hidden_states =
                self.model
                    .forward(&input_ids, &token_type_ids, Some(&attention_mask))?;
            let mask = attention_mask.to_dtype(DType::F32)?.unsqueeze(2)?;
            let summed = hidden_states.broadcast_mul(&mask)?.sum(1)?;
            let counts = attention_mask.to_dtype(DType::F32)?.sum(1)?.unsqueeze(1)?;
            let mean = summed.broadcast_div(&counts)?;
            let norm = mean.sqr()?.sum_keepdim(1)?.sqrt()?;
            let normalized = mean.broadcast_div(&norm)?;
            Ok(normalized.to_vec2::<f32>()?.into_iter().flatten().collect())
        })();
        let flat = candle_result
            .map_err(|error| ClientError::Io(format!("Embedding inference failed: {error}")))?;

        let dim = flat.len() / texts.len();
        Ok(flat.chunks(dim).map(|chunk| chunk.to_vec()).collect())
    }
}

fn download_if_missing(path: &Path, filename: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let url = format!("https://huggingface.co/{MODEL_REPO}/resolve/main/{filename}");
    let temp_path = path.with_extension("part");
    let status = Command::new("curl")
        .args(["-sS", "-L", "-f", "-o"])
        .arg(&temp_path)
        .arg(&url)
        .status()
        .map_err(|error| ClientError::Io(format!("Failed to invoke curl: {error}")))?;
    if !status.success() {
        let _ = std::fs::remove_file(&temp_path);
        return Err(ClientError::Io(format!(
            "Failed to download {filename} for the local embedding model (one-time setup; requires network access)"
        )));
    }
    std::fs::rename(&temp_path, path)?;
    Ok(())
}

/// Process-wide, lazily-loaded model shared by every semantic search call.
/// A load failure (no network on first run, disk full, etc.) is cached so
/// callers degrade to keyword/term-overlap search once per process instead
/// of retrying the expensive load (and re-failing) on every request.
static SHARED_MODEL: OnceLock<Option<EmbeddingModel>> = OnceLock::new();

pub fn shared_model(data_dir: &Path) -> Option<&'static EmbeddingModel> {
    let data_dir = data_dir.to_path_buf();
    SHARED_MODEL
        .get_or_init(move || match EmbeddingModel::load(&data_dir) {
            Ok(model) => Some(model),
            Err(error) => {
                eprintln!("Local semantic search disabled: {error}");
                None
            }
        })
        .as_ref()
}
