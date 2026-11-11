# Tunnelde Agent

[Documentation / 文档](docs/README.md)

[简体中文](README.zh-cn.md)

Optional native MCP stdio bridge and persistent remote supervisor for
[Tunnelde](https://github.com/TunneldeDev/Tunnelde). Ordinary SSH, SFTP, terminal,
local API chat and direct SSH agent execution do not require it. The binary includes no Node
runtime. Using an external runtime still requires explicit user consent.

## Download and compatibility

Download the matching OS/architecture binary and `SHA256SUMS` from this repository's
[Releases](https://github.com/TunneldeDev/Tunnelde-agent/releases). Verify its SHA-256 before
selecting it in Tunnelde. On POSIX, grant executable permission with `chmod +x <binary>`.
For a remote supervisor, choose the remote machine's OS/architecture.

This repository preserves the native helper's history from the desktop repository, through
desktop dev `1b95417252173e731c60b3d64c1327124d09dad7`. The imported source tree is
`a12394357c54329e77e5659181939bfbdee03de3`. Its inherited baseline version is **0.26.1**;
subsequent versions are independent of desktop releases. Migration preserves existing command
arguments, runner files and protocol behavior. Do not replace a running supervisor's binary.

## MCP stdio

Start Tunnelde MCP in the application and create a scoped client token. Configure the AI client's
stdio command to the helper's absolute path with argument `--mcp-stdio`, and set
`TUNNELDE_MCP_URL` to the plugin's numeric loopback HTTP endpoint and `TUNNELDE_MCP_TOKEN` to
the token. Keep the desktop application and MCP plugin running. The helper rejects non-loopback
URLs, embedded URL credentials, redirects and proxies. Tokens travel in HTTP headers; avoid
putting them in command arguments. The Node bridge shipped with the plugin is an alternative
only when the user has explicitly chosen to use Node.

## Remote AI

**This section describes advanced persistent supervisors.** With Tunnelde 0.25.0+ and
assistant plugins 1.3.0+, ordinary remote execution uses the chat's machine selector and SSH
stdio without this helper. Codex and Claude Code start their installed binaries directly;
Pi approves its permission-extension upload; Tunnelde AI selects and approves a remote Node
runtime. See [normal assistant setup](https://github.com/TunneldeDev/Tunnelde/blob/dev/docs/en-us/guide/ai-assistants.md).

In the plugin's advanced persistent-supervisor configuration, select a connected SSH host, an absolute fresh deployment
directory, the matching native helper on this computer, and the remote Node executable's explicit
path. The host asks before uploading this plugin and the helper; it never installs Node or changes
system services. Files are private on POSIX (directory 0700, configuration 0600). On Windows they
inherit the selected account directory's ACL; choose a private user directory.

Helper 0.27.0 adds explicitly negotiated [initialized configuration](docs/en-us/development/runner-initialization.md).
Opted-in plugins receive configuration and secrets through stdin and initialize instead of a
configuration file. Use a host that advertises `runners.startInitialized`; older hosts and
existing tasks retain the legacy contract. Helper storage starts empty in the new mode;
plugins can still explicitly persist their own state.

The helper launches a detached supervisor without installing a service. Secrets arrive over SSH
stdin and remain in memory. Conversation state, bounded task output, sequenced event journals
and pending approval descriptions stay in the deployment directory, so it should be treated as
private application data. Human approval waits remain pending while the desktop is disconnected.
After reconnecting, inspect the task before resuming. Stopping or restricting a plugin records stop
requests for unreachable runners and retries them on the next SSH connection. A stop request is
not proof that an unreachable process has exited. Closing the desktop preserves remote agents.

Plugins and selected coding CLIs are trusted executable code with the account's OS permissions.
API tools enforce canonical path scope and the four approval modes; these checks do not form an
OS sandbox for arbitrary commands. Claude Code and Pi ReadOnly execution is rejected; Codex
ReadOnly uses its own read sandbox. Use an API profile for host-enforced read-only file tools.

## Build and test

For read-only protocol negotiation, see [helper capability discovery](docs/en-us/development/capabilities.md).

```sh
cargo fmt -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --locked
node --test tests/*.test.mjs scripts/*.test.mjs
```

Tests explicitly use the development Node executable to run a disposable plugin fixture;
the shipped helper does not discover or install Node. Override `TUNNELDE_TEST_HELPER` to test
a selected built binary. Add `--release` for an optimized build in `target/release/`.

## Contributing and releases

Contribute to `main` through a PR. Linux, Windows and Intel macOS CI must pass, including runner
recovery tests; macOS CI also cross-compiles Apple Silicon. Squash merges keep changes scoped.
Version the Cargo manifest and lockfile together, merge the reviewed PR, then push `vX.Y.Z`.
The release workflow checks tag ancestry and versions, builds/tests all five platform binaries,
verifies their hashes and only then publishes the release. Published assets cannot be replaced.
Failed drafts can be retried with the same immutable tag. Desktop releases do not trigger this
repository. Protocol changes require compatibility review and behavioral tests.

Licensed under GPL-3.0-only; see [LICENSE](LICENSE).
