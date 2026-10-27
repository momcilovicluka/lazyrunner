# Security

## Supported versions

Only the latest release gets fixes. lazyrunner updates itself (README, "Updates"), so a fix reaches installs the next
time they open.

## Reporting a vulnerability

Please don't open a public issue. Report it privately on GitHub:
[Security → Report a vulnerability](https://github.com/momcilovicluka/lazyrunner/security/advisories/new). Say what
you found, how to reproduce it and which version you ran (`lazyrunner --version`). You'll hear back as soon as
possible, and the fix and its advisory credit you unless you'd rather they didn't.

What matters most:

- **The self-update**: what lazyrunner downloads, from where, and how it checks it before replacing itself.
- **Commands and files from outside**: a profile, a log line or a link that makes lazyrunner run a program or open a
  file it shouldn't.
- **The control channel**: the local pipe the MCP server (`lazyrunner mcp`) uses to drive a running lazyrunner.
