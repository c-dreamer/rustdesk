function Find-RustDeskExe {
    param([string]$Path)
    if ($Path) { return $Path }
    $candidates = @(
        "$env:ProgramFiles\RustDesk\rustdesk.exe",
        "${env:ProgramFiles(x86)}\RustDesk\rustdesk.exe"
    )
    foreach ($c in $candidates) {
        if (Test-Path $c) { return $c }
    }
    throw "rustdesk.exe not found. Pass -Exe <path> explicitly."
}

function Invoke-RustDeskExe {
    # rustdesk.exe is a GUI-subsystem (WinMain) binary; PowerShell's `&` call
    # operator does not reliably capture its stdout. Start-Process with an
    # explicit redirect file does.
    param(
        [Parameter(Mandatory)][string]$Exe,
        [Parameter(Mandatory)][string[]]$ArgumentList
    )
    $outFile = [System.IO.Path]::GetTempFileName()
    try {
        Start-Process -FilePath $Exe -ArgumentList $ArgumentList -NoNewWindow -Wait -RedirectStandardOutput $outFile
        Get-Content -Raw -Encoding UTF8 -Path $outFile
    } finally {
        Remove-Item -Path $outFile -ErrorAction SilentlyContinue
    }
}

function Get-RustDeskId {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    Invoke-RustDeskExe -Exe $exe -ArgumentList "--get-id"
}

function Get-RustDeskStatus {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    Invoke-RustDeskExe -Exe $exe -ArgumentList "--status" | ConvertFrom-Json
}

function Get-RustDeskOptions {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    $out = Invoke-RustDeskExe -Exe $exe -ArgumentList "--list-options"
    try { $out | ConvertFrom-Json } catch { $out }
}

function Get-RustDeskPeers {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    Invoke-RustDeskExe -Exe $exe -ArgumentList "--list-peers" | ConvertFrom-Json
}

function Set-RustDeskOption {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$Value,
        [string]$Exe
    )
    $exe = Find-RustDeskExe -Path $Exe
    & $exe --option $Name $Value
}

function Set-RustDeskPassword {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Password,
        [string]$Exe
    )
    $exe = Find-RustDeskExe -Path $Exe
    & $exe --password $Password
}

function Set-RustDeskId {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Id,
        [string]$Exe
    )
    $exe = Find-RustDeskExe -Path $Exe
    & $exe --set-id $Id
}

function Import-RustDeskConfig {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Path,
        [string]$Exe
    )
    $exe = Find-RustDeskExe -Path $Exe
    & $exe --import-config $Path
}

function Install-RustDeskService {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    & $exe --install-service
}

function Uninstall-RustDeskService {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    & $exe --uninstall-service
}

function Update-RustDeskApp {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    & $exe --update
}

function Uninstall-RustDeskApp {
    [CmdletBinding(SupportsShouldProcess)]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    if ($PSCmdlet.ShouldProcess($exe, "Uninstall RustDesk")) {
        & $exe --uninstall
    }
}

function Get-RustDeskInventory {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    Invoke-RustDeskExe -Exe $exe -ArgumentList "--inventory" | ConvertFrom-Json
}

function Get-RustDeskAlerts {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    Invoke-RustDeskExe -Exe $exe -ArgumentList "--alerts-status" | ConvertFrom-Json
}

function Set-RustDeskThreshold {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][ValidateSet('Cpu', 'Memory', 'Disk')][string]$Metric,
        [Parameter(Mandatory)][int]$Percent,
        [string]$Exe
    )
    $exe = Find-RustDeskExe -Path $Exe
    $key = @{ Cpu = 'rmm-cpu-threshold'; Memory = 'rmm-mem-threshold'; Disk = 'rmm-disk-threshold' }[$Metric]
    & $exe --option $key $Percent
}

function Get-RustDeskScripts {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    Invoke-RustDeskExe -Exe $exe -ArgumentList "--list-scripts" | ConvertFrom-Json
}

function Add-RustDeskScript {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$Path,
        [ValidateRange(1, [int]::MaxValue)][int]$ScheduleSeconds,
        [string]$Exe
    )
    $exe = Find-RustDeskExe -Path $Exe
    $cliArgs = @("--add-script", $Name, $Path)
    if ($PSBoundParameters.ContainsKey('ScheduleSeconds')) { $cliArgs += "$ScheduleSeconds" }
    Invoke-RustDeskExe -Exe $exe -ArgumentList $cliArgs
}

function Invoke-RustDeskScript {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Name,
        [string]$Exe
    )
    $exe = Find-RustDeskExe -Path $Exe
    Invoke-RustDeskExe -Exe $exe -ArgumentList "--run-script", $Name
}

function Enable-RustDeskMonitor {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    & $exe --option rmm-monitor-enabled true
}

function Disable-RustDeskMonitor {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    & $exe --option rmm-monitor-enabled false
}

Export-ModuleMember -Function Get-RustDeskId, Get-RustDeskStatus, Get-RustDeskOptions, Get-RustDeskPeers, Set-RustDeskOption, Set-RustDeskPassword, Set-RustDeskId, Import-RustDeskConfig, Install-RustDeskService, Uninstall-RustDeskService, Update-RustDeskApp, Uninstall-RustDeskApp, Get-RustDeskInventory, Get-RustDeskAlerts, Set-RustDeskThreshold, Get-RustDeskScripts, Add-RustDeskScript, Invoke-RustDeskScript, Enable-RustDeskMonitor, Disable-RustDeskMonitor
