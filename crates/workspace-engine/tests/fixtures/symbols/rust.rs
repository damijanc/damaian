//! Accuracy fixture for the heuristic Rust extractor. Read as text, never compiled.

use std::fmt;

pub struct Upload {
    pub path: String,
}

pub enum UploadState {
    Pending,
    Done,
}

fn upload_retry(upload: &Upload) -> bool {
    upload.path.is_empty()
}

impl Upload {
    pub fn new(path: &str) -> Self {
        Self { path: path.to_string() }
    }

    fn validate(&self) -> bool {
        true
    }
}

impl fmt::Display for Upload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.path)
    }
}

impl<T> From<T> for UploadState
where
    T: Into<String>,
{
    fn from(_value: T) -> Self {
        UploadState::Pending
    }
}

pub(crate) fn crate_visible() {}

pub(super) async fn fetch_remote() {}

pub const fn limit() -> usize {
    3
}

pub trait Store {
    fn put(&self, key: &str);
}

impl Store for Upload {
    fn put(&self, _key: &str) {}
}

pub type UploadMap = std::collections::HashMap<String, Upload>;

pub const MAX_RETRIES: u32 = 5;

static GREETING: &str = "fn not_a_function() {}";

unsafe extern "C" fn exported_callback() {}

mod helpers {
    pub fn helper() {}
}

// fn line_commented() {}

/*
fn block_commented() {}
struct BlockCommentedStruct;
*/

const TEMPLATE: &str = r#"one " quote
fn inside_raw_string() {}
"#;

const PLAIN: &str = "
fn inside_plain_string() {}
";

fn after_strings(input: char) -> bool {
    input == '{' || input == '"'
}

fn last_one() {}

macro_rules! make_getter {
    ($name:ident) => {
        fn $name() {}
    };
}
