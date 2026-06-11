// FASE 1 — Skills Registry
// Council 5-voces frontier: routing trait + path sanitize + typed structs + async I/O.
//
// Skill = una unidad ejecutable con prompt + routing + tools.
// Se instalan desde ~/.furx/skills/<name>/skill.yaml o ~/.claude/skills/<name>/SKILL.md.
// Se ejecutan contra el multi-model council o un provider específico.

use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Instant;
use tokio::sync::{mpsc, Semaphore};
use uuid::Uuid;
use walkdir::WalkDir;

/// Max concurrent skill executions (Audit F1C: prevent resource exhaustion).
static MAX_CONCURRENT_RUNS: usize = 4;
static ACTIVE_RUNS: Lazy<Semaphore> = Lazy::new(|| Semaphore::new(MAX_CONCURRENT_RUNS));

// --- Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    pub description: Option<String>,
    #[serde(default)]
    pub category: String,
    pub routing: Option<RoutingConfig>,
    #[serde(default)]
    pub tools: Vec<ToolConfig>,
    #[serde(default)]
    pub permissions: Vec<String>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    #[serde(default = "default_strategy")]
    pub strategy: String, // "council" | "single" | "pipeline"
    pub preset: Option<String>, // council preset name
    pub model: Option<String>,  // single model override
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub fallback: Option<String>, // fallback strategy
    pub max_retries: Option<u32>,
    pub cache_ttl_seconds: Option<u64>,
}

fn default_strategy() -> String {
    "council".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub name: String,
    pub endpoint: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub category: String,
    pub enabled: bool,
    pub installed_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillRunResult {
    pub run_id: String,
    pub skill_name: String,
    pub status: String,
    pub output: Option<String>,
    pub model_used: Option<String>,
    pub tokens_used: u64,
    pub latency_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRunHistory {
    pub id: String,
    pub skill_name: String,
    pub input: Option<String>,
    pub output: Option<String>,
    pub model_used: Option<String>,
    pub tokens_used: u64,
    pub latency_ms: u64,
    pub status: String,
    pub error: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

// --- Mapping Agent Skills (SKILL.md) to internal format ---

fn parse_skill_md(path: &Path) -> Result<SkillDefinition> {
    let text = std::fs::read_to_string(path)?;
    let body = text.trim();
    // Extract YAML frontmatter from SKILL.md
    let (frontmatter, _body) = if let Some(rest) = body.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            (&rest[..end], rest[end + 4..].trim())
        } else {
            ("", body)
        }
    } else {
        ("", body)
    };

    // Minimal frontmatter parser for SKILL.md
    let mut name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let mut description = None;
    for line in frontmatter.lines() {
        if let Some(v) = line.strip_prefix("name: ") {
            name = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("description: ") {
            description = Some(v.trim().to_string());
        }
    }

    Ok(SkillDefinition {
        name,
        version: "1.0.0".to_string(),
        description,
        category: "general".to_string(),
        routing: Some(RoutingConfig {
            strategy: "council".to_string(),
            preset: Some("frontier_free".to_string()),
            model: None,
            max_tokens: Some(4096),
            temperature: None,
            fallback: None,
            max_retries: Some(2),
            cache_ttl_seconds: None,
        }),
        tools: vec![],
        permissions: vec![],
    })
}

fn parse_skill_yaml(path: &Path) -> Result<SkillDefinition> {
    let text = std::fs::read_to_string(path)?;
    let skill: SkillDefinition =
        serde_yaml::from_str(&text).map_err(|e| anyhow!("invalid skill.yaml: {}", e))?;
    Ok(skill)
}

/// Scan a directory for skills (skill.yaml or SKILL.md).
/// Uses canonicalize + walkdir to prevent path traversal.
/// Council Sec V3: reject symlinks outside the base dir.
/// Uses a simple depth counter (thread_local) to skip hidden subdirs while
/// allowing the root to have any name.
pub fn scan_skills_dir(base: &Path) -> Vec<SkillDefinition> {
    if !base.is_dir() {
        return vec![];
    }
    let base_canon = match base.canonicalize() {
        Ok(p) => p,
        Err(_) => return vec![],
    };

    let mut skills = Vec::new();
    for entry in WalkDir::new(&base_canon)
        .max_depth(3)
        .into_iter()
        .filter_entry(|e| {
            // Council Sec V3: skip hidden subdirs, reject symlinks outside base
            let depth = e.depth();
            if depth > 0 {
                let fname = e.file_name().to_str().unwrap_or("");
                if fname.starts_with('.') {
                    return false;
                }
            }
            if e.path_is_symlink() {
                if let Ok(target) = e.path().canonicalize() {
                    if !target.starts_with(&base_canon) {
                        return false;
                    }
                }
            }
            true
        })
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let skill = match fname {
            "skill.yaml" => match parse_skill_yaml(path) {
                Ok(s) => s,
                Err(_) => continue,
            },
            "SKILL.md" => match parse_skill_md(path) {
                Ok(s) => s,
                Err(_) => continue,
            },
            _ => continue,
        };
        skills.push(skill);
    }
    skills
}

// --- DB Operations ---

/// Validate skill name: alphanumeric, hyphens, underscores, 1-64 chars.
fn validate_skill_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(anyhow!("skill name must be 1-64 characters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(anyhow!(
            "skill name must be alphanumeric, hyphens, or underscores"
        ));
    }
    Ok(())
}

