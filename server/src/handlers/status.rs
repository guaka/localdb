use axum::{extract::State, response::Html, Json};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};

use localdb_core::{resolve_named_stores, Error, StoreRow, TableSize};

use crate::error::ApiError;
use crate::state::{store_visibility_to_str, AppState};

/// One store's status figures, mirroring the embedded CLI's
/// `cmds::status::StoreStatusEntry` (issue #187 stage 5) — the two must stay
/// in lockstep or `localdb status` would render different numbers depending
/// on whether a daemon happens to be running.
#[derive(Debug, Serialize)]
pub struct StoreStatusRecord {
    pub name: String,
    pub visibility: String,
    pub backend: String,
    /// `None` when `RetrievalStore::stats()` failed for this store (e.g. a
    /// corrupt or mid-migration store) — `status` must keep reporting on the
    /// daemon state and the other stores rather than aborting outright.
    pub document_count: Option<u64>,
    pub chunk_count: Option<u64>,
}

/// The shared `localdb.db` file's on-disk figures — reported once, not
/// per-store (specs/03-config.md: one physical file backs every store).
#[derive(Debug, Serialize)]
pub struct DatabaseStatus {
    pub path: String,
    pub exists: bool,
    pub size_bytes: Option<u64>,
    pub wal_size_bytes: Option<u64>,
    pub total_size_bytes: u64,
    pub bytes_per_chunk: Option<u64>,
    pub largest_tables: Vec<TableSize>,
}

/// How many rows `largest_tables` reports — matches the embedded CLI's
/// `cmds::status::LARGEST_TABLES_LIMIT`.
const LARGEST_TABLES_LIMIT: usize = 5;

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub daemon: bool,
    pub store_count: usize,
    pub source_count: usize,
    pub job_count: usize,
    /// Per-store figures (issue #187 stage 5) — added so the CLI's
    /// daemon-routed `status` can render identically to embedded `status`
    /// instead of only reporting a bare `store_count`.
    pub stores: Vec<StoreStatusRecord>,
    pub database: DatabaseStatus,
}

/// `GET /status` query params: a repeatable `?store=` scopes the response to
/// specific stores, mirroring CLI `--store` (issue #187 review, finding F7).
/// `Vec<String>` + `#[serde(default)]`, not `Option<Vec<_>>` —
/// `axum_extra::extract::Query`'s own guidance for correctly handling zero,
/// one, or many repeated params of the same name.
#[derive(Debug, Deserialize)]
pub struct StatusQuery {
    #[serde(default)]
    pub store: Vec<String>,
}

/// Resolve `?store=` names against the DB-backed store list, reading raw
/// `StoreRow`s (name/id/visibility/backend) rather than the parsed
/// `EffectiveConfig` — `get_status` never reads a store's parsed
/// `.indexing` policy, so it must not require every store's
/// `indexing_policy` JSON to parse just to answer a scoped (or unscoped)
/// status request (Codex review finding G2, issue #187 PR #212). Empty
/// `names` lists every store; a non-empty list resolves through the same
/// `resolve_named_stores` helper the embedded CLI's `resolve_store_scope_inner`
/// (`cli/src/app_db.rs`) uses, so an unknown name is `Error::StoreNotFound`
/// (→ 404) in both surfaces alike.
async fn resolve_status_scope(state: &AppState, names: &[String]) -> Result<Vec<StoreRow>, Error> {
    if names.is_empty() {
        return state.backend().list_stores().await;
    }
    resolve_named_stores(state.backend(), names).await
}

pub async fn get_status(
    State(state): State<AppState>,
    Query(query): Query<StatusQuery>,
) -> Result<Json<StatusResponse>, ApiError> {
    Ok(Json(build_status_response(&state, &query.store).await?))
}

pub async fn get_status_page(State(state): State<AppState>) -> Result<Html<String>, ApiError> {
    let status = build_status_response(&state, &[]).await?;
    Ok(Html(render_status_page(&status)))
}

