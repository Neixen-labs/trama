# TRAMA workboard

`KICKOFF.md` is the product roadmap. This file is the coordination contract for parallel work; GitHub Issues are the live task list.

## Claim protocol

1. Pick one open issue and comment `Claimed by <agent> — branch <name> — paths <paths>`.
2. Start from current `origin/main` in a separate worktree: `git worktree add -b <branch> <path> origin/main`.
3. One issue, one branch, one PR. Do not edit another claimed path.
4. Before opening a PR: rebase on `origin/main`, run the task's checks, and record exact results in the PR.
5. Merge only after CI is green. Close the issue from the PR (`Closes #N`).

A task is unclaimed until its GitHub Issue comment exists. Do not use this file as a mutable status board: that would itself become a conflict point.

## Path ownership

| Area | Owner while claimed | Do not overlap with |
|---|---|---|
| `core/trama-format/**` | one format agent | another format change |
| `core/trama-epanet/**`, `core/trama-example/**` | one solver agent | another solver change |
| `core/trama-cli/**`, `core/trama-wasm/**` | one packaging agent | another entry-point change |
| `engine/**` | one runtime agent | another runtime change |
| `docs/SPEC.md`, `docs/SOLVER_CONTRACT.md` | one spec agent | format/code changes needing the same decision |
| `site/**` | one site agent | another site change |
| `.github/**`, root manifests | one integration agent | all concurrent dependency/CI work |

## Quality gate

- Read `KICKOFF.md`, `CLAUDE.md`, and the touched code before editing.
- New behavior: one focused test written first; then `cargo test --release`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check` for Rust work.
- Core source files carry the BSL SPDX header. Workflow files carry Apache SPDX.
- A PR may not silently change `docs/SPEC.md`; format decisions need a separate docs PR and owner approval first.
- Keep a deliberate ceiling explicit with a `ponytail:` comment only where it genuinely exists.

## Ready lanes

Open GitHub Issues are the lanes. This file used to list three of them by number; they were closed long ago and the table outlived them, which is the failure mode the claim protocol above exists to avoid — a status board in git goes stale silently, while an issue cannot.
