# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Project scaffolding: workspace structure, git init, `.gitignore`
- `README.md` with architecture overview, tech stack, and implementation progress checklist
- `CLAUDE.md` with git workflow rules, commit conventions, and code guidelines
- `CHANGELOG.md` (this file)
- `.github/pull_request_template.md` for PR workflow
- `docs/agent/ARCHITECTURE.md` — full technical design document
- `docs/agent/RESEARCH.md` — Cetus/Turbos/Sui protocol research
- `docs/agent/RESEARCH_2.md` — supplementary research findings

### Changed
- Reorganized DEX crates into `crates/dex/` subfolder (`dex/common`, `dex/cetus`, `dex/turbos`)
- Added `dex-common` crate for shared `DexCommands` trait
- Updated architecture diagram, dependency DAG, and implementation checklist to reflect new structure
