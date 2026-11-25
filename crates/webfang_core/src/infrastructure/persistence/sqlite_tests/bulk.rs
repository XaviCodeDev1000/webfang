//! Bulk chunk-vector read tests over the SQLite repository.

use super::sqlite::fresh_repo;
use super::VectorRepository;

#[tokio::test]
async fn bulk_chunk_vectors_skip_null_embeddings() {
    let (_dir, repo) = fresh_repo(4).await;
    let url = "https://example.com/bulk";
    repo.save_resource(url, "Bulk", "hash-bulk", 1)
        .await
        .expect("save_resource");
    let embedding = vec![1.0_f32, 0.0, 0.0];
    repo.save_chunk("with", url, 0, "valid", Some(&embedding), None)
        .await
        .expect("save_chunk with embedding");
    repo.save_chunk("without", url, 1, "null embedding", None, None)
        .await
        .expect("save_chunk without embedding");

    let rows = repo.load_chunk_vectors(None).await.expect("bulk read");
    assert_eq!(rows.len(), 1, "NULL-embedding rows must be filtered");
    assert_eq!(rows[0].id, "with");
    assert_eq!(rows[0].content, "valid");
    assert_eq!(rows[0].embedding, embedding);
}
