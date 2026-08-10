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
| `compiler/**` | one compiler agent | another compiler change |
| `engine/**` | one runtime agent | another runtime change |
| `solvers/**` | one solver agent | another solver change |
| `docs/SPEC.md`, `docs/SOLVER_CONTRACT.md` | one spec agent | format/code changes needing the same decision |
| `site/**` | one site agent | another site change |
| `.github/**`, root manifests | one integration agent | all concurrent dependency/CI work |

## Quality gate

- Read `KICKOFF.md`, `CLAUDE.md`, and the touched code before editing.
- New behavior: one focused test written first; then `uv run pytest -q`, `uv run ruff check .`, and `uv run mypy src` for compiler work.
- Core source files carry the BSL SPDX header. Workflow files carry Apache SPDX.
- A PR may not silently change `docs/SPEC.md`; format decisions need a separate docs PR and owner approval first.
- Keep a deliberate ceiling explicit with a `ponytail:` comment only where it genuinely exists.

## Ready lanes

| Issue | Lane | Claimed paths | Depends on |
|---|---|---|---|
| #9 | Compiler: nullable typed properties for multi-edge GeoJSON | `compiler/**` | none |
| #10 | Runtime: TypeScript container header/directory reader | `engine/**` | no code dependency; follow `docs/SPEC.md` |
| #11 | Spec: EPANET import/export boundary proposal | `docs/**` | owner decision before implementation |

These lanes do not overlap. Do not begin EPANET compiler code until its format decision is approved.
