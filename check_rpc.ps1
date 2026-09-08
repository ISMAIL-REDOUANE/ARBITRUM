$userRpc = [System.Environment]::GetEnvironmentVariable("ARBITRUM_RPC_URL", "User")
$machRpc = [System.Environment]::GetEnvironmentVariable("ARBITRUM_RPC_URL", "Machine")
$procRpc = [System.Environment]::GetEnvironmentVariable("ARBITRUM_RPC_URL", "Process")

Write-Host "User present: $([bool]$userRpc)"
Write-Host "Machine present: $([bool]$machRpc)"
Write-Host "Process present: $([bool]$procRpc)"
