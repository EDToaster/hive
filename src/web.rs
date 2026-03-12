/// `hive web` — adversarial alternative to the ratatui TUI.
///
/// Serves a vanilla HTML/CSS/JS dashboard at http://localhost:<port>.
/// State is pushed to the browser via Server-Sent Events (SSE) every second.
/// No JS framework, no new dependencies — just tokio TCP + embedded HTML.
///
/// ## Why this beats the TUI
/// - Mouse capture works (browser handles it natively, no Zellij breakage)
/// - Text selection / copy-paste works without toggling mouse mode
/// - Browser Ctrl+F searches agents, tasks, activity for free
/// - Full CSS color support — no ANSI code variance across terminals
/// - Accessible from any device on local network (just open the URL)
/// - Can add clickable deep-links to agents/tasks/Sentry/Datadog in future
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{Duration, sleep};

use crate::state::HiveState;
use crate::types::*;

// ---------------------------------------------------------------------------
// Embedded dashboard HTML
// ---------------------------------------------------------------------------

const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Hive Dashboard</title>
<style>
  :root {
    --bg: #0d0d0d;
    --bg2: #161616;
    --bg3: #1e1e1e;
    --border: #2a2a2a;
    --text: #e0e0e0;
    --muted: #666;
    --cyan: #5cc8c8;
    --yellow: #e0c070;
    --green: #5ec870;
    --red: #e05050;
    --blue: #6090d8;
    --purple: #b070d8;
    --orange: #e09040;
    --gray: #888;
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body {
    background: var(--bg);
    color: var(--text);
    font-family: 'SF Mono', 'Cascadia Code', 'JetBrains Mono', monospace;
    font-size: 13px;
    height: 100vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  /* Header */
  #header {
    background: var(--bg2);
    border-bottom: 1px solid var(--border);
    padding: 6px 16px;
    display: flex;
    align-items: center;
    gap: 16px;
    flex-shrink: 0;
  }
  #header .logo { color: var(--cyan); font-weight: bold; font-size: 14px; }
  #header .run-id { color: var(--gray); }
  #header .uptime { color: var(--muted); }
  #header .clock { margin-left: auto; color: var(--gray); }
  #header .connection { font-size: 11px; padding: 2px 8px; border-radius: 4px; }
  #header .connection.live { background: rgba(94,200,112,0.15); color: var(--green); }
  #header .connection.connecting { background: rgba(224,192,112,0.15); color: var(--yellow); }
  #header .connection.dead { background: rgba(224,80,80,0.15); color: var(--red); }

  /* Stats bar */
  #statsbar {
    background: var(--bg2);
    border-bottom: 1px solid var(--border);
    padding: 4px 16px;
    display: flex;
    gap: 12px;
    font-size: 12px;
    flex-shrink: 0;
    flex-wrap: wrap;
  }
  .stat-group { display: flex; gap: 6px; align-items: center; }
  .stat-label { color: var(--muted); }
  .badge {
    padding: 1px 6px;
    border-radius: 3px;
    font-size: 11px;
    font-weight: 500;
  }

  /* Main layout */
  #main {
    display: grid;
    grid-template-columns: 1fr 1.8fr;
    grid-template-rows: 1fr 1fr;
    flex: 1;
    overflow: hidden;
    gap: 1px;
    background: var(--border);
  }
  #agents-pane { grid-row: 1 / 2; grid-column: 1 / 2; }
  #tasks-pane  { grid-row: 1 / 3; grid-column: 2 / 3; }
  #activity-pane { grid-row: 2 / 3; grid-column: 1 / 2; }

  .pane {
    background: var(--bg);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .pane-header {
    background: var(--bg2);
    border-bottom: 1px solid var(--border);
    padding: 5px 12px;
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    flex-shrink: 0;
  }
  .pane-body {
    flex: 1;
    overflow-y: auto;
    padding: 6px 0;
  }
  .pane-body::-webkit-scrollbar { width: 6px; }
  .pane-body::-webkit-scrollbar-track { background: transparent; }
  .pane-body::-webkit-scrollbar-thumb { background: var(--border); border-radius: 3px; }

  /* Agent rows */
  .agent-row {
    display: flex;
    align-items: center;
    padding: 3px 12px;
    gap: 8px;
    cursor: default;
  }
  .agent-row:hover { background: var(--bg3); }
  .agent-indent { color: var(--muted); flex-shrink: 0; }
  .agent-id { flex: 0 0 auto; min-width: 140px; }
  .agent-status { flex: 0 0 60px; font-size: 11px; }
  .agent-task { color: var(--muted); font-size: 11px; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .agent-action { color: var(--gray); font-size: 11px; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .agent-hb { flex: 0 0 auto; font-size: 11px; text-align: right; }

  /* Task table */
  .tasks-table { width: 100%; border-collapse: collapse; }
  .tasks-table th {
    position: sticky; top: 0; background: var(--bg2);
    padding: 5px 8px; text-align: left;
    font-size: 11px; color: var(--muted); font-weight: 500;
    border-bottom: 1px solid var(--border);
  }
  .tasks-table td {
    padding: 3px 8px; font-size: 12px;
    border-bottom: 1px solid rgba(42,42,42,0.4);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .tasks-table tr:hover td { background: var(--bg3); }
  .task-id { font-size: 11px; color: var(--muted); max-width: 100px; }
  .task-status-cell { width: 90px; }
  .task-assigned { width: 110px; font-size: 11px; color: var(--gray); }
  .task-title { max-width: 260px; }

  /* Activity stream */
  .activity-item {
    padding: 2px 12px;
    font-size: 11px;
    line-height: 1.5;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .activity-item:hover { background: var(--bg3); }
  .activity-ts { color: var(--muted); }
  .activity-icon { margin: 0 4px; }
  .activity-agent { color: var(--gray); }
  .activity-tool { margin-left: 4px; }
  .activity-args { color: var(--gray); margin-left: 4px; }
  .activity-msg { color: var(--cyan); }

  /* Status colors */
  .s-running { color: var(--green); }
  .s-idle { color: var(--yellow); }
  .s-done { color: var(--gray); }
  .s-failed { color: var(--red); }
  .s-stalled { color: var(--red); }

  .t-active { color: var(--green); }
  .t-pending { color: var(--yellow); }
  .t-blocked { color: var(--yellow); }
  .t-review { color: var(--yellow); }
  .t-approved { color: var(--green); }
  .t-queued { color: var(--orange); }
  .t-merged { color: var(--gray); }
  .t-absorbed { color: var(--muted); }
  .t-failed { color: var(--red); }
  .t-cancelled { color: var(--muted); }

  .tool-hive { color: var(--yellow); }
  .tool-std  { color: var(--gray); }
  .tool-bash { color: var(--blue); }

  /* Heartbeat colors */
  .hb-fresh  { color: var(--green); }
  .hb-ok     { color: var(--yellow); }
  .hb-stale  { color: var(--red); }

  /* Empty state */
  .empty { padding: 20px 12px; color: var(--muted); font-style: italic; }
</style>
</head>
<body>
<div id="header">
  <span class="logo">&#x2B21; HIVE</span>
  <span class="run-id" id="run-id">loading...</span>
  <span class="uptime" id="uptime"></span>
  <span id="conn" class="connection connecting">connecting</span>
  <span class="clock" id="clock"></span>
</div>
<div id="statsbar">
  <span class="stat-group">
    <span class="stat-label">Agents:</span>
    <span id="agent-stats"></span>
  </span>
  <span class="stat-group">
    <span class="stat-label">Tasks:</span>
    <span id="task-stats"></span>
  </span>
</div>
<div id="main">
  <div class="pane" id="agents-pane">
    <div class="pane-header">Swarm</div>
    <div class="pane-body" id="agents-body"></div>
  </div>
  <div class="pane" id="tasks-pane">
    <div class="pane-header">Tasks</div>
    <div class="pane-body" id="tasks-body">
      <table class="tasks-table">
        <thead>
          <tr><th>ID</th><th>Status</th><th>Assigned</th><th>Title</th></tr>
        </thead>
        <tbody id="tasks-tbody"></tbody>
      </table>
    </div>
  </div>
  <div class="pane" id="activity-pane">
    <div class="pane-header">Activity</div>
    <div class="pane-body" id="activity-body"></div>
  </div>
</div>

<script>
const MAX_ACTIVITY = 200;
let runStart = null;
let activityLog = [];
let autoScroll = true;

// Clock
setInterval(() => {
  document.getElementById('clock').textContent = new Date().toLocaleTimeString();
}, 1000);

// Uptime
setInterval(() => {
  if (!runStart) return;
  const secs = Math.floor((Date.now() - runStart) / 1000);
  const m = Math.floor(secs / 60), s = secs % 60;
  document.getElementById('uptime').textContent = `${m}m ${s}s`;
}, 1000);

// Activity auto-scroll detection
document.getElementById('activity-body').addEventListener('scroll', (e) => {
  const el = e.target;
  autoScroll = el.scrollTop + el.clientHeight >= el.scrollHeight - 20;
});

function statusClass(s, prefix) {
  return `${prefix}-${s.toLowerCase()}`;
}

function agentStatusAbbrev(s) {
  return {running:'RUN', idle:'IDLE', done:'DONE', failed:'FAIL', stalled:'STALL'}[s.toLowerCase()] || s.toUpperCase();
}

function hbClass(ageSecs) {
  if (ageSecs < 60) return 'hb-fresh';
  if (ageSecs < 180) return 'hb-ok';
  return 'hb-stale';
}

function formatAge(secs) {
  if (secs < 60) return `${secs}s`;
  return `${Math.floor(secs/60)}m${secs%60}s`;
}

function renderAgents(agents) {
  const body = document.getElementById('agents-body');
  if (!agents || agents.length === 0) {
    body.innerHTML = '<div class="empty">No agents</div>';
    return;
  }

  // Sort: coordinator first, then by parent, then by id
  const sorted = [...agents].sort((a, b) => {
    if (a.role === 'coordinator') return -1;
    if (b.role === 'coordinator') return 1;
    return a.id.localeCompare(b.id);
  });

  const now = Date.now();
  let html = '';
  for (const agent of sorted) {
    const dimmed = agent.status === 'done' || agent.status === 'failed';
    const isChild = agent.parent != null && agent.role !== 'coordinator';
    const indent = isChild ? '  └ ' : '';
    const sc = statusClass(agent.status, 's');

    let hbHtml = '';
    if (agent.heartbeat && agent.role !== 'coordinator') {
      const hbMs = new Date(agent.heartbeat).getTime();
      const ageSecs = Math.floor((now - hbMs) / 1000);
      const hc = dimmed ? 'hb-ok' : hbClass(ageSecs);
      hbHtml = `<span class="agent-hb ${hc}">${formatAge(ageSecs)}</span>`;
    }

    const taskHtml = agent.task_id
      ? `<span class="agent-task" title="${agent.task_id}">${agent.task_id.slice(0,10)}</span>`
      : '';

    html += `<div class="agent-row">
      <span class="agent-indent">${indent}</span>
      <span class="agent-id ${dimmed ? 's-done' : sc}">${agent.id}</span>
      <span class="agent-status ${sc}">${agentStatusAbbrev(agent.status)}</span>
      ${taskHtml}
      ${hbHtml}
    </div>`;
  }
  body.innerHTML = html;
}

function taskStatusBullet(s) {
  const map = {
    active: '▶ active', pending: '· pending', blocked: '⏸ blocked',
    review: '⧗ review', approved: '✓ approved', queued: '⇢ queued',
    merged: '✔ merged', absorbed: '· absorbed', failed: '✗ failed', cancelled: '· cancelled'
  };
  return map[s.toLowerCase()] || s;
}

function renderTasks(tasks) {
  const tbody = document.getElementById('tasks-tbody');
  if (!tasks || tasks.length === 0) {
    tbody.innerHTML = '<tr><td colspan="4" class="empty">No tasks</td></tr>';
    return;
  }

  let html = '';
  for (const task of tasks) {
    const sc = statusClass(task.status, 't');
    const assigned = task.assigned_to || '--';
    const indent = task.parent_task ? '  · ' : '';
    html += `<tr>
      <td class="task-id" title="${task.id}">${indent}${task.id.slice(0,10)}</td>
      <td class="task-status-cell"><span class="${sc}">${taskStatusBullet(task.status)}</span></td>
      <td class="task-assigned" title="${assigned}">${assigned.slice(0,14)}</td>
      <td class="task-title" title="${task.title}">${task.title}</td>
    </tr>`;
  }
  tbody.innerHTML = html;
}

function toolClass(toolName) {
  if (toolName.startsWith('hive_') || toolName.includes('__hive_')) return 'tool-hive';
  if (toolName === 'Bash' || toolName === '$') return 'tool-bash';
  return 'tool-std';
}

function renderActivity(activity) {
  const body = document.getElementById('activity-body');
  const wasAtBottom = autoScroll;

  let html = '';
  for (const item of activity) {
    const ts = new Date(item.timestamp).toLocaleTimeString();
    if (item.type === 'message') {
      html += `<div class="activity-item">
        <span class="activity-ts">${ts}</span>
        <span class="activity-icon activity-msg">▸</span>
        <span class="activity-msg">${item.from} → ${item.to}: ${escHtml(item.body.slice(0, 120))}</span>
      </div>`;
    } else {
      const tc = toolClass(item.tool_name);
      const icon = item.status === 'success' ? '✓' : '✗';
      const iconClass = item.status === 'success' ? tc : 'tool-std s-failed';
      const dur = item.duration_ms ? ` ${item.duration_ms}ms` : '';
      html += `<div class="activity-item">
        <span class="activity-ts">${ts}</span>
        <span class="activity-icon ${iconClass}">${icon}</span>
        <span class="activity-agent">${item.agent_id}</span>
        <span class="activity-tool ${tc}">${item.tool_name}</span>
        ${item.args_summary ? `<span class="activity-args">${escHtml(item.args_summary.slice(0, 60))}</span>` : ''}
        <span class="activity-args">${dur}</span>
      </div>`;
    }
  }
  body.innerHTML = html;

  if (wasAtBottom) {
    body.scrollTop = body.scrollHeight;
  }
}

function renderStats(agents, tasks) {
  const agentCounts = {};
  for (const a of (agents || [])) {
    agentCounts[a.status] = (agentCounts[a.status] || 0) + 1;
  }
  const taskCounts = {};
  for (const t of (tasks || [])) {
    taskCounts[t.status] = (taskCounts[t.status] || 0) + 1;
  }

  const agentOrder = ['running','idle','done','failed','stalled'];
  const taskOrder = ['active','pending','blocked','review','approved','queued','merged','absorbed','failed','cancelled'];

  document.getElementById('agent-stats').innerHTML = agentOrder
    .filter(s => agentCounts[s])
    .map(s => `<span class="badge ${statusClass(s,'s')}">${agentCounts[s]} ${s}</span>`)
    .join(' ');

  document.getElementById('task-stats').innerHTML = taskOrder
    .filter(s => taskCounts[s])
    .map(s => `<span class="badge ${statusClass(s,'t')}">${taskCounts[s]} ${s}</span>`)
    .join(' ');
}

function escHtml(str) {
  return str.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function applyState(state) {
  document.getElementById('run-id').textContent = state.run_id;
  if (state.run_start && !runStart) {
    runStart = new Date(state.run_start).getTime();
  }
  renderAgents(state.agents);
  renderTasks(state.tasks);
  renderActivity(state.activity);
  renderStats(state.agents, state.tasks);
}

// SSE connection
const conn = document.getElementById('conn');
function connect() {
  conn.className = 'connection connecting';
  conn.textContent = 'connecting';
  const es = new EventSource('/events');

  es.onopen = () => {
    conn.className = 'connection live';
    conn.textContent = 'live';
  };

  es.onmessage = (e) => {
    try {
      const state = JSON.parse(e.data);
      applyState(state);
    } catch (err) {
      console.error('Parse error:', err);
    }
  };

  es.onerror = () => {
    conn.className = 'connection dead';
    conn.textContent = 'reconnecting';
    es.close();
    setTimeout(connect, 3000);
  };
}

connect();
</script>
</body>
</html>
"#;

// ---------------------------------------------------------------------------
// State serialization
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct ActivityItem {
    #[serde(rename = "type")]
    kind: &'static str,
    timestamp: String,
    // message fields
    from: Option<String>,
    to: Option<String>,
    body: Option<String>,
    // tool call fields
    agent_id: Option<String>,
    tool_name: Option<String>,
    args_summary: Option<String>,
    status: Option<String>,
    duration_ms: Option<i64>,
}

#[derive(serde::Serialize)]
struct DashboardState {
    run_id: String,
    run_start: Option<String>,
    agents: Vec<Agent>,
    tasks: Vec<Task>,
    activity: Vec<ActivityItem>,
}

fn load_state(state: &HiveState, run_id: &str) -> DashboardState {
    let agents = state.list_agents(run_id).unwrap_or_default();
    let tasks = state.list_tasks(run_id).unwrap_or_default();
    let messages = state.list_messages(run_id).unwrap_or_default();

    let run_start = {
        let path = state.run_dir(run_id).join("run.json");
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["created_at"].as_str().map(|s| s.to_string()))
    };

    // Load tool calls from log.db
    let log_db = {
        let run_log_path = state.run_dir(run_id).join("log.db");
        let hive_log_path = state.hive_dir().join("log.db");
        rusqlite::Connection::open(&run_log_path)
            .ok()
            .or_else(|| rusqlite::Connection::open(&hive_log_path).ok())
    };

    let mut activity: Vec<ActivityItem> = messages
        .iter()
        .map(|m| ActivityItem {
            kind: "message",
            timestamp: m.timestamp.to_rfc3339(),
            from: Some(m.from.clone()),
            to: Some(m.to.clone()),
            body: Some(m.body.clone()),
            agent_id: None,
            tool_name: None,
            args_summary: None,
            status: None,
            duration_ms: None,
        })
        .collect();

    if let Some(conn) = log_db {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT timestamp, agent_id, tool_name, args_summary, status, duration_ms \
             FROM tool_calls WHERE run_id = ?1 ORDER BY timestamp DESC LIMIT 300",
        ) {
            let rows = stmt.query_map(rusqlite::params![run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            });
            if let Ok(rows) = rows {
                for row in rows.filter_map(|r| r.ok()) {
                    activity.push(ActivityItem {
                        kind: "tool",
                        timestamp: row.0,
                        from: None,
                        to: None,
                        body: None,
                        agent_id: Some(row.1),
                        tool_name: Some(row.2),
                        args_summary: row.3,
                        status: Some(row.4),
                        duration_ms: row.5,
                    });
                }
            }
        }
    }

    // Sort by timestamp, keep last 200
    activity.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    if activity.len() > 200 {
        activity.drain(0..activity.len() - 200);
    }

    DashboardState {
        run_id: run_id.to_string(),
        run_start,
        agents,
        tasks,
        activity,
    }
}

// ---------------------------------------------------------------------------
// Minimal HTTP server
// ---------------------------------------------------------------------------

async fn handle_connection(
    mut stream: TcpStream,
    state: Arc<HiveState>,
    run_id: Arc<String>,
) {
    // Read request line + headers (stop at blank line)
    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await.is_err() {
        return;
    }

    // Consume rest of headers
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                if line == "\r\n" || line == "\n" {
                    break;
                }
            }
        }
    }

    // Parse path from "GET /path HTTP/1.1"
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();

    // Drop reader to regain ownership of stream
    drop(reader);

    match path.as_str() {
        "/" | "/index.html" => {
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                DASHBOARD_HTML.len(),
                DASHBOARD_HTML
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }

        "/api/state" => {
            let ds = load_state(&state, &run_id);
            let json = serde_json::to_string(&ds).unwrap_or_else(|_| "{}".to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{}",
                json.len(),
                json
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }

        "/events" => {
            // SSE: keep connection open, push state every second
            let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\nConnection: keep-alive\r\n\r\n";
            if stream.write_all(headers.as_bytes()).await.is_err() {
                return;
            }

            loop {
                let ds = load_state(&state, &run_id);
                match serde_json::to_string(&ds) {
                    Ok(json) => {
                        let event = format!("data: {json}\n\n");
                        if stream.write_all(event.as_bytes()).await.is_err() {
                            break; // client disconnected
                        }
                    }
                    Err(_) => break,
                }
                sleep(Duration::from_secs(1)).await;
            }
        }

        _ => {
            let body = "404 Not Found";
            let response = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run_web(port: u16) -> Result<(), String> {
    let state = HiveState::discover()?;
    let run_id = state.active_run_id()?;

    let url = format!("http://localhost:{port}");
    println!("Hive web dashboard: {url}");
    println!("Press Ctrl+C to stop.");

    // Try to open browser
    let _ = std::process::Command::new("open").arg(&url).spawn();

    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let state = Arc::new(state);
        let run_id = Arc::new(run_id);

        let addr = format!("127.0.0.1:{port}");
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("Failed to bind {addr}: {e}"))?;

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let state = Arc::clone(&state);
                    let run_id = Arc::clone(&run_id);
                    tokio::spawn(async move {
                        handle_connection(stream, state, run_id).await;
                    });
                }
                Err(e) => {
                    eprintln!("Accept error: {e}");
                }
            }
        }
    })
}
