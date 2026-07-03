pub fn create_unified_diff(old_content: &str, new_content: &str, file_path: &str) -> String {
    if old_content == new_content {
        return String::new();
    }
    let old_lines: Vec<&str> = if old_content.is_empty() {
        Vec::new()
    } else {
        old_content.split('\n').collect()
    };
    let new_lines: Vec<&str> = if new_content.is_empty() {
        Vec::new()
    } else {
        new_content.split('\n').collect()
    };

    let mut diff = format!(
        "--- a/{file_path}\n+++ b/{file_path}\n@@ -1,{} +1,{} @@\n",
        old_lines.len(),
        new_lines.len()
    );
    for line in old_lines {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new_lines {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}
