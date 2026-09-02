$env:PRIVATE_KEY = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
$env:ARBITRUM_RPC_URL = "http://127.0.0.1:8545"
$env:CHAIN_ID = "421614"
$env:SHADOW_MODE = "true"
$env:RUST_LOG = "info"
$env:TELEMETRY_PORT = "3000"
$env:CONTRACT_ADDRESS = "0x5FbDB2315678afecb367f032d93F642f64180aa3"

Set-Location "C:\Users\pc\arbitrage cr\CR54\arbitrage-engine\target\release"
.\arbitrage-engine.exe --rpc-url "http://127.0.0.1:8545" --ws-port 3000
