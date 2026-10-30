//! `config-set ... model_provider` must say when an explicit global still
//! overrides the new provider's defaults. Spec 18 §7, observation #45.
//!
//! The installed configuration spells out `model_base_url`, `model_api_key_env`
//! and `model_name`; leaving them behind made the file name OpenRouter while
//! the process still used DeepSeek's endpoint, key variable and model. The
//! command does not delete the user's lines, so it has to name each one and the
//! command that replaces it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_data_dir(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("damaian-cli-provider-{name}-{stamp}"));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn write_user_config(data_dir: &Path, text: &str) {
    let config_dir = data_dir.join("config");
    fs::create_dir_all(&config_dir).expect("config dir should be created");
    fs::write(config_dir.join("user.conf"), text).expect("user config should be written");
}

fn config_set(data_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_damaian"))
        .arg("config-set")
        .args(args)
        .env("DAMAIAN_DATA_DIR", data_dir)
        .env_remove("DAMAIAN_ADMIN_CONFIG")
        // Provider defaults read these, so a developer shell must not change
        // what the warning names.
        .env_remove("OPENROUTER_BASE_URL")
        .env_remove("OPENROUTER_MODEL")
        .env_remove("DEEPSEEK_BASE_URL")
        .env_remove("DEEPSEEK_MODEL")
        .output()
        .expect("cli should run")
}

#[test]
fn switching_provider_warns_about_each_explicit_global_and_keeps_it() {
    let data_dir = temp_data_dir("installed-shape");
    write_user_config(
        &data_dir,
        "model_provider=deepseek\n\
         model_name=deepseek-v4-flash\n\
         model_base_url=https://api.deepseek.com\n\
         model_api_key_env=keychain:model-api-key\n",
    );

    let output = config_set(&data_dir, &["user", "model_provider", "openrouter"]);
    assert!(
        output.status.success(),
        "config-set should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "model_base_url is set explicitly in user config (https://api.deepseek.com)",
        "damaian config-set user model_base_url https://openrouter.ai/api/v1",
        "model_api_key_env is set explicitly in user config (keychain:model-api-key)",
        "damaian config-set user model_api_key_env OPENROUTER_API_KEY",
        "model_name is set explicitly in user config (deepseek-v4-flash)",
        "damaian config-set user model_name anthropic/claude-sonnet-5.5",
    ] {
        assert!(
            stderr.contains(expected),
            "warning should contain {expected:?}:\n{stderr}"
        );
    }

    // The user's lines are still there: the command warns, it does not rewrite
    // settings the user chose.
    let user_conf = fs::read_to_string(data_dir.join("config/user.conf"))
        .expect("user config should still exist");
    assert!(user_conf.contains("model_base_url=https://api.deepseek.com"));
    assert!(user_conf.contains("model_api_key_env=keychain:model-api-key"));
    assert!(user_conf.contains("model_name=deepseek-v4-flash"));
    assert!(user_conf.contains("model_provider=openrouter"));
}

#[test]
fn a_provider_switch_with_no_explicit_overrides_is_silent() {
    let data_dir = temp_data_dir("fresh");

    let output = config_set(&data_dir, &["user", "model_provider", "openrouter"]);
    assert!(
        output.status.success(),
        "config-set should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("warning:"),
        "a fresh config has nothing to override: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_explicit_value_matching_the_new_default_is_not_a_warning() {
    let data_dir = temp_data_dir("matching");
    write_user_config(
        &data_dir,
        "model_name=anthropic/claude-sonnet-5.5\n\
         model_base_url=https://openrouter.ai/api/v1\n\
         model_api_key_env=OPENROUTER_API_KEY\n",
    );

    let output = config_set(&data_dir, &["user", "model_provider", "openrouter"]);
    assert!(
        output.status.success(),
        "config-set should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("warning:"),
        "values already equal to the provider defaults do not override them: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A provider's own keychain key wins over the global reference (spec 73), so
/// the global line is not overriding anything and naming it would send the
/// user to change a setting that has no effect.
#[test]
fn a_global_key_reference_the_provider_key_outranks_is_not_a_warning() {
    let data_dir = temp_data_dir("provider-key");
    write_user_config(
        &data_dir,
        "model_provider=openrouter\n\
         model_api_key_env=keychain:model-api-key\n\
         model_provider.deepseek.api_key_env=keychain:DEEPSEEK_API_KEY\n",
    );

    let output = config_set(&data_dir, &["user", "model_provider", "deepseek"]);
    assert!(
        output.status.success(),
        "config-set should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("model_api_key_env"),
        "the provider's own key is the one used: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
