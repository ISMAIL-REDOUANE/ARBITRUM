#Requires -Version 5.1
param(
    [string]$EnginePath = "C:\RustBuilds\ArbitrageEngine\release\arbitrage-engine.exe",
    [string]$LogFile = "C:\Users\pc\arbitrage cr\CR54\engine_runtime.log",
    [int]$TelemetryPort = 3000,
    [int]$HealthCheckInterval = 5
)

$ErrorActionPreference = "Continue"
$Script:EngineProcess = $null
$Script:RestartCount = 0
$Script:MaxRestarts = 10
$Script:LastErrorTime = $null
$Script:IncidentLog = @()

function Write-Log {
    param([string]$Message, [string]$Level = "INFO")
    $Timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss.fff"
    $LogEntry = "[$Timestamp] [$Level] $Message"
    Add-Content -Path $LogFile -Value $LogEntry
    switch ($Level) {
        "ERROR" { Write-Host $LogEntry -ForegroundColor Red }
        "WARN"  { Write-Host $LogEntry -ForegroundColor Yellow }
        "OK"    { Write-Host $LogEntry -ForegroundColor Green }
        default { Write-Host $LogEntry }
    }
}

function Get-IncidentReport {
    $report = @"
============================================================
INCIDENT REPORT - $(Get-Date -Format "yyyy-MM-dd HH:mm:ss")
============================================================
Total Restarts: $Script:RestartCount
Last Error: $Script:LastErrorTime
Recent Incidents: $($Script:IncidentLog.Count)
"@
    foreach ($incident in $Script:IncidentLog) {
        $report += "`n$incident"
    }
    return $report
}

function Test-EngineHealth {
    $health = @{
        ProcessRunning = $false
        MemoryMB = 0
        CPUPercent = 0
        WSConnected = $false
        LastLogAge = $null
    }

    if ($Script:EngineProcess -and !$Script:EngineProcess.HasExited) {
        $health.ProcessRunning = $true
        try {
            $proc = Get-Process -Id $Script:EngineProcess.Id -ErrorAction SilentlyContinue
            if ($proc) {
                $health.MemoryMB = [math]::Round($proc.WorkingSet64 / 1MB, 2)
                $health.CPUPercent = [math]::Round($proc.CPU, 2)
            }
        } catch {}
    }

    if (Test-Path $LogFile) {
        $lastLine = Get-Content $LogFile -Tail 1 -ErrorAction SilentlyContinue
        if ($lastLine -match '(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})') {
            try {
                $health.LastLogAge = [datetime]::ParseExact($matches[1], "yyyy-MM-ddTHH:mm:ss", $null)
            } catch {}
        }
        $health.WSConnected = $lastLine -notmatch "Connection failed|retrying|TlsFeatureNotEnabled"
    }

    return $health
}

function Start-EngineProcess {
    Write-Log "Starting Arbitrage Engine in SHADOW MODE..." "INFO"

    $env:SHADOW_MODE = "true"
    $env:RUST_LOG = "info"

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $EnginePath
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true

    try {
        $Script:EngineProcess = [System.Diagnostics.Process]::Start($psi)
        $Script:EngineProcess.StartInfo = $psi

        $Script:EngineProcess.BeginOutputReadLine()
    $Script:EngineProcess.BeginErrorReadLine()

        Write-Log "Engine process started with PID: $($Script:EngineProcess.Id)" "OK"
        return $true
    } catch {
        Write-Log "Failed to start engine: $_" "ERROR"
        return $false
    }
}

function Stop-EngineProcess {
    if ($Script:EngineProcess -and !$Script:EngineProcess.HasExited) {
        Write-Log "Stopping engine process..."
        try {
            $Script:EngineProcess.Kill($true)
            $Script:EngineProcess.WaitForExit(5000)
        } catch {}
    }
}

function Test-TelemetryEndpoint {
    try {
        $response = Invoke-WebRequest -Uri "http://localhost:$TelemetryPort/health" -TimeoutSec 2 -UseBasicParsing -ErrorAction SilentlyContinue
        return $response.StatusCode -eq 200
    } catch {
        return $false
    }
}

Write-Log "============================================================" "INFO"
Write-Log "Arbitrage Engine Health Monitor Starting" "INFO"
Write-Log "============================================================" "INFO"

$startupSuccess = Start-EngineProcess
if (-not $startupSuccess) {
    Write-Log "Fatal: Could not start engine" "ERROR"
    exit 1
}

Write-Log "Waiting 5 seconds for engine initialization..."
Start-Sleep -Seconds 5

Write-Log "Starting health monitoring loop (interval: ${HealthCheckInterval}s)" "INFO"

$runLoop = $true
$consecutiveFailures = 0

while ($runLoop) {
    Start-Sleep -Seconds $HealthCheckInterval

    $health = Test-EngineHealth

    if (-not $health.ProcessRunning) {
        $consecutiveFailures++
        Write-Log "Engine process not responding (attempt $consecutiveFailures)" "WARN"

        if ($consecutiveFailures -ge 3) {
            Write-Log "Critical: Engine process died. Restarting..." "ERROR"
            Stop-EngineProcess
            Start-Sleep -Seconds 2

            if ($Script:RestartCount -ge $Script:MaxRestarts) {
                Write-Log "Fatal: Max restarts ($Script:MaxRestarts) exceeded" "ERROR"
                Write-Log (Get-IncidentReport) "ERROR"
                break
            }

            $Script:RestartCount++
            $success = Start-EngineProcess
            if ($success) {
                Write-Log "Engine restarted (restart #$Script:RestartCount)" "OK"
                Start-Sleep -Seconds 5
            }
            $consecutiveFailures = 0
        }
    } else {
        $consecutiveFailures = 0

        $memMB = $health.MemoryMB
        $cpu = $health.CPUPercent
        $telemetryOk = Test-TelemetryEndpoint

        $status = if ($telemetryOk) { "OK" } else { "WARN" }
        Write-Log "Health: Memory=${memMB}MB CPU=${cpu}s Telemetry=$($telemetryOk)" $status

        if ($memMB -gt 500) {
            Write-Log "Warning: High memory usage detected (${memMB}MB)" "WARN"
        }

        if ($health.LastLogAge) {
            $logAge = (Get-Date) - $health.LastLogAge
            if ($logAge.TotalSeconds -gt 30) {
                Write-Log "Warning: No new log entries for $($logAge.TotalSeconds)s" "WARN"
            }
        }
    }

    if ($Script:IncidentLog.Count -gt 100) {
        $Script:IncidentLog = $Script:IncidentLog | Select-Object -Last 50
    }
}

Write-Log "Health monitor shutting down..." "INFO"
Stop-EngineProcess
Write-Log (Get-IncidentReport) "INFO"
Write-Log "Health monitor stopped" "INFO"
