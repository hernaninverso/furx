<!--
Thanks for contributing to Furx! Please fill in this template so reviewers can
understand and verify your change quickly. Keep PRs focused — one logical change
per PR is much easier to review.
-->

## Summary

<!-- What does this PR do, and why? Link any related issue, e.g. "Closes #123". -->

## Type of change

<!-- Delete the ones that don't apply. -->

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds functionality)
- [ ] Breaking change (fix or feature that changes existing behaviour)
- [ ] Documentation / tooling only
- [ ] Refactor / internal cleanup (no behaviour change)

## Open-core boundary

<!-- Furx is open-core: the whole tree is Apache-2.0, but some paid features are
     gated by a runtime license check. See CONTRIBUTING.md. -->

- [ ] This change targets the **core** (free, Apache-2.0) surface, **or**
- [ ] This change touches the **gating logic** and is a fix/improvement to it
      (it does **not** ungate a paid feature).

## Checklist

- [ ] **Tests pass locally** — `cd src-tauri && cargo test` (Rust) **and**
      `npm test` + `npm run test:components` (frontend) are green.
- [ ] I added or updated tests covering my change where it makes sense.
- [ ] **No secrets, keys, tokens, or personal/infrastructure data** are included
      in the diff (no hosts, IPs, accounts, emails, or credentials baked into
      source, migrations, fixtures, or tests). Endpoints default to
      `localhost`/empty and are configured per-user at runtime.
- [ ] If I added a new Tauri command, I also registered it in the command
      registry so the 1:1 coverage test stays green.
- [ ] **Coding style:** `cargo fmt` + `cargo clippy` clean (Rust); `tsc -b`
      green (TypeScript). I matched the style of the files I edited.
- [ ] Documentation updated where relevant (README, `docs/`, code comments).
- [ ] **DCO sign-off:** every commit is signed off with `git commit -s`
      (`Signed-off-by:` trailer). See CONTRIBUTING.md.

## How was this tested?

<!-- Describe the tests you ran, the OS/platform, and any manual verification. -->

## Screenshots / recordings (if UI)

<!-- Optional, but very helpful for UI changes. -->
