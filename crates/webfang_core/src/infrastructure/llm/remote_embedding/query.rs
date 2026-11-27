//! Query-side embedding methods (#3074).
//!
//! Asymmetric models need a different body field for vault lookups than for
//! indexing. These methods apply the `query_extra_body` overlay; they live in
//! a sibling module so the parent stays under the 500-line ratchet.

use super::*;

impl RemoteEmbeddingAdapter {
    pub(super) fn embed_query_impl<'a>(
        &'a self,
        text: &'a str,
    ) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
        let request_id = uuid::Uuid::new_v4();
        let merged =
            crate::domain::providers::merge_extra_body(&self.extra_body, &self.query_extra_body);
        Box::pin(async move {
            let batch = self
                .embed_batch_impl_with(std::slice::from_ref(&text.to_owned()), request_id, &merged)
                .await?;
            batch.into_iter().next().ok_or_else(|| {
                SemanticError::Inference("embedding endpoint returned empty `data`".to_string())
            })
        })
    }

    pub(super) fn embed_query_batch_impl<'a>(
        &'a self,
        texts: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, SemanticError>> {
        let request_id = uuid::Uuid::new_v4();
        let merged =
            crate::domain::providers::merge_extra_body(&self.extra_body, &self.query_extra_body);
        Box::pin(async move { self.embed_batch_impl_with(texts, request_id, &merged).await })
    }
}
