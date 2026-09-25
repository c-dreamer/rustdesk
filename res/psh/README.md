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

## AI agents (MCP)

`rustdesk.exe --mcp` runs a Model Context Protocol server over stdio (no listening port),
so Claude Code, Codex and opencode can drive this machine headlessly.

Read tools, always on: `get_status`, `get_inventory`, `get_alerts`, `list_scripts`, `list_peers`.
Write tools: `run_script`, and `set_option` (for `rmm-*` options only). These are refused
until a human turns them on. The agent cannot turn them on itself.

```powershell
rustdesk --option rmm-agent-write Y   # enable write tools; N to disable again
```

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

## Self-check

```powershell
powershell -File RustDesk-Admin.Tests.ps1
```
