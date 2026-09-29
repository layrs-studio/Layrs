//! Explicit authenticated client assertions, never local Operation replication.
use super::PrincipalId;
use layrs_core::scm::{
    ActorId, CaptureReason, Metadata, ObservationId, SpaceId, StepId, StepObservation, Timestamp,
};

pub fn client_observation_id(
    space: &SpaceId,
    principal: &PrincipalId,
    step: &StepId,
    observed_at: &Timestamp,
    reason: CaptureReason,
) -> Result<ObservationId, serde_json::Error> {
    let bytes = serde_jcs::to_vec(&(
        "layrs.client-step-observation.v1",
        space,
        principal,
        step,
        observed_at,
        reason,
    ))?;
    Ok(
        ObservationId::parse(format!("observation:{}", blake3::hash(&bytes)))
            .expect("valid observation digest"),
    )
}

/// The server independently checks actor against the authenticated principal.
/// This is an explicit assertion; it does not copy a local observation identity.
pub fn client_observation(
    space: &SpaceId,
    principal: &PrincipalId,
    actor: &ActorId,
    step: &StepId,
    observed_at: &Timestamp,
    reason: CaptureReason,
) -> Result<StepObservation, serde_json::Error> {
    Ok(StepObservation::new(
        client_observation_id(space, principal, step, observed_at, reason)?,
        step.clone(),
        Some(space.clone()),
        None,
        None,
        Some(actor.clone()),
        None,
        observed_at.clone(),
        reason,
        Metadata::default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_binds_authority_step_and_claim_but_contains_no_local_authority() {
        let space = SpaceId::parse("space:one").unwrap();
        let principal = PrincipalId::parse("principal:alice").unwrap();
        let actor = ActorId::parse("actor:alice").unwrap();
        let step = StepId::parse(format!("blake3:{}", "1".repeat(64))).unwrap();
        let when = Timestamp::parse("2020-01-02T03:04:05Z").unwrap();
        let observation = client_observation(
            &space,
            &principal,
            &actor,
            &step,
            &when,
            CaptureReason::ManualSnapshot,
        )
        .unwrap();
        assert_eq!(
            observation.id(),
            &client_observation_id(
                &space,
                &principal,
                &step,
                &when,
                CaptureReason::ManualSnapshot
            )
            .unwrap()
        );
        assert_ne!(
            observation.id(),
            &client_observation_id(
                &space,
                &PrincipalId::parse("principal:bob").unwrap(),
                &step,
                &when,
                CaptureReason::ManualSnapshot
            )
            .unwrap()
        );
        assert_ne!(
            observation.id(),
            &client_observation_id(
                &SpaceId::parse("space:two").unwrap(),
                &principal,
                &step,
                &when,
                CaptureReason::ManualSnapshot
            )
            .unwrap()
        );
        assert!(
            observation.operation_id().is_none()
                && observation.worktree_id().is_none()
                && observation.device_id().is_none()
        );
        assert_eq!(observation.metadata(), &Metadata::default());
    }
}
