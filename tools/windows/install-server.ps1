$ErrorActionPreference = "Stop"

$Port = 21300
$Install = Join-Path $env:ProgramFiles "StaDesk Server"
$Data = Join-Path $env:ProgramData "StaDesk\server"
$Task = "StaDesk Server"
$Rule = "StaDesk Server (UDP $Port)"

New-Item -ItemType Directory -Force -Path $Install | Out-Null
New-Item -ItemType Directory -Force -Path $Data | Out-Null
icacls $Data /inheritance:r /grant:r "SYSTEM:(OI)(CI)F" "Administrators:(OI)(CI)F" | Out-Null

Stop-ScheduledTask -TaskName $Task -ErrorAction SilentlyContinue
Get-Process -Name "stadesk-server" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 1
Copy-Item -Force (Join-Path $PSScriptRoot "stadesk-server.exe") (Join-Path $Install "stadesk-server.exe")

$Runner = Join-Path $Install "run.cmd"
$Executable = Join-Path $Install "stadesk-server.exe"
$Log = Join-Path $Data "server.log"
Set-Content -Encoding ASCII -Path $Runner -Value "`"$Executable`" --listen 0.0.0.0:$Port --data-dir `"$Data`" > `"$Log`" 2>&1"

Get-NetFirewallRule -DisplayName $Rule -ErrorAction SilentlyContinue | Remove-NetFirewallRule
New-NetFirewallRule -DisplayName $Rule -Direction Inbound -Protocol UDP -LocalPort $Port -Action Allow | Out-Null

$Action = New-ScheduledTaskAction -Execute "cmd.exe" -Argument "/c `"$Runner`""
$Trigger = New-ScheduledTaskTrigger -AtStartup
$Settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable
$Principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName $Task -Action $Action -Trigger $Trigger -Settings $Settings -Principal $Principal -Force | Out-Null
Start-ScheduledTask -TaskName $Task

for ($Attempt = 0; $Attempt -lt 50; $Attempt++) {
    if ((Test-Path $Log) -and (Select-String -Path $Log -Pattern "Server key" -Quiet)) {
        break
    }
    Start-Sleep -Milliseconds 200
}
Get-Content $Log
