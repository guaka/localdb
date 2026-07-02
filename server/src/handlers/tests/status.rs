use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use super::common::{
    json_body, make_app, make_state_with_fake_config, seed_store_a_chunk, SeedChunkInput,
};

#[tokio::test]
async fn get_status_returns_daemon_true() {
    let (_dir, app) = make_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp.into_body()).await;
    assert_eq!(body["daemon"], true);
}

#[tokio::test]
async fn get_status_reports_per_store_counts() {
    let (_dir, state) = make_state_with_fake_config().await;
    seed_store_a_chunk(
        &state,
        SeedChunkInput {
            chunk_id: "chunk-status-1",
            doc_id: "doc-status-1",
            text: "localdb status page coverage",
            uri: "file:///tmp/status.txt",
            metadata: localdb_core::DocumentMetadata::default(),
        },
    )
    .await;
    let app = crate::build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp.into_body()).await;
    assert_eq!(body["store_count"], 1);
    assert_eq!(body["source_count"], 1);
    assert_eq!(body["document_count"], 1);
    assert_eq!(body["chunk_count"], 1);
    assert_eq!(body["stores"][0]["name"], "store-A");
    assert_eq!(body["stores"][0]["source_count"], 1);
    assert_eq!(body["stores"][0]["document_count"], 1);
    assert_eq!(body["stores"][0]["chunk_count"], 1);
}

#[tokio::test]
async fn status_page_renders_store_summary_html() {
    let (_dir, state) = make_state_with_fake_config().await;
    seed_store_a_chunk(
        &state,
        SeedChunkInput {
            chunk_id: "chunk-status-page-1",
            doc_id: "doc-status-page-1",
            text: "browser-visible localdb store status",
            uri: "file:///tmp/status-page.txt",
            metadata: localdb_core::DocumentMetadata::default(),
        },
    )
    .await;
    let app = crate::build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(content_type.starts_with("text/html"));

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(html.contains("localdb status"));
    assert!(html.contains("store-A"));
    assert!(html.contains("https://guaka.github.io/localdb/"));
    assert!(html.contains("<td class=\"numeric\">1</td>"));
    assert!(html.contains("/v1/status"));
}
