// services/agents.rs — 019 F1 (T010): abstracción agent-neutral MÍNIMA (R1/FR-011).
//
// HOY el "dispatch de agente" está implícito en el string `cli_kind` (claude/codex/gemini/aider/…)
// que viaja por `agent_profiles` y se resuelve a (cmd,args,env) vía `synth_mode` → `resolve_mode`
// en commands.rs. Esta abstracción FORMALIZA ese dispatch en un único punto: un `AgentKind` tipado,
// un `AgentDescriptor` que describe cada agente (display name + binario + cómo nace su `mode`), y un
// `spawn_in_worktree` que RUTEA (describe) el lanzamiento de un agente en un worktree aislado.
//
// La abstracción es DESCRIPTIVA, no ejecutora: el spawn REAL del PTY sigue viviendo en commands.rs
// (que tiene el `PtyManager`). `spawn_in_worktree` devuelve un `SpawnPlan` (cmd/args/env/cwd) que el
// caller materializa. Esto evita duplicar el PTY spawn y mantiene el comportamiento observable
// idéntico — sólo se centraliza la decisión "qué binario / qué mode para este kind".
//
// F4 (ACP) — IMPLEMENTADO (T040): el nuevo `AgentKind::Acp` + su descriptor + su brazo en
// `spawn_in_worktree` (que arma el `env` de transporte ACP vía `crate::services::acp`) es TODO lo que
// hizo falta. El flujo best-of-N NO cambió: `launch_best_of_n` sigue llamando `spawn_in_worktree` y
// materializando el `SpawnPlan` igual; sólo que para un `SpawnPlan` con `env` de transporte ACP el
// caller monta un `AcpClient` en lugar de un PTY clásico. "Agregar el agente ACP" quedó LOCAL a
// agents.rs + acp.rs, demostrando la promesa agent-neutral de F1 (FR-011/FR-012).

use crate::services::agent_profiles::AgentProfile;
use std::collections::HashMap;

/// Los agentes que Furx sabe lanzar HOY, agent-neutral. Mapea 1:1 con los `cli_kind` ejecutables
/// de `agent_profiles` que corren un CLI en un pane PTY (008/006). `zsh`/`openai-api`/`custom` NO son
/// "agentes autónomos" del flujo best-of-N (zsh es un shell; openai-api/custom son escapes), así que
/// quedan FUERA de este enum — `from_cli_kind` los devuelve como `None` y el caller cae al path legacy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    Gemini,
    Aider,
    /// 062: xAI Grok CLI (`grok`, en ~/.grok/bin). Auth por OAuth propio (`grok login`/`--oauth`),
    /// como Claude — no exige cuenta a NUESTRO nivel (usa su propio login).
    Grok,
    /// 019 F4 (T040): un agente que se habla por **ACP** (Agent Client Protocol, JSON-RPC sobre
    /// stdio) en lugar de un PTY clásico. Agent-neutral: lo que cambia es el TRANSPORTE, no el flujo.
    Acp,
}

impl AgentKind {
    /// Todos los kinds conocidos (para enumerar en UI/tests).
    pub const ALL: &'static [AgentKind] = &[
        AgentKind::ClaudeCode,
        AgentKind::Codex,
        AgentKind::Gemini,
        AgentKind::Aider,
        AgentKind::Grok,
        AgentKind::Acp,
    ];

    /// El `cli_kind` string canónico que usa `agent_profiles`/`resolve_mode`. Estable: es la
    /// SSOT del binario y del prefijo de `mode`. NO cambiar sin migrar la DB.
    pub fn as_cli_kind(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Gemini => "gemini",
            AgentKind::Aider => "aider",
            AgentKind::Grok => "grok",
            AgentKind::Acp => "acp",
        }
    }

    /// Resuelve un `cli_kind` string a un `AgentKind` tipado. `None` para los kinds que NO son
    /// agentes autónomos del flujo (`zsh`/`openai-api`/`custom`) o desconocidos — el caller decide
    /// el fallback (típicamente: tratar como mode legacy/shell, sin romper el spawn).
    pub fn from_cli_kind(cli_kind: &str) -> Option<AgentKind> {
        match cli_kind {
            "claude" => Some(AgentKind::ClaudeCode),
            "codex" => Some(AgentKind::Codex),
            "gemini" => Some(AgentKind::Gemini),
            "aider" => Some(AgentKind::Aider),
            "grok" => Some(AgentKind::Grok),
            "acp" => Some(AgentKind::Acp),
            _ => None,
        }
    }

    /// Nombre legible para la UI/cards/compare.
    pub fn display_name(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::Gemini => "Gemini",
            AgentKind::Aider => "Aider",
            AgentKind::Grok => "Grok",
            AgentKind::Acp => "ACP agent",
        }
    }

    /// ¿Este agente se habla por ACP (JSON-RPC stdio) en lugar de PTY? Lo usa el caller para decidir
    /// si materializa un `AcpClient` o un PTY a partir del `SpawnPlan`.
    pub fn uses_acp(self) -> bool {
        matches!(self, AgentKind::Acp)
    }
}

