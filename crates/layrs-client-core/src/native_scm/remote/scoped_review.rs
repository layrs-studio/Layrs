use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ScopedReviewCommand {
    List {
        #[serde(default)]
        query: ScopedReviewListQuery,
    },
    Propose {
        request: ScopedReviewProposalRequest,
    },
    Detail {
        review_id: String,
    },
    Act {
        review_id: String,
        request: ScopedReviewActionRequest,
    },
    Status {
        review_id: String,
    },
    Integrate {
        review_id: String,
        request: ScopedReviewIntegrationRequest,
    },
}
impl NativeRemote {
    pub fn scoped_review(
        &self,
        space: &SpaceId,
        command: &ScopedReviewCommand,
    ) -> RemoteResult<serde_json::Value> {
        let endpoint = format!("v3/spaces/{space}/scoped-reviews");
        let review_path = |id: &str| -> RemoteResult<String> {
            if id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_'))
            {
                return Err(NativeRemoteError::Invalid(
                    "invalid scoped review identifier",
                ));
            }
            Ok(format!("{endpoint}/{id}"))
        };
        match command {
            ScopedReviewCommand::List { query } => {
                let response = self
                    .http
                    .get(self.url(&endpoint)?)
                    .bearer_auth(&self.token)
                    .query(query)
                    .timeout(self.transfer_timeout())
                    .send()?;
                let page: ScopedReviewPage = json(response)?;
                Ok(serde_json::to_value(page)?)
            }
            ScopedReviewCommand::Propose { request } => {
                let value: ScopedReview = self.post(&endpoint, request)?;
                Ok(serde_json::to_value(value)?)
            }
            ScopedReviewCommand::Detail { review_id } => {
                let value: ScopedReviewDetail = self.get(&review_path(review_id)?)?;
                Ok(serde_json::to_value(value)?)
            }
            ScopedReviewCommand::Act { review_id, request } => {
                let value: ScopedReviewAction =
                    self.post(&format!("{}/actions", review_path(review_id)?), request)?;
                Ok(serde_json::to_value(value)?)
            }
            ScopedReviewCommand::Status { review_id } => {
                let value: ScopedIntegrationStatus =
                    self.get(&format!("{}/integration-status", review_path(review_id)?))?;
                Ok(serde_json::to_value(value)?)
            }
            ScopedReviewCommand::Integrate { review_id, request } => {
                let receipt: SyncTransactionReceipt =
                    self.post(&format!("{}/integrate", review_path(review_id)?), request)?;
                receipt.validate_v3().map_err(|_| {
                    NativeRemoteError::Invalid("invalid scoped integration receipt")
                })?;
                let mut hash = blake3::Hasher::new();
                hash.update(b"layrs-scoped-review-integration-v1\0");
                hash.update(receipt.principal_id.as_str().as_bytes());
                hash.update(b"\0");
                hash.update(review_id.as_bytes());
                hash.update(b"\0");
                hash.update(request.idempotency_key.as_str().as_bytes());
                if &receipt.space_id != space
                    || receipt.client_id.as_str() != "client:scoped-integration"
                    || receipt.idempotency_key.as_str() != format!("scoped-int-{}", hash.finalize())
                {
                    return Err(NativeRemoteError::Invalid(
                        "scoped integration correlation mismatch",
                    ));
                }
                Ok(serde_json::json!({"receipt":receipt}))
            }
        }
    }
}
