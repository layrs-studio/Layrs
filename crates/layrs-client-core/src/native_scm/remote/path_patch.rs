use super::*;
impl NativeRemote {
    /// Submit an exact patch. Interactive callers must durably save the entire
    /// submission before dispatch and retry it unchanged until a terminal receipt.
    pub fn submit_path_patch(
        &self,
        submission: &PathPatchSubmission,
    ) -> RemoteResult<PathPatchReceipt> {
        submission
            .request
            .validate()
            .map_err(|_| NativeRemoteError::Invalid("invalid path patch"))?;
        let request = &submission.request;
        let receipt: PathPatchReceipt = self.post(
            &format!("v3/spaces/{}/path-patches", request.space_id),
            submission,
        )?;
        receipt
            .receipt
            .validate_v3()
            .map_err(|_| NativeRemoteError::Invalid("invalid path patch receipt"))?;
        if receipt.request_digest != request.request_digest
            || receipt.receipt.space_id != request.space_id
            || receipt.receipt.client_id != request.client_id
            || receipt.receipt.idempotency_key != request.idempotency_key
        {
            return Err(NativeRemoteError::Invalid(
                "path patch receipt correlation mismatch",
            ));
        }
        Ok(receipt)
    }
}
