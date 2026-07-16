use similar::{ChangeTag, TextDiff};

const CONTEXT_RADIUS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub tag: String,
    pub text: String,
}

/// A contiguous group of changed lines (plus a little surrounding context),
/// with a stable id so the UI can accept or reject it independently of the
/// rest of the file. `old_start`/`new_start` are 0-based line indices into
/// the line sequences `similar` split the file into; `reconstruct_content`
/// relies on them lining up exactly with those sequences.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub id: String,
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDiff {
    pub text: String,
    pub hunks: Vec<Hunk>,
}

pub fn diff_file(old_content: &str, new_content: &str, file_path: &str) -> FileDiff {
    if old_content == new_content {
        return FileDiff::default();
    }

    let diff = TextDiff::from_lines(old_content, new_content);
    let text = diff
        .unified_diff()
        .context_radius(CONTEXT_RADIUS)
        .header(&format!("a/{file_path}"), &format!("b/{file_path}"))
        .to_string();

    let hunks = diff
        .grouped_ops(CONTEXT_RADIUS)
        .into_iter()
        .filter(|group| !group.is_empty())
        .enumerate()
        .map(|(index, group)| {
            let old_start = group.first().unwrap().old_range().start;
            let old_end = group.last().unwrap().old_range().end;
            let new_start = group.first().unwrap().new_range().start;
            let new_end = group.last().unwrap().new_range().end;
            let lines = group
                .iter()
                .flat_map(|op| diff.iter_changes(op))
                .map(|change| DiffLine {
                    tag: match change.tag() {
                        ChangeTag::Equal => "context",
                        ChangeTag::Insert => "insert",
                        ChangeTag::Delete => "delete",
                    }
                    .to_string(),
                    text: change.value().trim_end_matches('\n').to_string(),
                })
                .collect();
            Hunk {
                id: format!("hunk_{index}"),
                old_start,
                old_lines: old_end - old_start,
                new_start,
                new_lines: new_end - new_start,
                lines,
            }
        })
        .collect();

    FileDiff { text, hunks }
}

/// Backward-compatible whole-file diff text, used wherever only the display
/// string is needed (e.g. before a caller cares about per-hunk data).
pub fn create_unified_diff(old_content: &str, new_content: &str, file_path: &str) -> String {
    diff_file(old_content, new_content, file_path).text
}

/// Reconstructs file content from `old_content` by applying only the
/// accepted hunks, keeping every other hunk's old-side content. Lines
/// outside any hunk are identical between old and new content by
/// construction (`similar` only groups genuinely different regions into
/// hunks), so they're taken from `old_content` regardless of acceptance.
pub fn reconstruct_content(
    old_content: &str,
    new_content: &str,
    hunks: &[Hunk],
    accepted_hunk_ids: &[String],
) -> String {
    let diff = TextDiff::from_lines(old_content, new_content);
    let old_slices: Vec<&str> = diff.iter_old_slices().collect();
    let new_slices: Vec<&str> = diff.iter_new_slices().collect();

    let mut result = String::new();
    let mut old_cursor = 0usize;
    for hunk in hunks {
        if hunk.old_start > old_cursor {
            for slice in &old_slices[old_cursor..hunk.old_start] {
                result.push_str(slice);
            }
        }
        let accepted = accepted_hunk_ids.iter().any(|id| id == &hunk.id);
        if accepted {
            for slice in &new_slices[hunk.new_start..hunk.new_start + hunk.new_lines] {
                result.push_str(slice);
            }
        } else {
            for slice in &old_slices[hunk.old_start..hunk.old_start + hunk.old_lines] {
                result.push_str(slice);
            }
        }
        old_cursor = hunk.old_start + hunk.old_lines;
    }
    if old_cursor < old_slices.len() {
        for slice in &old_slices[old_cursor..] {
            result.push_str(slice);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_diff_for_identical_content() {
        let result = diff_file("same\n", "same\n", "file.txt");
        assert_eq!(result, FileDiff::default());
    }

    #[test]
    fn produces_hunk_with_context_and_change_lines() {
        let old = "one\ntwo\nthree\nfour\nfive\n";
        let new = "one\ntwo\nCHANGED\nfour\nfive\n";
        let result = diff_file(old, new, "file.txt");

        assert_eq!(result.hunks.len(), 1);
        let hunk = &result.hunks[0];
        assert!(hunk.lines.iter().any(|line| line.tag == "delete" && line.text == "three"));
        assert!(hunk.lines.iter().any(|line| line.tag == "insert" && line.text == "CHANGED"));
        assert!(hunk.lines.iter().any(|line| line.tag == "context" && line.text == "two"));
        assert!(result.text.contains("@@"));
        assert!(result.text.contains("-three"));
        assert!(result.text.contains("+CHANGED"));
    }

    #[test]
    fn separates_distant_changes_into_multiple_hunks() {
        let old = (1..=30).map(|n| format!("line{n}\n")).collect::<String>();
        let mut new_lines: Vec<String> = (1..=30).map(|n| format!("line{n}\n")).collect();
        new_lines[1] = "CHANGED_2\n".to_string();
        new_lines[27] = "CHANGED_28\n".to_string();
        let new = new_lines.concat();

        let result = diff_file(&old, &new, "file.txt");
        assert_eq!(result.hunks.len(), 2);
    }

    #[test]
    fn reconstructs_full_new_content_when_all_hunks_accepted() {
        let old = "one\ntwo\nthree\nfour\nfive\n";
        let new = "one\ntwo\nCHANGED\nfour\nfive\n";
        let result = diff_file(old, new, "file.txt");
        let accepted: Vec<String> = result.hunks.iter().map(|hunk| hunk.id.clone()).collect();

        let reconstructed = reconstruct_content(old, new, &result.hunks, &accepted);
        assert_eq!(reconstructed, new);
    }

    #[test]
    fn reconstructs_old_content_when_no_hunks_accepted() {
        let old = "one\ntwo\nthree\nfour\nfive\n";
        let new = "one\ntwo\nCHANGED\nfour\nfive\n";
        let result = diff_file(old, new, "file.txt");

        let reconstructed = reconstruct_content(old, new, &result.hunks, &[]);
        assert_eq!(reconstructed, old);
    }

    #[test]
    fn reconstructs_partial_accept_with_multiple_hunks() {
        let old = (1..=30).map(|n| format!("line{n}\n")).collect::<String>();
        let mut new_lines: Vec<String> = (1..=30).map(|n| format!("line{n}\n")).collect();
        new_lines[1] = "CHANGED_2\n".to_string();
        new_lines[27] = "CHANGED_28\n".to_string();
        let new = new_lines.concat();
        let result = diff_file(&old, &new, "file.txt");
        assert_eq!(result.hunks.len(), 2);

        // Accept only the second hunk.
        let accepted = vec![result.hunks[1].id.clone()];
        let reconstructed = reconstruct_content(&old, &new, &result.hunks, &accepted);

        let mut expected_lines: Vec<String> = (1..=30).map(|n| format!("line{n}\n")).collect();
        expected_lines[27] = "CHANGED_28\n".to_string();
        assert_eq!(reconstructed, expected_lines.concat());
    }
}
