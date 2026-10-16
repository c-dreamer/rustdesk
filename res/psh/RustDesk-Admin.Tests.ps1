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
try {
    & (Get-Module RustDesk-Admin) { Find-RustDeskExe -Path $null }
    Assert $false "should have thrown when rustdesk.exe is not found"
} catch {
    Assert $true "throws when rustdesk.exe is not found anywhere"
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

if ($failures -gt 0) {
    Write-Error "$failures check(s) failed"
    exit 1
}
Write-Host "All checks passed"