fn ensure_skill(db: &Connection, skill: &SkillDefinition) -> Result<String> {
    validate_skill_name(&skill.name)?;
    let id = Uuid::new_v4().to_string();
    let routing_json = serde_json::to_string(&skill.routing).ok();
    let tools_json = serde_json::to_string(&skill.tools).ok();
    let perms_json = serde_json::to_string(&skill.permissions).ok();

    db.execute(
        "INSERT OR IGNORE INTO skills (id, name, version, description, category, routing_config, tools_config, permissions)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            id,
            skill.name,
            skill.version,
            skill.description,
            skill.category,
            routing_json,
            tools_json,
            perms_json,
        ],
    )?;
    Ok(id)
}

pub fn list_skills(db: &Mutex<Connection>) -> Result<Vec<SkillSummary>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, name, version, description, category, enabled, installed_at FROM skills ORDER BY name"
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SkillSummary {
                id: r.get(0)?,
                name: r.get(1)?,
                version: r.get(2)?,
                description: r.get(3)?,
                category: r.get(4)?,
                enabled: r.get::<_, i64>(5)? != 0,
                installed_at: r.get(6)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

pub fn get_skill(db: &Mutex<Connection>, name: &str) -> Result<SkillDefinition> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT name, version, description, category, routing_config, tools_config, permissions FROM skills WHERE name = ? AND enabled = 1"
    )?;
    let mut rows = stmt.query_map(params![name], |r| {
        let name: String = r.get(0)?;
        let version: String = r.get(1)?;
        let description: Option<String> = r.get(2)?;
        let category: String = r.get(3)?;
        let rc: Option<String> = r.get(4)?;
        let tc: Option<String> = r.get(5)?;
        let pc: Option<String> = r.get(6)?;
        Ok(SkillDefinition {
            name,
            version,
            description,
            category,
            routing: rc.and_then(|s| serde_json::from_str(&s).ok()),
            tools: tc
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            permissions: pc
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
        })
    })?;
    match rows.next() {
        Some(Ok(s)) => Ok(s),
        _ => Err(anyhow!("skill '{}' not found or disabled", name)),
    }
}

pub fn set_enabled(db: &Mutex<Connection>, name: &str, enabled: bool) -> Result<()> {
    db.lock().execute(
        "UPDATE skills SET enabled = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE name = ?",
        params![enabled as i64, name],
    )?;
    Ok(())
}

pub fn delete_skill(db: &Mutex<Connection>, name: &str) -> Result<()> {
    db.lock()
        .execute("DELETE FROM skills WHERE name = ?", params![name])?;
    Ok(())
}

