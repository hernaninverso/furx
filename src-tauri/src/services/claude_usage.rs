// F5 — Cost/quota strip: read ~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl
// stream events and sum the per-message `usage` object that Claude Code emits.
// Falls back gracefully (zero counts) when no project dir exists.
//
// 2026-05-28 fix: original implementation looked for a `usage.json` file inside a
// `<sessionId>/` subdirectory — that file shape doesn't exist in Claude Code 2.x.
// Real events live in `<sessionId>.jsonl` siblings at the project level, with
// each line a JSON object where `message.usage = { input_tokens, output_tokens,
// cache_creation_input_tokens, cache_read_input_tokens }`. We now stream and sum.

use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct UsageSummary {
    pub source_files: usize,
    pub total_tokens: u64,
    /// Tokens whose `updated_at` is within last 24h (UTC).
    pub burn_24h_tokens: u64,
    pub burn_7d_tokens: u64,
    /// Tokens grouped by model name.
    pub by_model: Vec<ModelTokens>,
    pub by_session: Vec<SessionUsage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelTokens {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionUsage {
    pub session_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model: Option<String>,
    pub updated_at: Option<String>,
}

/// Walk ~/.claude/projects/ for `usage.json` files. Robust to absent
/// directories. Caps at 50 files scanned.
pub fn summary() -> UsageSummary {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return empty(),
    };
    let root = home.join(".claude").join("projects");
    if !root.exists() {
        return empty();
    }
    let mut out = Vec::new();
    let mut scanned = 0usize;
    walk(&root, &mut out, &mut scanned, 4, 50);
    let total = out.iter().map(|s| s.input_tokens + s.output_tokens).sum();
    let now = chrono::Utc::now();
    let burn_24h = out
        .iter()
        .filter(|s| within_hours(s.updated_at.as_deref(), now, 24))
        .map(|s| s.input_tokens + s.output_tokens)
        .sum();
    let burn_7d = out
        .iter()
        .filter(|s| within_hours(s.updated_at.as_deref(), now, 24 * 7))
        .map(|s| s.input_tokens + s.output_tokens)
        .sum();
    let mut by_model_map: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for s in &out {
        let model = s.model.clone().unwrap_or_else(|| "unknown".into());
        let entry = by_model_map.entry(model).or_insert((0, 0));
        entry.0 += s.input_tokens;
        entry.1 += s.output_tokens;
    }
    let by_model = by_model_map
        .into_iter()
        .map(|(model, (i, o))| ModelTokens {
            model,
            input_tokens: i,
            output_tokens: o,
        })
        .collect();
    UsageSummary {
        source_files: out.len(),
        total_tokens: total,
        burn_24h_tokens: burn_24h,
        burn_7d_tokens: burn_7d,
        by_model,
        by_session: out,
    }
}

fn within_hours(ts: Option<&str>, now: chrono::DateTime<chrono::Utc>, hours: i64) -> bool {
    let Some(s) = ts else {
        return false;
    };
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) else {
        return false;
    };
    (now - t.with_timezone(&chrono::Utc)).num_hours() < hours
}

/// Parse a single `<sessionId>.jsonl` file, summing tokens across all events
/// that carry a `message.usage` block. Returns None if the file is empty or
/// has no usage events. Cheap (stream-read, ignores parse errors per line).
fn read_session_jsonl(path: &std::path::Path) -> Option<SessionUsage> {
    let f = std::fs::File::open(path).ok()?;
    let rdr = BufReader::new(f);
    let mut input = 0u64;
    let mut output = 0u64;
    let mut cache_create = 0u64;
    let mut cache_read = 0u64;
    let mut model: Option<String> = None;
    let mut last_ts: Option<String> = None;
    // 053 fix (audit-3 Codex) — Claude Code 2.x escribe VARIOS eventos JSONL con el MISMO `message.id`
    // y el MISMO bloque `usage` (en un .jsonl real: 20 líneas usage = 7 mensajes únicos). Sumar cada
    // línea contaba input/output/cache_creation 2-3 veces. Deduplicamos por `message.id`: solo el
    // primer evento de cada mensaje aporta sus tokens.
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    for line in rdr.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        // Tauri client (Claude Code) writes events with `message` object containing usage.
        if let Some(msg) = v.get("message").and_then(|m| m.as_object()) {
            // Dedup por message.id: si ya contamos este mensaje, saltear su usage (pero igual
            // actualizar model/timestamp abajo). Sin id (formatos viejos) → se cuenta (no dedup).
            let dup = msg
                .get("id")
                .and_then(|x| x.as_str())
                .map(|id| !seen_ids.insert(id.to_string()))
                .unwrap_or(false);
            if !dup {
            if let Some(u) = msg.get("usage").and_then(|x| x.as_object()) {
                input += u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                output += u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                cache_create += u
                    .get("cache_creation_input_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                cache_read += u
                    .get("cache_read_input_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
            }
            if model.is_none() {
                if let Some(m) = msg.get("model").and_then(|x| x.as_str()) {
                    model = Some(m.to_string());
                }
            }
            } // cierra `if !dup` (053 — dedup por message.id)
        }
        // The event itself usually has `timestamp` (ISO8601). Use the latest seen.
        if let Some(t) = v.get("timestamp").and_then(|x| x.as_str()) {
            last_ts = Some(t.to_string());
        }
    }

    if input + output + cache_create + cache_read == 0 {
        return None;
    }
    Some(SessionUsage {
        session_id,
        // "input_tokens" = input fresco + cache creation (tokens NUEVOS procesados en la sesión).
        // NO sumamos `cache_read`: es el prefijo cacheado RE-LEÍDO en CADA turno (crece
        // cumulativamente), así que sumarlo a lo largo de N turnos cuenta el contexto temprano N
        // veces → inflaba groseramente el total. El cache_read no es trabajo nuevo (es re-lectura del
        // mismo contexto, a 1/10 del precio). `cache_read` queda acumulado por si se quiere mostrar
        // aparte, pero fuera del total. `output_tokens` se suma normal (cada turno es nuevo).
        input_tokens: input + cache_create,
        output_tokens: output,
        model,
        updated_at: last_ts,
    })
}

fn empty() -> UsageSummary {
    UsageSummary {
        source_files: 0,
        total_tokens: 0,
        burn_24h_tokens: 0,
        burn_7d_tokens: 0,
        by_model: vec![],
        by_session: Vec::new(),
    }
}

fn walk(
    dir: &PathBuf,
    out: &mut Vec<SessionUsage>,
    scanned: &mut usize,
    depth_left: usize,
    cap: usize,
) {
    if depth_left == 0 || out.len() >= cap || *scanned >= 5000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // Belt-and-suspenders: recursive walk() inside the loop can grow `out`
        // past `cap` while we're mid-iteration; check after each iteration too.
        if out.len() >= cap {
            return;
        }
        let path = entry.path();
        *scanned += 1;
        if path.is_dir() {
            walk(&path, out, scanned, depth_left - 1, cap);
            continue;
        }
        // 2026-05-28 fix: Claude Code 2.x writes per-session `<sessionId>.jsonl`
        // streams of events. Each event line is a JSON object; usage data lives
        // under `message.usage`. We sum across the whole file.
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            if let Some(session) = read_session_jsonl(&path) {
                out.push(session);
                if out.len() >= cap {
                    return;
                }
            }
        }
    }
}

