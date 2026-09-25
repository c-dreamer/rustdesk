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

## Self-check

```powershell
powershell -File RustDesk-Admin.Tests.ps1
```