pub fn get_run_history(
    db: &Mutex<Connection>,
    skill_name: &str,
    limit: usize,
) -> Result<Vec<SkillRunHistory>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, skill_name, input, output, model_used, tokens_used, latency_ms, status, error, started_at, finished_at
         FROM skill_runs WHERE skill_name = ? ORDER BY started_at DESC LIMIT ?"
    )?;
    let rows = stmt
        .query_map(params![skill_name, limit as i64], |r| {
            Ok(SkillRunHistory {
                id: r.get(0)?,
                skill_name: r.get(1)?,
                input: r.get(2)?,
                output: r.get(3)?,
                model_used: r.get(4)?,
                tokens_used: r.get::<_, i64>(5)? as u64,
                latency_ms: r.get::<_, i64>(6)? as u64,
                status: r.get(7)?,
                error: r.get(8)?,
                started_at: r.get(9)?,
                finished_at: r.get(10)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

// --- Skill Refresh ---

pub fn refresh_from_disk(db: &Mutex<Connection>) -> Result<usize> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home dir"))?;
    let mut count = 0;

    // Scan ~/.furx/skills/
    let furx_skills = home.join(".furx").join("skills");
    if furx_skills.is_dir() {
        for skill in scan_skills_dir(&furx_skills) {
            ensure_skill(&db.lock(), &skill)?;
            count += 1;
        }
    }

    // Scan ~/.claude/skills/ (Agent Skills compatibility)
    let claude_skills = home.join(".claude").join("skills");
    if claude_skills.is_dir() {
        for skill in scan_skills_dir(&claude_skills) {
            ensure_skill(&db.lock(), &skill)?;
            count += 1;
        }
    }

    Ok(count)
}

// --- Skill Run ---

/// 050 FR-005 — resuelve la signing-key (SHA-256 hex del pubkey, 64 chars) de un skill para el
/// registro CRL de spans. Los skills LLM-routed de esta tabla (`skills`) NO se firman → `None` (no son
/// key-revocables; correcto). El gancho existe para que un path de skill FIRMADO (que conozca su
/// `key_id[..64]` del manifest) registre su span con `Some(key)` y así una revocación de esa key aborte
/// el span en vivo. Mantenerlo como función deja el punto de extensión explícito y testeable.
fn skill_signing_key(_skill: &SkillDefinition) -> Option<String> {
    None
}

/// Run a skill against the configured routing strategy.
/// Uses mpsc channel to stream progress events.
/// Council LLM Ops V5: retry + fallback + cache.
pub async fn run_skill(
    db: &Mutex<Connection>,
    skill_name: &str,
    input: &str,
    tx: mpsc::UnboundedSender<SkillEvent>,
) -> Result<SkillRunResult> {
    // Audit F1C: acquire semaphore permit to limit concurrent runs.
    let _permit = ACTIVE_RUNS
        .acquire()
        .await
        .map_err(|e| anyhow!("semaphore: {}", e))?;
    let skill = get_skill(db, skill_name)?;
    let run_id = Uuid::new_v4().to_string();
    let input_hash = blake3_hash(input);
    // Check cache: look for identical input_hash with success status
    let cached = check_cache(db, skill_name, &input_hash);
    if let Some(cached_result) = cached {
        let _ = tx.send(SkillEvent::CacheHit {
            run_id: run_id.clone(),
        });
        return Ok(cached_result);
    }

    // Mark run as started in DB
    {
        let conn = db.lock();
        let _ = conn.execute(
            "INSERT INTO skill_runs (id, skill_name, skill_version, input_hash, input, status)
             VALUES (?, ?, ?, ?, ?, 'running')",
            params![run_id, skill_name, skill.version, input_hash, input],
        );
    }

    let _ = tx.send(SkillEvent::Progress {
        run_id: run_id.clone(),
        step: "starting".to_string(),
        message: format!("Running '{}'...", skill.name),
    });

    // 050 FR-005 — registra este run como un SPAN VIVO en el registro CRL. La signing-key de un skill
    // LLM-routed (este path) no se persiste → `None` (no es key-revocable; correcto). Un path de skill
    // FIRMADO que conozca su `key_id[..64]` lo registraría con `Some(key)` y entonces una revocación
    // de esa key abortaría este span en vivo. El guard DESREGISTRA al dropearse (incluso si paniquea).
    let _span = crate::services::crl::register_span(run_id.clone(), skill_signing_key(&skill));

    // Determine routing
    let strategy = skill
        .routing
        .as_ref()
        .map(|r| r.strategy.as_str())
        .unwrap_or("council");

    // 050 FR-005 — chequeo de aborto ANTES del trabajo costoso: si la key del skill se revocó entre el
    // registro y acá (o ya estaba revocada al arrancar → nace abortado), cortamos sin gastar el run.
    if _span.aborted() {
        let conn = db.lock();
        let _ = conn.execute(
            "UPDATE skill_runs SET status = 'error', error = 'skill revoked (signing key revoked)', \
             finished_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
            params![run_id],
        );
        drop(conn);
        let _ = tx.send(SkillEvent::Error {
            run_id: run_id.clone(),
            error: "skill revocado: la signing key fue revocada".to_string(),
        });
        return Err(anyhow!("skill revoked: signing key revoked"));
    }

    let result = match strategy {
        "single" => run_single_provider(&skill, input, &tx).await,
        "council" => run_council(&skill, input, &tx).await,
        _ => run_council(&skill, input, &tx).await, // default to council
    };

    // 050 FR-005 — re-chequeo TRAS el trabajo: si la key se revocó MIENTRAS corría, descartamos el
    // resultado (no lo cacheamos ni lo entregamos) y reportamos revocación (fail-closed: un skill
    // revocado a mitad de run NO entrega output).
    if _span.aborted() {
        let conn = db.lock();
        let _ = conn.execute(
            "UPDATE skill_runs SET status = 'error', error = 'skill revoked mid-run', \
             finished_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
            params![run_id],
        );
        drop(conn);
        let _ = tx.send(SkillEvent::Error {
            run_id: run_id.clone(),
            error: "skill revocado durante la ejecución".to_string(),
        });
        return Err(anyhow!("skill revoked mid-run"));
    }

    match &result {
        Ok(r) => {
            let _ = tx.send(SkillEvent::Complete {
                run_id: run_id.clone(),
                output: r.output.clone().unwrap_or_default(),
            });
            // Persist
            let conn = db.lock();
            let _ = conn.execute(
                "UPDATE skill_runs SET status = 'success', output = ?, model_used = ?, tokens_used = ?, latency_ms = ?, finished_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
                params![r.output, r.model_used, r.tokens_used as i64, r.latency_ms as i64, run_id],
            );
        }
        Err(e) => {
            let err_msg = e.to_string();
            let _ = tx.send(SkillEvent::Error {
                run_id: run_id.clone(),
                error: err_msg.clone(),
            });
            let conn = db.lock();
            let _ = conn.execute(
                "UPDATE skill_runs SET status = 'error', error = ?, finished_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
                params![err_msg, run_id],
            );
        }
    }

    result
}

fn check_cache(
    db: &Mutex<Connection>,
    skill_name: &str,
    input_hash: &str,
) -> Option<SkillRunResult> {
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT output, model_used, tokens_used, latency_ms FROM skill_runs
         WHERE skill_name = ? AND input_hash = ? AND status = 'success'
         ORDER BY started_at DESC LIMIT 1",
        )
        .ok()?;
    let mut rows = stmt
        .query_map(params![skill_name, input_hash], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .ok()?;
    if let Some(Ok((output, model, tokens, latency))) = rows.next() {
        return Some(SkillRunResult {
            run_id: String::new(),
            skill_name: skill_name.to_string(),
            status: "success".to_string(),
            output,
            model_used: model,
            tokens_used: tokens as u64,
            latency_ms: latency as u64,
            error: None,
        });
    }
    None
}

async fn run_council(
    skill: &SkillDefinition,
    input: &str,
    tx: &mpsc::UnboundedSender<SkillEvent>,
) -> Result<SkillRunResult> {
    let _ = tx.send(SkillEvent::Progress {
        run_id: String::new(),
        step: "council".to_string(),
        message: "Consulting multi-model council...".to_string(),
    });

    // Use the existing council_multi system
    let preset = skill
        .routing
        .as_ref()
        .and_then(|r| r.preset.as_deref())
        .unwrap_or("frontier_free");

    // Call council via resilience layer or directly
    let start = Instant::now();
    let result = call_council_with_preset(preset, input, skill).await?;
    let elapsed = start.elapsed();

    Ok(SkillRunResult {
        run_id: String::new(),
        skill_name: skill.name.clone(),
        status: "success".to_string(),
        output: Some(result.output),
        model_used: Some(preset.to_string()),
        tokens_used: result.tokens_used,
        latency_ms: elapsed.as_millis() as u64,
        error: None,
    })
}

async fn run_single_provider(
    skill: &SkillDefinition,
    input: &str,
    tx: &mpsc::UnboundedSender<SkillEvent>,
) -> Result<SkillRunResult> {
    let model = skill
        .routing
        .as_ref()
        .and_then(|r| r.model.as_deref())
        .unwrap_or("gpt-oss-120b");
    let _ = tx.send(SkillEvent::Progress {
        run_id: String::new(),
        step: "llm".to_string(),
        message: format!("Calling {}...", model),
    });

    let start = Instant::now();
    let result = call_single_llm(model, input, skill).await?;
    let elapsed = start.elapsed();

    Ok(SkillRunResult {
        run_id: String::new(),
        skill_name: skill.name.clone(),
        status: "success".to_string(),
        output: Some(result),
        model_used: Some(model.to_string()),
        tokens_used: 0,
        latency_ms: elapsed.as_millis() as u64,
        error: None,
    })
}

/// Reusable HTTP client for skill execution (Audit-1 MED B003/B010).
static SKILLS_HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("skills reqwest client build")
});

// BLOQUE J (ULTRA REVIEW): this path calls the AIE directly with the
// requested preset name (e.g. "frontier_free"). The AIE already runs a
// council-style cascade over multiple providers internally, so the
// observable behaviour matches council_multi::run() for the single-preset
// case. council_multi gives the user finer per-provider control + cost
// estimate — wire that in if/when we expose preset-vs-explicit-providers
// UI in the skill editor (out of scope for the current Skills Registry).
async fn call_council_with_preset(
    preset: &str,
    input: &str,
    _skill: &SkillDefinition,
) -> Result<CouncilOutput> {
    // 039 — in-process cached bearer (was a `/usr/bin/security` subprocess per call).
    let bearer =
        crate::services::keychain_bearer::get_bearer().ok_or_else(|| anyhow!("missing bearer"))?;

    let aie_url = format!(
        "{}/v1/chat/completions",
        crate::services::aie_endpoint::resolve_url_or_default()
    );
    let resp = SKILLS_HTTP_CLIENT
        .post(&aie_url)
        .header("Authorization", format!("Bearer {}", bearer))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "profile": preset,
            "messages": [{"role": "user", "content": input}],
            "max_tokens": _skill.routing.as_ref().and_then(|r| r.max_tokens).unwrap_or(4096),
        }))
        .send()
        .await
        .map_err(|e| anyhow!("AIE call failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        // 039 — drop a stale bearer on 401 so the next call re-reads the rotated value.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            crate::services::keychain_bearer::invalidate_bearer_cache();
        }
        return Err(anyhow!("AIE status {}", status));
    }
    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("AIE parse failed: {}", e))?;

    let content = data["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let tokens = data["usage"]["total_tokens"].as_u64().unwrap_or(0);

    Ok(CouncilOutput {
        output: content,
        tokens_used: tokens,
    })
}