/// BLOQUE E · F5 — per-pane lookup: given a pane's cwd, find the most recent
/// ~/.claude/projects/<encoded-cwd>/<session>/usage.json and report its
/// totals. Claude CLI encodes the project path by replacing every `/` with `-`,
/// so e.g. `/Users/dev/furx` → `-Users-hernan-furx`. Returns None when no
/// matching session has been recorded yet.
#[derive(Debug, Clone, Serialize)]
pub struct PaneUsage {
    pub session_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub model: Option<String>,
    pub updated_at: Option<String>,
}

pub fn for_cwd(cwd: &str) -> Option<PaneUsage> {
    let home = dirs::home_dir()?;
    let encoded = cwd.replace('/', "-");
    let project_dir = home.join(".claude").join("projects").join(&encoded);
    if !project_dir.is_dir() {
        return None;
    }

    // 2026-05-28 fix: scan the newest <sessionId>.jsonl in the project dir and
    // sum its message.usage events. Claude Code 2.x replaced the prior
    // `<sessionId>/usage.json` shape with stream events at the project root.
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries = std::fs::read_dir(&project_dir).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        let modtime = p.metadata().and_then(|m| m.modified()).ok()?;
        if best.as_ref().map(|(t, _)| modtime > *t).unwrap_or(true) {
            best = Some((modtime, p));
        }
    }
    let (_mt, jsonl_path) = best?;
    let session = read_session_jsonl(&jsonl_path)?;
    Some(PaneUsage {
        session_id: session.session_id,
        input_tokens: session.input_tokens,
        output_tokens: session.output_tokens,
        total_tokens: session.input_tokens + session.output_tokens,
        model: session.model,
        updated_at: session.updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_dir_returns_empty() {
        // ~/.claude/projects may or may not exist depending on machine; just verify no panic.
        let s = summary();
        assert!(s.by_session.len() <= 50);
    }

    #[test]
    fn for_cwd_returns_none_for_unknown_cwd() {
        let r = for_cwd("/nonexistent-furx-test-path-2026");
        assert!(r.is_none());
    }

    // 053 fix — el total NO debe sumar `cache_read_input_tokens` cumulativo (el prefijo cacheado se
    // re-lee cada turno y crece; sumarlo contaba el contexto N veces → total groseramente inflado).
    #[test]
    fn token_count_excludes_cumulative_cache_read() {
        // 3 turnos: input fresco 100 c/u, output 50 c/u, cache_creation 200 c/u, y cache_read que
        // CRECE (1000, 2000, 3000 = el contexto re-leído). Sin el fix, total inflaría con 6000 de
        // cache_read. Con el fix: input(300) + cache_create(600) = 900 input, output 150.
        let dir = std::env::temp_dir().join(format!("furx-usage-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        // 3 mensajes ÚNICOS (m1/m2/m3), cada uno repetido 2 veces con el MISMO message.id (Claude
        // Code 2.x escribe varios eventos por mensaje). La dedup por id debe contar cada uno UNA vez.
        let mut body = String::new();
        for (i, cr) in [1000u64, 2000, 3000].iter().enumerate() {
            let line = format!(
                "{{\"message\":{{\"id\":\"m{i}\",\"usage\":{{\"input_tokens\":100,\"output_tokens\":50,\"cache_creation_input_tokens\":200,\"cache_read_input_tokens\":{cr}}},\"model\":\"claude-test\"}},\"timestamp\":\"2026-06-05T10:00:0{i}Z\"}}\n"
            );
            body.push_str(&line);
            body.push_str(&line); // evento DUPLICADO del mismo message.id
        }
        std::fs::write(&path, body).unwrap();
        let u = read_session_jsonl(&path).expect("parsea");
        // 3 mensajes únicos: input fresh(300) + cache_create(600) = 900, SIN cache_read, SIN duplicar.
        assert_eq!(u.input_tokens, 900, "dedup por id + sin cache_read");
        assert_eq!(u.output_tokens, 150, "3 mensajes únicos × 50, no 6 × 50");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
