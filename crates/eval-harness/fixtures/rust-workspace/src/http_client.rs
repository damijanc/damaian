/// Builds an absolute request URL from a base and a path.
pub fn build_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}
