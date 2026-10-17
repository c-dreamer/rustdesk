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

function Format-RustDeskArgument {
    # Start-Process joins an argument array with spaces and no quoting, so quote
    # each argument here using the rules CommandLineToArgvW (and Rust's std::env::args) parse.
    param([AllowEmptyString()][string]$Argument)
    if ($Argument -ne '' -and $Argument -notmatch '[\s"]') { return $Argument }
    $escaped = $Argument -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1'
    '"' + $escaped + '"'
}

function Invoke-RustDeskExe {
    # rustdesk.exe is a GUI-subsystem (WinMain) binary; PowerShell's `&` call
    # operator does not reliably capture its stdout. Start-Process with an
    # explicit redirect file does.
    param(
        [Parameter(Mandatory)][string]$Exe,
        [Parameter(Mandatory)][AllowEmptyString()][string[]]$ArgumentList
    )
    $outFile = [System.IO.Path]::GetTempFileName()
    try {
        $argString = ($ArgumentList | ForEach-Object { Format-RustDeskArgument $_ }) -join ' '
        Start-Process -FilePath $Exe -ArgumentList $argString -NoNewWindow -Wait -RedirectStandardOutput $outFile
        Get-Content -Raw -Encoding UTF8 -Path $outFile
    } finally {
        Remove-Item -Path $outFile -ErrorAction SilentlyContinue
    }
}

function Wait-RustDeskExe {
    # For commands that start long-lived processes (the tray, the main window):
    # Start-Process -Wait would wait for those too, i.e. forever, and a redirected
    # stdout file would be held open by them. Wait for this process only.
    param(
        [Parameter(Mandatory)][string]$Exe,
        [Parameter(Mandatory)][string[]]$ArgumentList
    )
    $argString = ($ArgumentList | ForEach-Object { Format-RustDeskArgument $_ }) -join ' '
    $process = Start-Process -FilePath $Exe -ArgumentList $argString -NoNewWindow -PassThru
    $process.WaitForExit()
}

function Confirm-RustDeskOutput {
    # rustdesk.exe exits 0 either way and reports problems as text (e.g. "Installation
    # and administrative privileges required!"), so compare with what success prints.
    param(
        [AllowNull()][AllowEmptyString()][string]$Output,
        [AllowEmptyString()][string]$Success
    )
    $text = "$Output".Trim()
    if ($text -ne $Success) { Write-Error $text }
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
        [Parameter(Mandatory)][AllowEmptyString()][string]$Value,
        [string]$Exe
    )
    $exe = Find-RustDeskExe -Path $Exe
    $out = Invoke-RustDeskExe -Exe $exe -ArgumentList '--option', $Name, $Value
    Confirm-RustDeskOutput -Output $out -Success ''
}

function Set-RustDeskPassword {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Password,
        [string]$Exe
    )
    $exe = Find-RustDeskExe -Path $Exe
    $out = Invoke-RustDeskExe -Exe $exe -ArgumentList '--password', $Password
    Confirm-RustDeskOutput -Output $out -Success 'Done!'
}

function Set-RustDeskId {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Id,
        [string]$Exe
    )
    $exe = Find-RustDeskExe -Path $Exe
    $out = Invoke-RustDeskExe -Exe $exe -ArgumentList '--set-id', $Id
    Confirm-RustDeskOutput -Output $out -Success 'Done!'
}

function Import-RustDeskConfig {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Path,
        [string]$Exe
    )
    $exe = Find-RustDeskExe -Path $Exe
    Invoke-RustDeskExe -Exe $exe -ArgumentList '--import-config', $Path
}

function Install-RustDeskService {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    Wait-RustDeskExe -Exe $exe -ArgumentList '--install-service'
}

function Uninstall-RustDeskService {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    Wait-RustDeskExe -Exe $exe -ArgumentList '--uninstall-service'
}

function Update-RustDeskApp {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    Wait-RustDeskExe -Exe $exe -ArgumentList '--update'
}

function Uninstall-RustDeskApp {
    [CmdletBinding(SupportsShouldProcess)]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    if ($PSCmdlet.ShouldProcess($exe, "Uninstall RustDesk")) {
        Wait-RustDeskExe -Exe $exe -ArgumentList '--uninstall'
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
    $out = Invoke-RustDeskExe -Exe $exe -ArgumentList '--option', $key, "$Percent"
    Confirm-RustDeskOutput -Output $out -Success ''
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
    $out = Invoke-RustDeskExe -Exe $exe -ArgumentList $cliArgs
    Confirm-RustDeskOutput -Output $out -Success 'Done!'
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
    $out = Invoke-RustDeskExe -Exe $exe -ArgumentList '--option', 'rmm-monitor-enabled', 'Y'
    Confirm-RustDeskOutput -Output $out -Success ''
}

function Disable-RustDeskMonitor {
    [CmdletBinding()]
    param([string]$Exe)
    $exe = Find-RustDeskExe -Path $Exe
    $out = Invoke-RustDeskExe -Exe $exe -ArgumentList '--option', 'rmm-monitor-enabled', 'N'
    Confirm-RustDeskOutput -Output $out -Success ''
}

Export-ModuleMember -Function Get-RustDeskId, Get-RustDeskStatus, Get-RustDeskOptions, Get-RustDeskPeers, Set-RustDeskOption, Set-RustDeskPassword, Set-RustDeskId, Import-RustDeskConfig, Install-RustDeskService, Uninstall-RustDeskService, Update-RustDeskApp, Uninstall-RustDeskApp, Get-RustDeskInventory, Get-RustDeskAlerts, Set-RustDeskThreshold, Get-RustDeskScripts, Add-RustDeskScript, Invoke-RustDeskScript, Enable-RustDeskMonitor, Disable-RustDeskMonitor
