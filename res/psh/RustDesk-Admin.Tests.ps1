# Minimal self-check, no Pester dependency. Run: powershell -File RustDesk-Admin.Tests.ps1
Import-Module "$PSScriptRoot\RustDesk-Admin.psm1" -Force

$failures = 0

function Assert($condition, $message) {
    if (-not $condition) {
        Write-Error "FAIL: $message"
        $script:failures++
    } else {
        Write-Host "PASS: $message"
    }
}

# Find-RustDeskExe: explicit -Path bypasses the Program Files search entirely.
$explicit = & (Get-Module RustDesk-Admin) { Find-RustDeskExe -Path "C:\fake\rustdesk.exe" }
Assert ($explicit -eq "C:\fake\rustdesk.exe") "explicit -Exe path is returned as-is"

# Find-RustDeskExe: no path and nothing installed throws rather than returning empty/null.
# Only checkable where RustDesk is not installed, since it would find the real exe.
$installed = (Test-Path "$env:ProgramFiles\RustDesk\rustdesk.exe") -or
    (Test-Path "${env:ProgramFiles(x86)}\RustDesk\rustdesk.exe")
if ($installed) {
    Write-Host "SKIP: RustDesk is installed, so Find-RustDeskExe finds it"
} else {
    try {
        & (Get-Module RustDesk-Admin) { Find-RustDeskExe -Path $null }
        Assert $false "should have thrown when rustdesk.exe is not found"
    } catch {
        Assert $true "throws when rustdesk.exe is not found anywhere"
    }
}

# Get-RustDeskOptions: a non-JSON gate message (e.g. "Installation and
# administrative privileges required!") must be returned as-is, not throw.
try {
    $out = & (Get-Module RustDesk-Admin) {
        function Invoke-RustDeskExe { "Installation and administrative privileges required!" }
        Get-RustDeskOptions -Exe "C:\fake\rustdesk.exe"
    }
    Assert ($out -eq "Installation and administrative privileges required!") "Get-RustDeskOptions returns non-JSON gate message instead of throwing"
} catch {
    Assert $false "Get-RustDeskOptions should not throw on non-JSON output: $_"
}

# Format-RustDeskArgument: Start-Process does not quote, so arguments with spaces
# or quotes must survive the round trip through the Windows command-line parser.
$fmt = { param($a) & (Get-Module RustDesk-Admin) { param($x) Format-RustDeskArgument $x } $a }
Assert ((& $fmt 'plain') -eq 'plain') "plain argument is left unquoted"
Assert ((& $fmt 'C:\Users\John Smith\x.ps1') -eq '"C:\Users\John Smith\x.ps1"') "argument with a space is quoted"
Assert ((& $fmt 'C:\My Dir\') -eq '"C:\My Dir\\"') "trailing backslash is doubled inside quotes"
Assert ((& $fmt 'say "hi"') -eq '"say \"hi\""') "embedded quotes are escaped"
Assert ((& $fmt '') -eq '""') "empty argument is kept as an empty quoted string"

# Setters: rustdesk.exe always exits 0, so a gate message must become an error.
# Mock and call share one module scriptblock, so the mock is in the call's scope chain.
$mod = Get-Module RustDesk-Admin
$threw = try {
    & $mod {
        function Invoke-RustDeskExe { "Installation and administrative privileges required!" }
        Enable-RustDeskMonitor -Exe "C:\fake\rustdesk.exe" -ErrorAction Stop
    } | Out-Null
    $false
} catch { $true }
Assert $threw "a setter reports a gate message as an error"
$threw = try {
    & $mod {
        function Invoke-RustDeskExe { "" }
        Set-RustDeskOption -Name rmm-cpu-threshold -Value "" -Exe "C:\fake\rustdesk.exe" -ErrorAction Stop
    } | Out-Null
    $false
} catch { $true }
Assert (-not $threw) "Set-RustDeskOption accepts an empty value, and empty output is success"
$threw = try {
    & $mod {
        function Invoke-RustDeskExe { "Done!" }
        Set-RustDeskId -Id "123456789" -Exe "C:\fake\rustdesk.exe" -ErrorAction Stop
    } | Out-Null
    $false
} catch { $true }
Assert (-not $threw) "'Done!' is success for --set-id"
$threw = try {
    & $mod {
        function Invoke-RustDeskExe { "Changing ID is disabled!" }
        Set-RustDeskId -Id "123456789" -Exe "C:\fake\rustdesk.exe" -ErrorAction Stop
    } | Out-Null
    $false
} catch { $true }
Assert $threw "any other --set-id output is an error"

# Script and JSON commands: failures must surface as errors, not as data.
$threw = try {
    & $mod {
        function Invoke-RustDeskExe { "Error: No such script: x" }
        Invoke-RustDeskScript -Name x -Exe "C:\fake\rustdesk.exe" -ErrorAction Stop
    } | Out-Null
    $false
} catch { $true }
Assert $threw "a failed --run-script is an error"
$out = & $mod {
    function Invoke-RustDeskExe { "hello" }
    Invoke-RustDeskScript -Name x -Exe "C:\fake\rustdesk.exe" -ErrorAction Stop
}
Assert ($out -eq "hello") "a script's own output is returned"
$threw = try {
    & $mod {
        function Invoke-RustDeskExe { '{"error":"Administrator privileges required to use the script library"}' }
        Get-RustDeskScripts -Exe "C:\fake\rustdesk.exe" -ErrorAction Stop
    } | Out-Null
    $false
} catch { $_.Exception.Message -like "*Administrator*" }
Assert $threw "Get-RustDeskScripts reports the service's error, not a JSON parse error"
$count = (& $mod {
    function Invoke-RustDeskExe { '[{"name":"a","schedule_secs":null},{"name":"b","schedule_secs":60}]' }
    Get-RustDeskScripts -Exe "C:\fake\rustdesk.exe"
} | Measure-Object).Count
Assert ($count -eq 2) "a JSON array reaches the pipeline item by item"

if ($failures -gt 0) {
    Write-Error "$failures check(s) failed"
    exit 1
}
Write-Host "All checks passed"
