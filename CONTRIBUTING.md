# Contributing to cow-rs

Thanks for considering contributing. Here is how to do it without wasting
time.

## Before you start

1. **Check existing issues and PRs**: someone might already be working on
   it.
2. **Open an issue first**: unless it is a typo fix. Let us agree on the
   problem before you code the solution.
3. **One PR, one thing**: do not bundle unrelated changes.

## Writing style

All documentation, comments and written communication uses **Oxford
English**:

- British spelling with `-ise` / `-isation` endings (e.g. serialise,
  organise, recognise).
- British vocabulary (e.g. colour, behaviour, centre).
- **No em dashes.** Use colons, semicolons or restructure the sentence.
- Contractions are acceptable in informal documentation (e.g. READMEs).

Identifiers stay American where the language demands it (`Serialize`,
`Deserialize`, etc.).

## AI assistance

If AI wrote any part of your contribution, **you must say so in the PR
description**. This includes code generation beyond basic autocomplete,
documentation or comments, PR descriptions and commit messages, and
review responses.

In your PR description, add a line:

```
AI Assistance: [tool name] used for [what parts]
```

Example: `AI Assistance: Claude Code used for test fixtures and the PR
description; code spot-checked and verified via cargo test.`

**AI disclosure belongs in the PR description only.** Do not add
`Generated with Claude Code` (or similar) to commit messages. Do not add
Claude (or any AI) as a co-author in commits. Commit messages stay clean
and follow conventional commits.

Not disclosing AI use is treated as bad faith and will get the PR
closed.

## Before opening the PR

1. **Link to an issue**: PRs without linked issues may be ignored or
   closed.
2. **Fork and branch**: branch from `develop`, not `main`.
3. **Self-review**: review your own code before asking for review. Check
   that all tests pass, no compiler warnings, and the code follows
   existing patterns. Test it manually.
4. **Keep PRs small and focused**: ≤ 1,500 lines, ≤ 25 files, one feature
   surface. Stacked branches are welcome.

## PR body

Use the template. Required sections: **What / Changes / Testing / AI
assistance disclosure / Related issues / Checklist**.

The Testing section is not optional. "It compiles" is not enough. State
what you did and how you verified it.

## Quality bar

Code must pass:

```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --workspace -- -Dwarnings
cargo test --all-targets --all-features --workspace
```

Or, equivalently:

```
just clippy
just test
```

Plus:

- **Public APIs have doc comments.** Module-level architecture goes in
  `//!` headers or `docs/`, not in per-function rustdoc.
- **No `.unwrap()` in library code.** Use the crate's `Result<T>` alias
  and let the caller decide.
- **`thiserror` for errors in library crates, `eyre` only at the binary
  layer.** `anyhow` is out.
- **Conventional commits**: `<type>(<scope>)?: <description>`. Types:
  `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `ci`, `perf`,
  `style`. Use `!` for breaking changes (`feat!: ...`).

## What gets merged

**We want:** bug fixes for actual bugs; performance improvements with
benchmarks; features that solve real problems; documentation that helps
developers; tests that catch real issues.

**We do not want:** cosmetic changes with no real benefit; "refactoring"
that does not improve anything measurable; features nobody asked for;
breaking changes without strong justification; PRs that do not pass CI.

## Reality check

Reviews may take time. Be patient. Not all PRs will be accepted, even
if well-written. PRs to the wrong scope will be closed.

---

**TL;DR**: make it work, make it clear, make it tested. Use Oxford
English. Disclose AI in the PR description, never in commits.