/// Describe CÓMO se materializa un agente: su binario y si necesita una cuenta (account_slug) para
/// nacer su `mode`. Esto es lo que hoy está disperso entre `synth_mode` (requisitoria de cuenta) y
/// `resolve_mode` (binario/wrapper). El descriptor lo centraliza para que el dispatch sea UNO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDescriptor {
    pub kind: AgentKind,
    /// Nombre legible para UI.
    pub display_name: &'static str,
    /// El binario base del CLI (lo que `resolve_mode`/el wrapper terminan invocando). Para los
    /// agentes con cuenta el spawn real va por el wrapper `~/bin/<cli>-as-<slug>`, pero el binario
    /// LÓGICO (para routing/labels/done-detection patterns) es éste.
    pub cli_binary: &'static str,
    /// ¿Este agente EXIGE una cuenta (account_slug) para lanzarse? (Claude sí; codex/gemini/aider
    /// tienen mode legacy sin cuenta). Espeja la regla de `synth_mode`.
    pub requires_account: bool,
}

/// Descriptor canónico de un `AgentKind`. Única fuente de verdad del dispatch agent-neutral.
pub fn descriptor_for(kind: AgentKind) -> AgentDescriptor {
    match kind {
        AgentKind::ClaudeCode => AgentDescriptor {
            kind,
            display_name: "Claude Code",
            cli_binary: "claude",
            requires_account: true,
        },
        AgentKind::Codex => AgentDescriptor {
            kind,
            display_name: "Codex",
            cli_binary: "codex",
            requires_account: false,
        },
        AgentKind::Gemini => AgentDescriptor {
            kind,
            display_name: "Gemini",
            cli_binary: "gemini",
            requires_account: false,
        },
        AgentKind::Aider => AgentDescriptor {
            kind,
            display_name: "Aider",
            cli_binary: "aider",
            requires_account: false,
        },
        // 062: Grok usa su propio OAuth (`grok login`) → no exige cuenta a NUESTRO nivel (como los
        // modos legacy de codex/gemini/aider). El binario lógico es `grok` (~/.grok/bin/grok).
        AgentKind::Grok => AgentDescriptor {
            kind,
            display_name: "Grok",
            cli_binary: "grok",
            requires_account: false,
        },
        // ACP (F4/T040): el binario lógico es el adaptador ACP del agente (ej. `claude-code-acp`).
        // NO exige cuenta a NUESTRO nivel: el agente ACP resuelve su credencial BYOK por su propia
        // config/Keychain (F-I) — Furx nunca le proxea la key.
        AgentKind::Acp => AgentDescriptor {
            kind,
            display_name: "ACP agent",
            cli_binary: ACP_DEFAULT_BIN,
            requires_account: false,
        },
    }
}

/// Binario por defecto del adaptador ACP. Es el binario LÓGICO del descriptor; el binario REAL a
/// spawnear viaja en `SpawnPlan.env` (`FURX_ACP_BIN`) para que el caller lo materialice. NO secreto.
pub const ACP_DEFAULT_BIN: &str = "claude-code-acp";

/// Resuelve el `cli_kind` EFECTIVO de una tarea de orquestación, agent-neutral. Centraliza la
/// derivación que `orchestration_prepare_task` hacía inline: del agent_profile (006) si hay, sino
/// del prefijo del `mode` legacy. Devuelve `(cli_kind_string, Option<AgentKind>)` — el string para
/// cachear/done-detection (incluye zsh/custom), el `AgentKind` tipado sólo para los agentes del flujo.
///
/// `profile_lookup` es un closure (no `&Db`) para que esto sea PURO y unit-testeable sin tocar la DB.
pub fn resolve_task_kind<F>(
    agent_profile_id: Option<&str>,
    mode: Option<&str>,
    profile_lookup: F,
) -> (Option<String>, Option<AgentKind>)
where
    F: FnOnce(&str) -> Option<AgentProfile>,
{
    // 1) Del agent_profile, si hay uno asociado.
    let from_profile = agent_profile_id
        .and_then(profile_lookup)
        .map(|p| p.cli_kind);
    // 2) Sino, del prefijo del mode legacy ("claude-A" → "claude", "codex" → "codex", "zsh" → "zsh").
    let cli_kind = from_profile
        .or_else(|| mode.map(|m| m.split(['-', '_', ' ']).next().unwrap_or("").to_string()));
    let cli_kind = cli_kind.filter(|s| !s.is_empty());
    let agent_kind = cli_kind.as_deref().and_then(AgentKind::from_cli_kind);
    (cli_kind, agent_kind)
}

