use crate::hash::{create_id, now_millis};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRun {
    pub run_id: String,
    pub provider: String,
    pub model: String,
    pub started_at_ms: u128,
    pub completed_at_ms: u128,
    pub content: String,
    pub incomplete: bool,
}

pub trait ModelAdapter {
    fn stream_response(&mut self, prompt: &str, on_token: &mut dyn FnMut(&str)) -> ModelRun;
    fn estimate_tokens(&self, payload: &str) -> usize {
        payload.len().div_ceil(4)
    }
    fn cancel(&mut self, run_id: &str);
}

#[derive(Debug, Clone)]
pub struct MockModelAdapter {
    response: String,
    cancelled: Vec<String>,
}

impl MockModelAdapter {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            cancelled: Vec::new(),
        }
    }
}

impl ModelAdapter for MockModelAdapter {
    fn stream_response(&mut self, _prompt: &str, on_token: &mut dyn FnMut(&str)) -> ModelRun {
        let run_id = create_id("modelrun");
        let started_at_ms = now_millis();
        let mut content = String::new();
        for chunk in self.response.as_bytes().chunks(24) {
            if self.cancelled.contains(&run_id) {
                break;
            }
            let token = String::from_utf8_lossy(chunk);
            content.push_str(&token);
            on_token(&token);
        }
        ModelRun {
            run_id: run_id.clone(),
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            started_at_ms,
            completed_at_ms: now_millis(),
            content,
            incomplete: self.cancelled.contains(&run_id),
        }
    }

    fn cancel(&mut self, run_id: &str) {
        self.cancelled.push(run_id.to_string());
    }
}
