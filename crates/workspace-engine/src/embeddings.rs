use crate::error::{ClientError, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig, DTYPE};
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use tokenizers::{PaddingParams, Tokenizer};

/// A small (~90MB), widely-used sentence-embedding model. Chosen over a
/// larger model for fast local inference on a developer laptop; 384
/// dimensions is enough for repository-scale semantic search.
const MODEL_REPO: &str = "sentence-transformers/all-MiniLM-L6-v2";

/// The model repository's commit the files below come from. Downloading from
/// `main` would let a later change to that repository reach users unannounced.
const MODEL_REVISION: &str = "1110a243fdf4706b3f48f1d95db1a4f5529b4d41";

/// A GitHub release of Damaian's own repository holding the same three files,
/// tried when huggingface.co cannot be reached. The tag names the revision, so
/// a new revision needs a new release rather than replacing these assets.
const MIRROR_BASE: &str = "https://github.com/damijanc/damaian/releases/download/embedding-model-all-MiniLM-L6-v2-1110a24";

/// One file of the model and the SHA-256 it must have. A file is used only
/// when it matches, whichever source it came from and whenever it was saved.
struct ModelFile {
    name: &'static str,
    sha256: &'static str,
}

const MODEL_FILES: &[ModelFile] = &[
    ModelFile {
        name: "config.json",
        sha256: "953f9c0d463486b10a6871cc2fd59f223b2c70184f49815e7efbcab5d8908b41",
    },
    ModelFile {
        name: "tokenizer.json",
        sha256: "be50c3628f2bf5bb5e3a7f17b1f74611b2561a3a27eeab05e5aa30f411572037",
    },
    ModelFile {
        name: "model.safetensors",
        sha256: "53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db",
    },
];

/// Where `name` can be downloaded from, in the order they are tried.
fn model_file_sources(name: &str) -> Vec<String> {
    vec![
        format!("https://huggingface.co/{MODEL_REPO}/resolve/{MODEL_REVISION}/{name}"),
        format!("{MIRROR_BASE}/{name}"),
    ]
}

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
            ensure_verified(
                &model_dir.join(file.name),
                file.sha256,
                &model_file_sources(file.name),
            )?;
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
            VarBuilder::from_mmaped_safetensors(&[&weights_path], DTYPE, &device).map_err(
                |error| ClientError::Io(format!("Failed to load model weights: {error}")),
            )?
        };
        let model = BertModel::load(vb, &config).map_err(|error| {
            ClientError::Io(format!("Failed to build embedding model: {error}"))
        })?;

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

/// Makes `path` hold the file whose SHA-256 is `expected`, downloading it from
/// the first of `sources` that serves it.
///
/// A file already on disk is hashed too: one saved before downloads were
/// pinned, or damaged since, is replaced rather than loaded. A download lands
/// in a `.part` file of its own and is renamed into place only once it matches, so
/// neither a truncated transfer nor different content is ever loaded.
fn ensure_verified(path: &Path, expected: &str, sources: &[String]) -> Result<()> {
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if path.exists() {
        if file_sha256(path)? == expected {
            return Ok(());
        }
        eprintln!(
            "Embedding model file {filename} does not match its pinned hash; downloading it again"
        );
        std::fs::remove_file(path)?;
    }
    // One per download: the desktop shell and the CLI, or two loaders in one
    // process, can fetch the same file at once, and a shared temporary file
    // would mix their transfers.
    static DOWNLOADS: AtomicU64 = AtomicU64::new(0);
    let temp_path = path.with_extension(format!(
        "{}-{}.part",
        std::process::id(),
        DOWNLOADS.fetch_add(1, Ordering::Relaxed)
    ));
    let mut failures = Vec::new();
    for url in sources {
        let _ = std::fs::remove_file(&temp_path);
        let status = Command::new("curl")
            .args(["-sS", "-L", "-f", "-o"])
            .arg(&temp_path)
            .arg(url)
            .status()
            .map_err(|error| ClientError::Io(format!("Failed to invoke curl: {error}")))?;
        if !status.success() {
            failures.push(format!("{url}: download failed"));
            continue;
        }
        let actual = file_sha256(&temp_path)?;
        if actual != expected {
            failures.push(format!("{url}: SHA-256 {actual}, expected {expected}"));
            continue;
        }
        std::fs::rename(&temp_path, path)?;
        return Ok(());
    }
    let _ = std::fs::remove_file(&temp_path);
    Err(ClientError::Io(format!(
        "Failed to download {filename} for the local embedding model (one-time setup; requires \
         network access): {}",
        failures.join("; ")
    )))
}