/// Plan de lanzamiento de un agente en un worktree: lo que el caller (commands.rs) necesita para
/// montar el pane y spawnear el PTY. La abstracción DESCRIBE/RUTEA; el spawn real lo hace el caller
/// con su `PtyManager`. Mantiene retrocompat: el `mode` es el mismo string que `resolve_mode` ya
/// entiende, y `objective` es lo que el front entrega al agente tras montar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnPlan {
    pub kind: Option<AgentKind>,
    /// `mode` legacy que `resolve_mode`/`resolve_agent_runtime` consumen (retrocompat total).
    pub mode: String,
    /// cwd del agente = el worktree aislado del attempt.
    pub worktree_path: String,
    /// Objetivo a entregar al agente tras montar (vía pty_write).
    pub objective: String,
    /// Env extra a inyectar (HOY vacío; F4/ACP lo poblará). Reservado para no romper la firma luego.
    pub env: HashMap<String, String>,
}

/// Rutea el lanzamiento de un agente en un worktree → `SpawnPlan`. Encapsula la decisión "qué mode
/// para este (kind, slug)"; NO spawnea (eso es del caller con el PtyManager). Si `agent_kind` es
/// `Some`, deriva el `mode` por la regla del descriptor (claude exige cuenta); sino usa el
/// `fallback_mode` (mode legacy de la tarea, p.ej. "zsh"). Esto deja el dispatch en UN solo lugar y
/// listo para que F4 enchufe ACP (un `AgentKind` nuevo → un brazo acá, sin tocar el flujo).
///
/// FAIL-SAFE (audit codex+deepseek HIGH 1): cuando se pidió un `AgentKind` ESPECÍFICO pero su mode no
/// se puede resolver (ej. Claude sin `account_slug`, o slug inválido), devuelve `Err` — NUNCA cae al
/// `fallback_mode` (que sería "zsh" → lanzaría un SHELL en lugar del agente pedido). El fallback al
/// `fallback_mode` SÓLO es válido cuando `agent_kind` es `None` (path legacy/shell, sin agente pedido).
/// El caller (best-of-N) traduce ese `Err` en un attempt `failed` con razón, sin lanzar nada.
pub fn spawn_in_worktree(
    agent_kind: Option<AgentKind>,
    account_slug: Option<&str>,
    worktree_path: &str,
    objective: &str,
    fallback_mode: &str,
) -> anyhow::Result<SpawnPlan> {
    // BRAZO ACP (F4/T040): el agente ACP NO va por `synth_mode` (no tiene "mode" PTY) ni por el PTY
    // clásico. Su `SpawnPlan` lleva un `env` de transporte ACP (binario + flag, CERO secretos) que el
    // caller lee para montar un `AcpClient`. El `mode` queda como rótulo "acp" sólo para
    // labels/done-detection. Esto es TODO lo que "agregar el agente ACP" tocó del dispatch.
    if matches!(agent_kind, Some(AgentKind::Acp)) {
        return Ok(SpawnPlan {
            kind: agent_kind,
            mode: "acp".to_string(),
            worktree_path: worktree_path.to_string(),
            objective: objective.to_string(),
            env: crate::services::acp::acp_transport_env(ACP_DEFAULT_BIN),
        });
    }

    let mode = match agent_kind {
        // Reusa la MISMA pieza pura que el resto del spawn (cero divergencia de comportamiento):
        // (cli_kind, slug) → mode string. Si la combinación es inválida (claude sin cuenta / slug
        // inválido), PROPAGAMOS el error: NO se cae a fallback_mode porque eso lanzaría el agente
        // equivocado (un shell). El caller marca el attempt failed.
        Some(k) => crate::services::agent_profiles::synth_mode(k.as_cli_kind(), account_slug)
            .map_err(|e| {
                anyhow::anyhow!(
                    "no se puede resolver el mode para el agente {}: {}",
                    k.as_cli_kind(),
                    e
                )
            })?,
        // Sin AgentKind pedido (tarea legacy/shell): el fallback_mode es el mode legítimo.
        None => fallback_mode.to_string(),
    };
    Ok(SpawnPlan {
        kind: agent_kind,
        mode,
        worktree_path: worktree_path.to_string(),
        objective: objective.to_string(),
        env: HashMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(cli_kind: &str) -> AgentProfile {
        AgentProfile {
            id: "p1".into(),
            name: "Test".into(),
            description: String::new(),
            cli_kind: cli_kind.into(),
            account_slug: None,
            model: None,
            system_prompt: String::new(),
            default_cwd: None,
            council_enabled: false,
            council_preset: None,
            shell_enabled: false,
            icon: None,
            color: None,
            is_builtin: false,
            engine_kind: "cli".into(),
            category: None,
            plugins: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn every_kind_resolves_its_descriptor() {
        // cada AgentKind resuelve un descriptor coherente (display no vacío, binario no vacío).
        for &k in AgentKind::ALL {
            let d = descriptor_for(k);
            assert_eq!(d.kind, k);
            assert!(!d.cli_binary.is_empty());
            assert!(!d.display_name.is_empty());
            // Para los agentes CLI clásicos, el binario lógico ES el cli_kind. ACP es la excepción:
            // su binario lógico es el adaptador (claude-code-acp), el cli_kind rótulo es "acp".
            if k != AgentKind::Acp {
                assert_eq!(d.cli_binary, k.as_cli_kind());
            }
        }
        // claude EXIGE cuenta; los demás no (ACP resuelve su BYOK aparte → tampoco exige acá).
        assert!(descriptor_for(AgentKind::ClaudeCode).requires_account);
        assert!(!descriptor_for(AgentKind::Codex).requires_account);
        assert!(!descriptor_for(AgentKind::Gemini).requires_account);
        assert!(!descriptor_for(AgentKind::Aider).requires_account);
        assert!(!descriptor_for(AgentKind::Acp).requires_account);
    }

    #[test]
    fn cli_kind_roundtrip_and_unknown() {
        for &k in AgentKind::ALL {
            assert_eq!(AgentKind::from_cli_kind(k.as_cli_kind()), Some(k));
        }
        // un kind que NO es agente del flujo → None (caller cae a legacy), no panic.
        assert_eq!(AgentKind::from_cli_kind("zsh"), None);
        assert_eq!(AgentKind::from_cli_kind("openai-api"), None);
        assert_eq!(AgentKind::from_cli_kind("custom"), None);
        assert_eq!(AgentKind::from_cli_kind("totally-unknown"), None);
    }

    #[test]
    fn resolve_task_kind_prefers_profile_over_mode() {
        // Con agent_profile asociado, gana su cli_kind (no el prefijo del mode).
        let (ck, ak) = resolve_task_kind(Some("p1"), Some("zsh"), |_| Some(profile("codex")));
        assert_eq!(ck.as_deref(), Some("codex"));
        assert_eq!(ak, Some(AgentKind::Codex));
    }

    #[test]
    fn resolve_task_kind_falls_back_to_mode_prefix() {
        // Sin profile, sale del prefijo del mode legacy.
        let (ck, ak) = resolve_task_kind(None, Some("claude-work"), |_| None);
        assert_eq!(ck.as_deref(), Some("claude"));
        assert_eq!(ak, Some(AgentKind::ClaudeCode));
        // mode "zsh" → cli_kind "zsh" pero NO es AgentKind del flujo.
        let (ck, ak) = resolve_task_kind(None, Some("zsh"), |_| None);
        assert_eq!(ck.as_deref(), Some("zsh"));
        assert_eq!(ak, None);
        // sin profile ni mode → nada.
        let (ck, ak) = resolve_task_kind(None, None, |_| None);
        assert_eq!(ck, None);
        assert_eq!(ak, None);
    }

    #[test]
    fn spawn_routes_to_correct_binary_via_mode() {
        // codex sin cuenta → mode legacy "codex" (el binario que resolve_mode invoca).
        let plan =
            spawn_in_worktree(Some(AgentKind::Codex), None, "/wt/a", "hacé X", "zsh").unwrap();
        assert_eq!(plan.mode, "codex");
        assert_eq!(plan.kind, Some(AgentKind::Codex));
        assert_eq!(plan.worktree_path, "/wt/a");
        assert_eq!(plan.objective, "hacé X");

        // codex CON cuenta → "codex-work" (rutea al wrapper ~/bin/codex-as-work).
        let plan =
            spawn_in_worktree(Some(AgentKind::Codex), Some("work"), "/wt/b", "", "zsh").unwrap();
        assert_eq!(plan.mode, "codex-work");

        // claude CON cuenta → "claude-A".
        let plan =
            spawn_in_worktree(Some(AgentKind::ClaudeCode), Some("A"), "/wt/c", "", "zsh").unwrap();
        assert_eq!(plan.mode, "claude-A");

        // gemini/aider rutean a su binario.
        assert_eq!(
            spawn_in_worktree(Some(AgentKind::Gemini), None, "/wt/d", "", "zsh")
                .unwrap()
                .mode,
            "gemini"
        );
        assert_eq!(
            spawn_in_worktree(Some(AgentKind::Aider), None, "/wt/e", "", "zsh")
                .unwrap()
                .mode,
            "aider"
        );
    }

    #[test]
    fn spawn_claude_without_account_fails_no_zsh_fallback() {
        // FAIL-SAFE (audit HIGH 1): claude EXIGE cuenta; sin slug, synth_mode falla → spawn_in_worktree
        // devuelve Err. NUNCA cae a "zsh" (eso lanzaría un shell en lugar del agente pedido). El caller
        // traduce este Err en un attempt failed.
        let r = spawn_in_worktree(Some(AgentKind::ClaudeCode), None, "/wt", "", "zsh");
        assert!(r.is_err(), "claude sin cuenta debe fallar, no caer a zsh");
        // un slug inválido también falla (no se rebaja a fallback).
        let r = spawn_in_worktree(Some(AgentKind::Codex), Some("bad slug!"), "/wt", "", "zsh");
        assert!(r.is_err(), "slug inválido debe fallar, no caer a zsh");
    }

    #[test]
    fn spawn_no_kind_uses_fallback_mode() {
        // Sin AgentKind (tarea legacy/shell), usa el mode legacy tal cual (fallback legítimo).
        let plan = spawn_in_worktree(None, None, "/wt", "obj", "zsh").unwrap();
        assert_eq!(plan.mode, "zsh");
        assert_eq!(plan.kind, None);
        assert!(plan.env.is_empty());
    }

    #[test]
    fn acp_kind_routes_through_transport_env_not_pty() {
        // F4/T040: un AgentKind::Acp produce un SpawnPlan con el `env` de transporte ACP (binario +
        // flag) — NO va por synth_mode ni cae al fallback "zsh". Esto demuestra que "agregar el agente
        // ACP" es LOCAL a agents.rs/acp.rs: el flujo best-of-N llama spawn_in_worktree igual.
        let plan =
            spawn_in_worktree(Some(AgentKind::Acp), None, "/wt/acp", "hacé X", "zsh").unwrap();
        assert_eq!(plan.kind, Some(AgentKind::Acp));
        assert_eq!(plan.mode, "acp");
        assert_eq!(plan.objective, "hacé X");
        assert_eq!(plan.worktree_path, "/wt/acp");
        // El env marca transporte ACP y trae el binario; el caller montará un AcpClient con esto.
        assert!(crate::services::acp::is_acp_transport(&plan.env));
        assert_eq!(
            plan.env
                .get(crate::services::acp::ENV_ACP_BIN)
                .map(String::as_str),
            Some(ACP_DEFAULT_BIN)
        );
        // uses_acp() es la señal que el caller consulta.
        assert!(AgentKind::Acp.uses_acp());
        assert!(!AgentKind::Codex.uses_acp());
    }

    #[test]
    fn acp_kind_roundtrips_and_carries_no_secret() {
        // El cli_kind "acp" roundtrips por la SSOT como cualquier otro agente del flujo.
        assert_eq!(AgentKind::from_cli_kind("acp"), Some(AgentKind::Acp));
        assert_eq!(AgentKind::Acp.as_cli_kind(), "acp");
        // BYOK (F-I): el env del SpawnPlan ACP no contiene credenciales.
        let plan = spawn_in_worktree(Some(AgentKind::Acp), None, "/wt", "", "zsh").unwrap();
        for (k, v) in &plan.env {
            let kv = format!("{k}={v}").to_lowercase();
            assert!(
                !["token", "secret", "password", "bearer", "sk-", "api_key", "apikey"]
                    .iter()
                    .any(|m| kv.contains(m)),
                "el SpawnPlan ACP no debe transportar secretos: {kv}"
            );
        }
    }
}
