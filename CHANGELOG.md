# Changelog

All notable changes to this project are documented in this file. Entries are
generated from the conventional commits by [git-cliff](https://git-cliff.org),
except where writing one by hand said more.

## [0.1.0] - 2026-08-01

First release.

Names each Zellij tab after the folder its panes are in, or after the git
repository when that folder sits inside one, and keeps that name current as you
`cd` around.

- Tabs are named at session start, on new tabs and on layout restore, not only
  after a `cd`.
- Folder name, the repository name instead when there is one, `~` for `$HOME`.
- `git_detection` can be turned off to always use the folder name and skip
  running `git` altogether.
- Optional pane-count suffix, off by default.
- Decoration pipe API (`set_prefix`, `set_suffix`, `clear_prefix`,
  `clear_suffix`, `clear_all`) so another plugin can wrap a symbol around the
  name without fighting for `TabInfo.name`. Decorations survive a `cd`.