fn file_sha256(path: &Path) -> Result<String> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    let mut file = std::fs::File::open(path)?;
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "damaian-model-download-{name}-{}-{}",
            std::process::id(),
            crate::hash::now_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A local file served the way curl reads any URL, so no test touches the
    /// network.
    fn served(dir: &Path, name: &str, content: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        format!("file://{}", path.display())
    }

    fn no_part_files_in(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .unwrap()
            .all(|entry| !entry.unwrap().path().to_string_lossy().ends_with(".part"))
    }

    fn sha256_of(content: &str) -> String {
        crate::hash::sha256_hex(content.as_bytes())
    }

    #[test]
    fn a_download_that_matches_its_hash_is_kept() {
        let dir = scratch("match");
        let source = served(&dir, "source.json", "the model");
        let target = dir.join("model.json");

        ensure_verified(&target, &sha256_of("the model"), &[source]).unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "the model");
        assert!(no_part_files_in(&dir), "a temporary file was left behind");
    }

    #[test]
    fn a_source_serving_other_content_is_refused_and_the_next_one_tried() {
        let dir = scratch("fallback");
        let tampered = served(&dir, "tampered.json", "not the model");
        let mirror = served(&dir, "mirror.json", "the model");
        let target = dir.join("model.json");

        ensure_verified(&target, &sha256_of("the model"), &[tampered, mirror]).unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "the model");
    }

    #[test]
    fn an_unreachable_source_falls_back_to_the_next_one() {
        let dir = scratch("unreachable");
        let missing = format!("file://{}", dir.join("absent.json").display());
        let mirror = served(&dir, "mirror.json", "the model");
        let target = dir.join("model.json");

        ensure_verified(&target, &sha256_of("the model"), &[missing, mirror]).unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "the model");
    }

    #[test]
    fn when_no_source_matches_nothing_is_left_to_load() {
        let dir = scratch("no-match");
        let tampered = served(&dir, "tampered.json", "not the model");
        let target = dir.join("model.json");

        let error = ensure_verified(&target, &sha256_of("the model"), &[tampered]).unwrap_err();

        assert!(error.to_string().contains("expected"), "{error}");
        assert!(!target.exists());
        assert!(no_part_files_in(&dir), "a temporary file was left behind");
    }

    #[test]
    fn a_saved_file_that_no_longer_matches_is_downloaded_again() {
        let dir = scratch("stale");
        let target = dir.join("model.json");
        std::fs::write(&target, "an older model").unwrap();
        let source = served(&dir, "source.json", "the model");

        ensure_verified(&target, &sha256_of("the model"), &[source]).unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "the model");
    }

    #[test]
    fn a_saved_file_that_matches_is_used_without_a_download() {
        let dir = scratch("cached");
        let target = dir.join("model.json");
        std::fs::write(&target, "the model").unwrap();

        ensure_verified(&target, &sha256_of("the model"), &[]).unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "the model");
    }

    #[test]
    fn every_model_file_is_pinned_to_a_revision_with_a_mirror() {
        for file in MODEL_FILES {
            assert_eq!(file.sha256.len(), 64, "{}", file.name);
            let sources = model_file_sources(file.name);
            assert!(
                sources[0].contains(&format!("/resolve/{MODEL_REVISION}/")),
                "{sources:?}"
            );
            assert!(sources[1].starts_with("https://github.com/"), "{sources:?}");
        }
    }
}
