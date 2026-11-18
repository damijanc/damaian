use crate::error::ClientError;
use crate::verification::{CheckTermination, environment_termination};

#[test]
fn only_policy_and_a_real_spawn_failure_are_environment_facts() {
    assert_eq!(
        environment_termination(&ClientError::PolicyBlocked("blocked".to_string())),
        Some(CheckTermination::PolicyRefused)
    );
    assert_eq!(
        environment_termination(&ClientError::ApprovalRequired("approval".to_string())),
        Some(CheckTermination::PolicyRefused)
    );
    assert_eq!(
        environment_termination(&ClientError::CommandSpawnFailed("no shell".to_string())),
        Some(CheckTermination::SpawnFailed)
    );
    // Every unrelated failure must propagate. `Io` is the important one:
    // the previous mapping turned any I/O error into a spawn failure.
    for error in [
        ClientError::Io("execution store is read-only".to_string()),
        ClientError::Cancelled,
        ClientError::InvalidInput("bad proposal".to_string()),
        ClientError::AccessDenied("no".to_string()),
        ClientError::Git("git failed".to_string()),
    ] {
        assert_eq!(environment_termination(&error), None, "{error:?}");
    }
}
