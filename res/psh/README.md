# RustDesk-Admin PowerShell module

Thin wrappers around the `rustdesk.exe` CLI flags (`--get-id`, `--status`, `--list-options`,
`--list-peers`, `--option`, `--password`, `--install-service`, `--uninstall-service`). No new functionality —
each function just shells out to the exe and, where the flag prints JSON, pipes it through
`ConvertFrom-Json`.

## Usage

```powershell
Import-Module .\RustDesk-Admin.psm1

Get-RustDeskId
Get-RustDeskStatus
Get-RustDeskOptions
Get-RustDeskPeers
Set-RustDeskOption -Name "custom-rendezvous-server" -Value "example.com"
Set-RustDeskId -Id "new-id"
Import-RustDeskConfig -Path "C:\deploy\rustdesk.toml"
Install-RustDeskService
Update-RustDeskApp
Uninstall-RustDeskApp -Confirm   # prompts before uninstalling
```

Auto-detects `rustdesk.exe` under `Program Files`; pass `-Exe <path>` to override.
Admin CLI commands require an elevated shell — run PowerShell as Administrator.
Setters return nothing on success and write an error otherwise (rustdesk.exe always exits 0,
so the module checks its output), so `-ErrorAction Stop` works in deployment scripts.

If the module was downloaded, Windows' default execution policy blocks it; run
`Unblock-File .\RustDesk-Admin.psm1` once, or start PowerShell with `-ExecutionPolicy Bypass`.

## Script library

```powershell
Add-RustDeskScript -Name cleanup -Path .\cleanup.ps1 -ScheduleSeconds 3600  # omit for on-demand only
Set-RustDeskOption -Name rmm-scripts-enabled -Value Y                      # turn the scheduler on
```

On an installed endpoint, scheduled scripts run as SYSTEM. The library therefore lives in
the service profile (`C:\Windows\ServiceProfiles\LocalService\...\config\rmm_scripts.json`),
where only admins can write it. Add, list and run scripts from an elevated shell.

On Linux and macOS the scheduler runs in the desktop user's RustDesk, so add scripts as that
user, without `sudo`; `sudo rustdesk --add-script` writes root's own library, which it never
reads.

## AI agents (MCP)

`rustdesk --mcp` runs a Model Context Protocol server over stdio (no listening port),
so Claude Code, Codex, opencode, Gemini CLI, Hermes or any other MCP client can drive this
machine headlessly, on Windows, Linux or macOS.

Read tools, always on: `get_status`, `get_inventory`, `get_alerts`, `list_scripts`, `list_peers`.
Write tools: `run_script`, and `set_option` (monitoring options only: `rmm-monitor-enabled` and the CPU, memory, disk and watched-service settings). These are refused
until a human turns them on, and no MCP tool can turn them on. This is a guard for
well-behaved agents, not a sandbox: an agent that also has a shell as the same user is that
user, and can do whatever the user's RustDesk can.

Watched services and the installed-software list are Windows only. On macOS and Linux,
alerts go to `get_alerts`, the Tools tab and `rmm_alerts.log`, with no desktop notification.

On an installed Windows RustDesk the script library is admin-only (see above), so
`list_scripts` and `run_script` work only when the agent itself runs elevated, and then run
scripts as that admin; only the scheduler runs them as SYSTEM.

```powershell
Set-RustDeskOption -Name rmm-agent-write -Value Y   # enable write tools; N to disable again
```

`--option` only works on an installed RustDesk, from an elevated shell. The same switch is in
Settings > Tools > AI agents; on an installed RustDesk it sits behind "Unlock Security Settings"
(UAC or the unlock PIN). On a portable copy, that switch is the only way to turn write tools on.

Register the server. Replace the path if RustDesk is installed elsewhere.

Claude Code:

```powershell
claude mcp add rustdesk -- "C:\Program Files\RustDesk\rustdesk.exe" --mcp
```

Codex (`~/.codex/config.toml`, or `codex mcp add rustdesk -- <exe> --mcp`):

```toml
[mcp_servers.rustdesk]
command = 'C:\Program Files\RustDesk\rustdesk.exe'
args = ["--mcp"]
```

opencode (`opencode.json`):

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "rustdesk": {
      "type": "local",
      "command": ["C:\\Program Files\\RustDesk\\rustdesk.exe", "--mcp"],
      "enabled": true
    }
  }
}
```

### Linux and macOS

Same server, different binary path. The snippets use the Linux path, `/usr/bin/rustdesk`; on
macOS, replace it with `/Applications/RustDesk.app/Contents/MacOS/RustDesk`. On Linux and
macOS, `sudo <that path> --option rmm-agent-write Y` (or the Tools tab switch) opens the write
gate.

Claude Code:

```sh
claude mcp add --scope user rustdesk -- /usr/bin/rustdesk --mcp
```

Codex (`codex mcp add rustdesk -- /usr/bin/rustdesk --mcp`, or `~/.codex/config.toml`):

```toml
[mcp_servers.rustdesk]
command = "/usr/bin/rustdesk"
args = ["--mcp"]
```

opencode (`~/.config/opencode/opencode.json`):

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "rustdesk": {
      "type": "local",
      "command": ["/usr/bin/rustdesk", "--mcp"],
      "enabled": true
    }
  }
}
```

Gemini CLI (writes `~/.gemini/settings.json`; the `--` stops gemini reading `--mcp` as its own flag):

```sh
gemini mcp add -s user rustdesk /usr/bin/rustdesk -- --mcp
```

Hermes (`~/.hermes/config.yaml`, then `/reload-mcp` in a running session):

```yaml
mcp_servers:
  rustdesk:
    command: "/usr/bin/rustdesk"
    args: ["--mcp"]
```

Any other MCP client: launch `rustdesk --mcp` as a stdio server.

## Self-check

```powershell
powershell -ExecutionPolicy Bypass -File RustDesk-Admin.Tests.ps1
```
