use super::*;

#[test]
fn the_run_command_description_names_the_shell_and_warns_about_zsh_globs() {
    let zsh = run_command_tool_definition("/bin/zsh").description;
    assert!(zsh.contains("zsh -lc"), "zsh run_command: {zsh}");
    assert!(
        zsh.contains("Quote glob patterns"),
        "zsh must teach the glob-quoting rule: {zsh}"
    );
    assert!(!zsh.contains("/bin/zsh"), "full path leaked: {zsh}");

    let bash = run_command_tool_definition("/bin/bash").description;
    assert!(bash.contains("bash -lc"), "bash run_command: {bash}");
    assert!(
        !bash.contains("Quote glob patterns"),
        "bash must not teach the zsh-only rule: {bash}"
    );
    assert!(!bash.contains("/bin/bash"), "full path leaked: {bash}");
}

#[test]
fn the_envelope_paragraphs_name_the_shell_and_warn_about_zsh_globs() {
    for zsh_paragraph in [
        command_envelope_paragraph_unrestricted("/bin/zsh"),
        command_envelope_paragraph_read_only("/bin/zsh"),
    ] {
        assert!(zsh_paragraph.contains("zsh -lc"), "{zsh_paragraph}");
        assert!(
            zsh_paragraph.contains("Quote glob patterns"),
            "zsh must teach the glob-quoting rule: {zsh_paragraph}"
        );
        assert!(
            !zsh_paragraph.contains("/bin/zsh"),
            "full path leaked: {zsh_paragraph}"
        );
    }

    for bash_paragraph in [
        command_envelope_paragraph_unrestricted("/bin/bash"),
        command_envelope_paragraph_read_only("/bin/bash"),
    ] {
        assert!(bash_paragraph.contains("bash -lc"), "{bash_paragraph}");
        assert!(
            !bash_paragraph.contains("Quote glob patterns"),
            "bash must not teach the zsh-only rule: {bash_paragraph}"
        );
        assert!(
            !bash_paragraph.contains("/bin/bash"),
            "full path leaked: {bash_paragraph}"
        );
    }
}

#[test]
fn only_the_basename_of_a_non_default_shell_path_is_named() {
    let paragraph = command_envelope_paragraph_unrestricted("/opt/homebrew/bin/zsh");
    assert!(paragraph.contains("zsh -lc"), "{paragraph}");
    assert!(
        !paragraph.contains("/opt/homebrew/bin/zsh"),
        "full path leaked: {paragraph}"
    );

    let description = run_command_tool_definition("/opt/homebrew/bin/bash").description;
    assert!(description.contains("bash -lc"), "{description}");
    assert!(
        !description.contains("/opt/homebrew/bin/bash"),
        "full path leaked: {description}"
    );
}

/// Spec 64 requirement 5: the model is pointed at `read_git_diff` and
/// `read_git_log` for diff and commit content, and the shell forms that
/// now ask (`git diff`, `git show`, `git log -p`) leave the "prefer
/// read-only commands" list.
#[test]
fn the_command_description_and_envelope_paragraphs_point_to_the_git_read_tools() {
    for text in [
        run_command_tool_definition("/bin/zsh").description,
        command_envelope_paragraph_unrestricted("/bin/zsh"),
        command_envelope_paragraph_read_only("/bin/zsh"),
    ] {
        assert!(text.contains("read_git_diff"), "{text}");
        assert!(text.contains("read_git_log"), "{text}");
        assert!(
            !text.contains("git show, git diff"),
            "the read-only list must not still advertise git show/diff: {text}"
        );
    }
}
