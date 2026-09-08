$lines = Get-Content .env
foreach ($line in $lines) {
    if ($line -match '^([A-Za-z_0-9]+)=(.*)$') {
        $name = $matches[1]
        $val = $matches[2]
        [System.Environment]::SetEnvironmentVariable($name, $val, "Process")
    }
}
[System.Environment]::SetEnvironmentVariable("BLOXROUTE_RPC", "https://arb1.arbitrum.io/rpc", "Process")
[System.Environment]::SetEnvironmentVariable("ARBITRUM_RPC_URL", "https://arb1.arbitrum.io/rpc", "Process")
Write-Host "Loaded .env and set BLOXROUTE_RPC & ARBITRUM_RPC_URL"
$rpc = [System.Environment]::GetEnvironmentVariable("ARBITRUM_RPC_HTTP_URL", "Process")
Write-Host "ARBITRUM_RPC_HTTP_URL: $rpc"

