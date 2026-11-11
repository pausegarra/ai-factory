# Repository Guidelines

## Project Structure & Modules

`crates/factory-core/` contains the Rust engine, organized by domain, application, and infrastructure; its integration tests live in `crates/factory-core/tests/`. `src-tauri/` hosts the native Tauri shell, startup and lifecycle code, capabilities, and platform icons. The React and TypeScript UI lives in `desktop/src/`, with Vitest tests in `desktop/tests/`. Keep new code within the existing `context/factory` boundaries. `docs/` holds specifications, plans, and verification records; `.ci/` contains Linux build helpers, and `.github/workflows/` defines CI.

## Build, Test, and Development Commands

Run `npm run setup` to install root and desktop dependencies. Use `npm run dev` to launch the desktop app, or `npm --prefix desktop run dev` for a browser preview. `npm run build` packages the Tauri app; `npm --prefix desktop run build` builds only the frontend. Run `npm run test` for Vitest and Rust workspace tests, and `npm run check` for frontend build and Rust checks. CI also requires:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Coding Style & Naming Conventions

Use standard `rustfmt` formatting and 2-space indentation in TypeScript and TSX. Keep TypeScript strict and follow existing domain/application/infrastructure boundaries. Use `snake_case` for Rust modules and tests, PascalCase for React components, and lowercase descriptive filenames. Name frontend tests `*.test.ts` or `*.test.tsx`. No separate frontend formatter or linter command is configured.

## Testing Guidelines

Add UI tests under `desktop/tests/` with Vitest and engine integration tests under `crates/factory-core/tests/`. Run the full suite with `npm run test`. No coverage threshold is configured. Engine tests use local HTTP fixtures and fake agent processes; they must not call live GitHub services.

## Commits & Pull Requests

History contains only `Initial commit`, so no commit convention is established. Use a concise imperative subject, such as `feat: add workflow retry` or `fix: preserve run status`. PRs should explain behavior and scope, list validation commands and results, link related issues when available, and include screenshots for UI changes.

## Security & Configuration

Never commit GitHub tokens, local settings, or generated build output. Keep tests isolated from live accounts and services; use fixtures or fake CLI processes.