async fn build_status_response(
    state: &AppState,
    store_names: &[String],
) -> Result<StatusResponse, Error> {
    let scoped_stores = resolve_status_scope(state, store_names).await?;
    let store_count = scoped_stores.len();

    let mut source_count = 0;
    let mut stores = Vec::with_capacity(scoped_stores.len());
    for store in &scoped_stores {
        // Best-effort, exactly like the stats call just below: a single
        // store's source listing failing (e.g. a corrupt or mid-migration
        // store) must not blank out the whole status report — it simply
        // doesn't contribute to `source_count`.
        if let Ok(sources) = state.list_sources(&store.name).await {
            source_count += sources.len();
        }

        // Best-effort, exactly like the embedded path's
        // `gather_store_status`: a single corrupt or mid-migration store
        // must not blank out the whole status report.
        let stats = match state.backend().retrieval_store(&store.id).await {
            Ok(retrieval) => retrieval.stats().await.ok(),
            Err(_) => None,
        };
        stores.push(StoreStatusRecord {
            name: store.name.clone(),
            visibility: store_visibility_to_str(&store.visibility).to_string(),
            backend: store.backend.clone(),
            document_count: stats.as_ref().map(|s| s.document_count),
            chunk_count: stats.as_ref().map(|s| s.chunk_count),
        });
    }

    let jobs = state.job_queue().list_jobs().await;

    let db_path = state.data_dir().join("localdb.db");
    let db_size = localdb_core::compute_db_file_size(&db_path);
    let total_chunks: u64 = stores.iter().filter_map(|s| s.chunk_count).sum();
    let largest_tables = state
        .backend()
        .largest_tables(LARGEST_TABLES_LIMIT)
        .await
        .unwrap_or_default();

    Ok(StatusResponse {
        daemon: true,
        store_count,
        source_count,
        job_count: jobs.len(),
        stores,
        database: DatabaseStatus {
            path: db_path.display().to_string(),
            exists: db_size.main_bytes.is_some(),
            size_bytes: db_size.main_bytes,
            wal_size_bytes: db_size.wal_bytes,
            total_size_bytes: db_size.total_bytes(),
            bytes_per_chunk: localdb_core::bytes_per_chunk(db_size.total_bytes(), total_chunks),
            largest_tables,
        },
    })
}

