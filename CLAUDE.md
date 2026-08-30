# Codacy

Codacy scans every PR opened against this repo. Before running `git commit`, use the Codacy MCP server (`codacy_cli_analyze`) to analyze the files you changed and fix anything it flags — don't wait for CI to catch it.

A hook in `.claude/settings.json` enforces this: `git commit` is gated on `codacy_cli_analyze` having run since the last commit in this session.
