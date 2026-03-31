//! Documentation endpoint handlers
//!
//! Serves project documentation files from a configured directory.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{extract::Path, extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;

/// Shared state for docs endpoints
#[derive(Clone)]
pub struct DocsState {
    pub docs_dir: Arc<PathBuf>,
}

/// Allowed doc files and their repo-relative paths (prevents path traversal)
const ALLOWED_DOCS: &[(&str, &str)] = &[
    ("ROADMAP.md", "ROADMAP.md"),
    ("TASKS.md", "TASKS.md"),
    ("state.md", "state.md"),
    ("ARCHITECTURE.md", "docs/ARCHITECTURE.md"),
    ("OVERVIEW.md", "docs/OVERVIEW.md"),
    ("GETTING_STARTED.md", "docs/GETTING_STARTED.md"),
];

fn doc_path(name: &str) -> Option<&'static str> {
    ALLOWED_DOCS
        .iter()
        .find_map(|(allowed, path)| (*allowed == name).then_some(*path))
}

#[derive(Serialize)]
pub struct DocEntry {
    name: String,
    available: bool,
}

/// GET /api/docs — list available documentation files
pub async fn list_docs(State(state): State<DocsState>) -> Json<Vec<DocEntry>> {
    let entries: Vec<DocEntry> = ALLOWED_DOCS
        .iter()
        .map(|(name, rel_path)| {
            let path = state.docs_dir.join(rel_path);
            DocEntry {
                name: (*name).to_string(),
                available: path.exists(),
            }
        })
        .collect();
    Json(entries)
}

/// GET /api/docs/:name — get content of a specific doc
pub async fn get_doc(
    State(state): State<DocsState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let rel_path = doc_path(&name).ok_or(StatusCode::NOT_FOUND)?;
    let path = state.docs_dir.join(rel_path);
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => Ok((
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            )],
            content,
        )),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}
