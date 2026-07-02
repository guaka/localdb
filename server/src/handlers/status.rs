use axum::{extract::State, response::Html, Json};
use serde::Serialize;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub daemon: bool,
    pub data_dir: String,
    pub store_count: usize,
    pub source_count: usize,
    pub document_count: u64,
    pub chunk_count: u64,
    pub job_count: usize,
    pub running_job_count: usize,
    pub failed_job_count: usize,
    pub stores: Vec<StoreStatus>,
}

#[derive(Debug, Serialize)]
pub struct StoreStatus {
    pub id: String,
    pub name: String,
    pub visibility: String,
    pub backend: String,
    pub source_count: usize,
    pub document_count: u64,
    pub chunk_count: u64,
}

pub async fn get_status(State(state): State<AppState>) -> Result<Json<StatusResponse>, ApiError> {
    Ok(Json(build_status_response(&state).await?))
}

pub async fn get_status_page(State(state): State<AppState>) -> Result<Html<String>, ApiError> {
    let status = build_status_response(&state).await?;
    Ok(Html(render_status_page(&status)))
}

async fn build_status_response(state: &AppState) -> Result<StatusResponse, ApiError> {
    let effective = state.effective_config().await?;
    let mut stores = Vec::with_capacity(effective.stores.len());
    let mut source_count = 0;
    let mut document_count = 0;
    let mut chunk_count = 0;

    for store in &effective.stores {
        let sources = state.list_sources(&store.name).await?;
        let stats = state
            .backend()
            .retrieval_store(&store.id)
            .await?
            .stats()
            .await?;
        let store_source_count = sources.len();
        source_count += store_source_count;
        document_count += stats.document_count;
        chunk_count += stats.chunk_count;
        stores.push(StoreStatus {
            id: store.id.clone(),
            name: store.name.clone(),
            visibility: store.visibility.clone(),
            backend: store.backend.clone(),
            source_count: store_source_count,
            document_count: stats.document_count,
            chunk_count: stats.chunk_count,
        });
    }

    let jobs = state.job_queue().list_jobs().await;
    let running_job_count = jobs
        .iter()
        .filter(|job| job.state == localdb_core::IndexJobState::Running)
        .count();
    let failed_job_count = jobs
        .iter()
        .filter(|job| job.state == localdb_core::IndexJobState::Failed)
        .count();

    Ok(StatusResponse {
        daemon: true,
        data_dir: state.data_dir().display().to_string(),
        store_count: stores.len(),
        source_count,
        document_count,
        chunk_count,
        job_count: jobs.len(),
        running_job_count,
        failed_job_count,
        stores,
    })
}

fn render_status_page(status: &StatusResponse) -> String {
    let stores = if status.stores.is_empty() {
        r#"<tr><td colspan="6" class="empty">No stores configured yet.</td></tr>"#.to_string()
    } else {
        status
            .stores
            .iter()
            .map(|store| {
                format!(
                    r#"<tr>
    <td><strong>{}</strong><span>{}</span></td>
    <td>{}</td>
    <td>{}</td>
    <td class="numeric">{}</td>
    <td class="numeric">{}</td>
    <td class="numeric">{}</td>
</tr>"#,
                    escape_html(&store.name),
                    escape_html(&store.id),
                    escape_html(&store.visibility),
                    escape_html(&store.backend),
                    store.source_count,
                    store.document_count,
                    store.chunk_count
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
            --warn: #b54708;
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
                --warn: #f0a45d;
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
        td span {{
            display: block;
            margin-top: 3px;
            color: var(--muted);
            font-size: 0.82rem;
            overflow-wrap: anywhere;
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
            table {{ min-width: 760px; }}
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
                <p>{} running, {} failed</p>
            </div>
            <table>
                <thead>
                    <tr>
                        <th>Store</th>
                        <th>Visibility</th>
                        <th>Backend</th>
                        <th class="numeric">Sources</th>
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
        status.document_count,
        status.chunk_count,
        status.job_count,
        status.running_job_count,
        status.failed_job_count,
        stores,
        escape_html(&status.data_dir)
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
