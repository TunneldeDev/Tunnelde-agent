# Optional native helper

`tunnelde-agent` is a standalone executable for the MCP stdio bridge and recoverable remote AI
agents. Ordinary SSH, SFTP, terminal and local API chat use does not require it. It contains no
Node runtime and is not included in the desktop installers or the main GitHub Release assets.

For these optional integrations, open a successful **Release** workflow run under GitHub
**Actions** and download the `tunnelde-agent-<platform>-<architecture>` artifact. Extract it and
verify the binary against its accompanying SHA-256 file. The artifacts are retained for 30 days;
you can also build from source using the instructions below. Choose the target machine's OS and
architecture for remote runners. Linux ARM64 is built for remote agents even though the desktop
application currently targets Linux x64. On macOS and Linux, restore executable permission with
`chmod +x <binary>` after extracting the artifact.

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
runtime. See [normal assistant setup](../docs/ai-assistants.md).

In the plugin's advanced persistent-supervisor configuration, select a connected SSH host, an absolute fresh deployment
directory, the matching native helper on this computer, and the remote Node executable's explicit
path. The host asks before uploading this plugin and the helper; it never installs Node or changes
system services. Files are private on POSIX (directory 0700, configuration 0600). On Windows they
inherit the selected account directory's ACL; choose a private user directory.

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

```sh
cargo build --locked --manifest-path native-helpers/Cargo.toml
cargo test --locked --manifest-path native-helpers/Cargo.toml
node --test native-helpers/tests/*.test.mjs
```

For an optimized executable, add `--release` to the build command. The output is
`native-helpers/target/release/tunnelde-agent` (`tunnelde-agent.exe` on Windows); build on the
target platform or use an appropriately configured Rust cross-compilation toolchain.

Integration tests use explicitly selected test runtimes and disposable directories. No SSH server,
paid provider, system service, clipboard, or existing user data is touched by these tests.