fn render_status_page(status: &StatusResponse) -> String {
    let document_count: u64 = status
        .stores
        .iter()
        .filter_map(|store| store.document_count)
        .sum();
    let chunk_count: u64 = status
        .stores
        .iter()
        .filter_map(|store| store.chunk_count)
        .sum();
    let stores = if status.stores.is_empty() {
        r#"<tr><td colspan="5" class="empty">No stores configured yet.</td></tr>"#.to_string()
    } else {
        status
            .stores
            .iter()
            .map(|store| {
                format!(
                    r#"<tr>
    <td><strong>{}</strong></td>
    <td>{}</td>
    <td>{}</td>
    <td class="numeric">{}</td>
    <td class="numeric">{}</td>
</tr>"#,
                    escape_html(&store.name),
                    escape_html(&store.visibility),
                    escape_html(&store.backend),
                    format_optional_count(store.document_count),
                    format_optional_count(store.chunk_count),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta http-equiv="refresh" content="15">
    <title>localdb status</title>
    <style>
        :root {{
            color-scheme: light dark;
            --bg: #f7f8fa;
            --panel: #ffffff;
            --text: #1f2328;
            --muted: #5f6b7a;
            --line: #d8dee8;
            --accent: #2e7d62;
            --accent-soft: #e4f3ed;
        }}
        @media (prefers-color-scheme: dark) {{
            :root {{
                --bg: #111418;
                --panel: #191d23;
                --text: #edf1f5;
                --muted: #a5aebb;
                --line: #303844;
                --accent: #6fcea6;
                --accent-soft: #143827;
            }}
        }}
        * {{ box-sizing: border-box; }}
        body {{
            margin: 0;
            min-height: 100vh;
            background: var(--bg);
            color: var(--text);
            font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
            line-height: 1.45;
        }}
        main {{
            width: min(1120px, calc(100vw - 32px));
            margin: 0 auto;
            padding: 40px 0;
        }}
        header {{
            display: flex;
            align-items: flex-start;
            justify-content: space-between;
            gap: 24px;
            margin-bottom: 24px;
        }}
        h1 {{
            margin: 0 0 6px;
            font-size: clamp(2rem, 5vw, 3.5rem);
            letter-spacing: 0;
        }}
        p {{ margin: 0; color: var(--muted); }}
        .badge {{
            display: inline-flex;
            align-items: center;
            gap: 8px;
            min-height: 32px;
            padding: 4px 10px;
            border-radius: 999px;
            background: var(--accent-soft);
            color: var(--accent);
            font-weight: 700;
            white-space: nowrap;
        }}
        .dot {{
            width: 8px;
            height: 8px;
            border-radius: 50%;
            background: currentColor;
        }}
        .metrics {{
            display: grid;
            grid-template-columns: repeat(5, minmax(0, 1fr));
            gap: 12px;
            margin: 24px 0;
        }}
        .metric {{
            min-width: 0;
            border: 1px solid var(--line);
            border-radius: 8px;
            background: var(--panel);
            padding: 16px;
        }}
        .metric span {{
            display: block;
            color: var(--muted);
            font-size: 0.9rem;
        }}
        .metric strong {{
            display: block;
            margin-top: 8px;
            font-size: 1.8rem;
            line-height: 1;
        }}
        section {{
            border: 1px solid var(--line);
            border-radius: 8px;
            background: var(--panel);
            overflow: hidden;
        }}
        .section-head {{
            display: flex;
            align-items: center;
            justify-content: space-between;
            gap: 16px;
            padding: 16px;
            border-bottom: 1px solid var(--line);
        }}
        h2 {{
            margin: 0;
            font-size: 1rem;
            letter-spacing: 0;
        }}
        a {{ color: var(--accent); }}
        table {{
            width: 100%;
            border-collapse: collapse;
        }}
        th, td {{
            padding: 14px 16px;
            border-bottom: 1px solid var(--line);
            text-align: left;
            vertical-align: top;
        }}
        th {{
            color: var(--muted);
            font-size: 0.78rem;
            text-transform: uppercase;
            letter-spacing: 0;
        }}
        tr:last-child td {{ border-bottom: 0; }}
        .numeric {{
            text-align: right;
            font-variant-numeric: tabular-nums;
        }}
        .empty {{
            color: var(--muted);
            text-align: center;
        }}
        footer {{
            margin-top: 16px;
            color: var(--muted);
            font-size: 0.9rem;
        }}
        @media (max-width: 820px) {{
            main {{ width: min(100vw - 24px, 1120px); padding-top: 24px; }}
            header {{ display: block; }}
            .badge {{ margin-top: 14px; }}
            .metrics {{ grid-template-columns: repeat(2, minmax(0, 1fr)); }}
            section {{ overflow-x: auto; }}
            table {{ min-width: 680px; }}
        }}
    </style>
</head>
<body>
    <main>
        <header>
            <div>
                <h1>localdb status</h1>
                <p>Auto-refreshes every 15 seconds. Project site: <a href="https://guaka.github.io/localdb/">guaka.github.io/localdb</a>. JSON is available at <a href="/v1/status">/v1/status</a>.</p>
            </div>
            <div class="badge"><span class="dot"></span>{}</div>
        </header>

        <div class="metrics" aria-label="Store summary">
            <div class="metric"><span>Stores</span><strong>{}</strong></div>
            <div class="metric"><span>Sources</span><strong>{}</strong></div>
            <div class="metric"><span>Documents</span><strong>{}</strong></div>
            <div class="metric"><span>Chunks</span><strong>{}</strong></div>
            <div class="metric"><span>Jobs</span><strong>{}</strong></div>
        </div>

        <section>
            <div class="section-head">
                <h2>Stores</h2>
                <p>Database: {}</p>
            </div>
            <table>
                <thead>
                    <tr>
                        <th>Store</th>
                        <th>Visibility</th>
                        <th>Backend</th>
                        <th class="numeric">Documents</th>
                        <th class="numeric">Chunks</th>
                    </tr>
                </thead>
                <tbody>
                    {}
                </tbody>
            </table>
        </section>
        <footer>Data directory: {}</footer>
    </main>
</body>
</html>"#,
        if status.daemon {
            "Daemon online"
        } else {
            "Daemon offline"
        },
        status.store_count,
        status.source_count,
        document_count,
        chunk_count,
        status.job_count,
        format_bytes(status.database.total_size_bytes),
        stores,
        escape_html(&status.database.path),
    )
}

fn format_optional_count(value: Option<u64>) -> String {
    value
        .map(|count| count.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
