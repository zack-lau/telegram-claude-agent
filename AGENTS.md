# Repository Guidelines

## Project Structure & Module Organization

This repository contains a Bun/TypeScript Telegram bot that runs Claude Agent SDK sessions. Application code lives in `src/`: `bot/` handles Telegram inputs, formatting, middleware, and approvals; `agent/` handles sessions and SDK hooks; `memory/` provides project and LanceDB integrations. The entry point is `src/index.ts`, with configuration in `src/config.ts` and scheduling in `src/scheduler.ts`.

Tests are in `tests/*.test.ts`. Maintenance utilities live in `scripts/`. `data/` and `logs/` are runtime outputs. Treat `workspace/` as the agent's working directory and `projects/` as nested project material; do not broadly rewrite either while changing the root bot.

## Build, Test, and Development Commands

- `bun install`: install dependencies from `bun.lock`.
- `bun run dev`: start the bot in watch mode for local development.
- `bun run start`: run the bot once using the configured environment.
- `bun test`: execute the Bun test suite in `tests/`.
- `bunx tsc --noEmit`: type-check strict TypeScript without emitting `dist/`.
- `bun run setup:launchd`: install the macOS launchd service configuration when deploying locally.

Copy `.env.example` to `.env` before running. At minimum, configure `TELEGRAM_BOT_TOKEN` and `ALLOWED_USERS`.

## Coding Style & Naming Conventions

Use TypeScript ES modules and preserve the existing style: two-space indentation, semicolons, double-quoted strings, and explicit exported types where interfaces cross module boundaries. Use `camelCase` for functions and variables, `PascalCase` for types, and kebab-case module filenames such as `send-file.ts`. Keep changes scoped to the owning folder and validate external configuration with the existing Zod patterns.

No formatter or linter is currently configured; keep formatting consistent with adjacent code and run the type checker.

## Testing Guidelines

Tests use Bun's `bun:test` API (`describe`, `test`, `expect`, and `mock`). Add focused tests alongside related behavior, named `tests/<module>.test.ts`; for example, session changes belong in `tests/sessions.test.ts`. Cover message routing, approval gating, session persistence, and background-task behavior when those paths change. Run `bun test` and `bunx tsc --noEmit` before submitting.

## Commit & Pull Request Guidelines

Recent commits follow Conventional Commit-style subjects, such as `feat(aegis): ...`, `fix(qwen_review): ...`, and `docs: ...`. Use an imperative, scoped subject when practical.

Pull requests should explain the behavioral change, list verification commands, note configuration additions, and include screenshots only for user-visible Telegram output changes. Never commit `.env`, credentials, tokens, or generated logs.
