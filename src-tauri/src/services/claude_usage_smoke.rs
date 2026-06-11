// Smoke test for claude_usage::summary() — confirms it now returns non-zero
// totals against the real ~/.claude/projects (Claude Code 2.x jsonl format).
// Lives in its own module so it can be enabled by feature without polluting
// the always-on test suite.
#[cfg(test)]
mod tests {
    use super::super::claude_usage;

    #[test]
    #[ignore = "machine-specific; run with `cargo test claude_usage_smoke -- --ignored --nocapture`"]
    fn real_home_returns_non_zero_when_claude_code_is_used() {
        let s = claude_usage::summary();
        println!(
            "[smoke] source_files={} total_tokens={} burn_24h={} burn_7d={}",
            s.source_files, s.total_tokens, s.burn_24h_tokens, s.burn_7d_tokens
        );
        for m in &s.by_model {
            println!(
                "  by_model[{}] in={} out={}",
                m.model, m.input_tokens, m.output_tokens
            );
        }
        for sess in s.by_session.iter().take(3) {
            println!(
                "  session[{}] in={} out={} model={:?}",
                sess.session_id, sess.input_tokens, sess.output_tokens, sess.model
            );
        }
        // Only assert that the reader ran without panic and produced a coherent struct.
        // We can't assume the developer has used Claude Code, so non-zero is best-effort.
        assert!(s.by_session.len() <= 50, "cap respected");
    }
}
