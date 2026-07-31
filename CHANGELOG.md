# Changelog

All notable changes to this project are documented in this file.
Generated from the conventional commits by [git-cliff](https://git-cliff.org).

## [0.1.0] - 2026-07-31

### Bug Fixes

- Apply result to all tabs awaiting same cwd
- Close six review findings on the discovery query
- Resolve symlinked cwds via root aliases + review nits
- Don't name nested repos after an ancestor root; bound caches
- Request ReadCliPipes to cover pipe unblocking (#5)

### Documentation

- Add README, LICENSE, and package metadata for publishing
- Add ROADMAP

### Features

- Initial tab-cwd-rename plugin for Zellij 0.44
- Use git repo name for tabs
- Rename new tabs from active tab CWD
- Add git_detection config option
- Add pane count display
- Add pipe API for tab prefix/suffix decorations (#1)
- Name tabs at session start, new tab and restore

### Performance

- Deduplicate in-flight git rev-parse queries

### Refactor

- Split pure core from zellij adapter via effects
- Align code with CONTEXT.md glossary