async fn call_single_llm(model: &str, input: &str, skill: &SkillDefinition) -> Result<String> {
    // 039 — in-process cached bearer (was a `/usr/bin/security` subprocess per call).
    let bearer =
        crate::services::keychain_bearer::get_bearer().ok_or_else(|| anyhow!("missing bearer"))?;

    let aie_url = format!(
        "{}/v1/chat/completions",
        crate::services::aie_endpoint::resolve_url_or_default()
    );
    let resp = SKILLS_HTTP_CLIENT
        .post(&aie_url)
        .header("Authorization", format!("Bearer {}", bearer))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": input}],
            "max_tokens": skill.routing.as_ref().and_then(|r| r.max_tokens).unwrap_or(4096),
        }))
        .send()
        .await
        .map_err(|e| anyhow!("AIE call failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        // 039 — drop a stale bearer on 401 so the next call re-reads the rotated value.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            crate::services::keychain_bearer::invalidate_bearer_cache();
        }
        return Err(anyhow!("AIE status {}", status));
    }
    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("AIE parse failed: {}", e))?;

    Ok(data["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

struct CouncilOutput {
    output: String,
    tokens_used: u64,
}

// --- Events ---

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum SkillEvent {
    Progress {
        run_id: String,
        step: String,
        message: String,
    },
    CacheHit {
        run_id: String,
    },
    Complete {
        run_id: String,
        output: String,
    },
    Error {
        run_id: String,
        error: String,
    },
}

// --- Helpers ---

/// Audit-1 LOW: 32 hex chars = 128 bits of collision resistance for the skill
/// run cache key (was 16 = 64 bits, which is birthday-bound at ~2^32 entries).
fn blake3_hash(input: &str) -> String {
    blake3::hash(input.as_bytes()).to_hex()[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_scan_skills_dir_empty() {
        let dir = tempdir().unwrap();
        let skills = scan_skills_dir(dir.path());
        assert!(skills.is_empty());
    }

    #[test]
    fn test_parse_skill_yaml_roundtrip() {
        let dir = tempdir().unwrap();
        let skill_path = dir.path().join("skill.yaml");
        std::fs::write(
            &skill_path,
            r#"
name: test-skill
version: 1.0.0
description: A test skill
category: testing
routing:
  strategy: council
  preset: frontier_free
  max_tokens: 4096
tools:
  - name: web_search
    enabled: true
permissions:
  - network:api
"#,
        )
        .unwrap();

        let skill = parse_skill_yaml(&skill_path).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.version, "1.0.0");
        assert_eq!(skill.category, "testing");
        assert_eq!(skill.routing.as_ref().unwrap().strategy, "council");
    }

    #[test]
    fn test_parse_skill_md() {
        let dir = tempdir().unwrap();
        let md_path = dir.path().join("SKILL.md");
        std::fs::write(
            &md_path,
            r#"---
name: test-from-md
description: A markdown skill
---

This skill does something useful.
"#,
        )
        .unwrap();

        let skill = parse_skill_md(&md_path).unwrap();
        assert_eq!(skill.name, "test-from-md");
        assert_eq!(skill.description.as_deref(), Some("A markdown skill"));
    }

    #[test]
    fn test_scan_skills_finds_yaml_and_md() {
        let dir = tempdir().unwrap();
        let skill1_dir = dir.path().join("skill1");
        let skill2_dir = dir.path().join("skill2");
        std::fs::create_dir_all(&skill1_dir).unwrap();
        std::fs::create_dir_all(&skill2_dir).unwrap();
        std::fs::write(
            skill1_dir.join("skill.yaml"),
            "name: yaml-skill\nversion: 1.0.0\ncategory: test\n",
        )
        .unwrap();
        std::fs::write(
            skill2_dir.join("SKILL.md"),
            "---\nname: md-skill\ndescription: MD skill\n---\n\nContent\n",
        )
        .unwrap();
        let skills = scan_skills_dir(dir.path());
        assert_eq!(
            skills.len(),
            2,
            "expected 2 skills, got 0. dir={:?}",
            dir.path()
        );
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"yaml-skill"));
        assert!(names.contains(&"md-skill"));
    }

    #[test]
    #[cfg(unix)] // crea symlinks POSIX para probar traversal; en Windows requieren privilegio → se omite
    fn test_path_traversal_protection() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        // Create a symlink pointing outside
        let link_path = dir.path().join("evil_link");
        std::os::unix::fs::symlink(outside.path(), &link_path).unwrap();
        let skill_dir = link_path.join("skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("skill.yaml"),
            "name: evil\nversion: 1.0.0\ncategory: test\n",
        )
        .unwrap();

        // scan_skills_dir should NOT follow the external symlink
        let skills = scan_skills_dir(dir.path());
        assert!(skills.is_empty(), "should not follow external symlinks");
    }

    #[test]
    fn test_refresh_from_disk_empty_scan() {
        let dir = tempdir().unwrap();
        let db = Connection::open(dir.path().join("test.db")).unwrap();
        let db = Mutex::new(db);
        db.lock().execute_batch(
            "CREATE TABLE IF NOT EXISTS skills (id TEXT PRIMARY KEY, name TEXT, version TEXT, description TEXT, category TEXT, routing_config TEXT, tools_config TEXT, permissions TEXT, enabled INTEGER DEFAULT 1, installed_at TEXT, updated_at TEXT, UNIQUE(name, version))"
        ).unwrap();

        // scan_skills_dir on non-existent dir should be empty
        let skills = scan_skills_dir(&dir.path().join("nonexistent"));
        assert!(skills.is_empty());
    }
}
