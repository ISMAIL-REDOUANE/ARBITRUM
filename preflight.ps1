# Pre-flight and deployment script for Windows
$ErrorActionPreference = "Continue"

Write-Host ""
Write-Host "================================================================================"
Write-Host "       MEV ARBITRAGE ENGINE - PRE-FLIGHT VERIFICATION & DEPLOYMENT"
Write-Host "================================================================================"
Write-Host ""

$PROJECT_ROOT = "C:\Users\pc\arbitrage cr\CR54"

# Check if files exist
Write-Host "[INFO] Checking deployment files..."
$deployScript = Join-Path $PROJECT_ROOT "deploy.sh"
$runScript = Join-Path $PROJECT_ROOT "run_testnet.sh"
$envFile = Join-Path $PROJECT_ROOT ".env.testnet"

if (Test-Path $deployScript) { Write-Host "  [OK] deploy.sh found" } else { Write-Host "  [FAIL] deploy.sh not found" }
if (Test-Path $runScript) { Write-Host "  [OK] run_testnet.sh found" } else { Write-Host "  [FAIL] run_testnet.sh not found" }
if (Test-Path $envFile) { Write-Host "  [OK] .env.testnet found" } else { Write-Host "  [WARN] .env.testnet not found (will be created by deploy.sh)" }

# Check port 3000
Write-Host ""
Write-Host "[INFO] Checking port 3000 availability..."
$portInUse = Get-NetTCPConnection -LocalPort 3000 -ErrorAction SilentlyContinue
if ($portInUse) {
    Write-Host "  [WARN] Port 3000 is in use by PID: $($portInUse.OwningProcess)"
} else {
    Write-Host "  [OK] Port 3000 is available"
}

# Check Foundry
Write-Host ""
Write-Host "[INFO] Checking Foundry installation..."
$forgePath = Get-Command forge -ErrorAction SilentlyContinue
$castPath = Get-Command cast -ErrorAction SilentlyContinue
if ($forgePath -and $castPath) {
    Write-Host "  [OK] Foundry (forge, cast) installed"
    $forgeVersion = forge --version 2>&1 | Select-Object -First 1
    Write-Host "         Version: $forgeVersion"
} else {
    Write-Host "  [FAIL] Foundry not found. Install from: https://getfoundry.sh"
}

# Check Rust/Cargo
Write-Host ""
Write-Host "[INFO] Checking Rust/Cargo installation..."
$rustPath = Get-Command cargo -ErrorAction SilentlyContinue
if ($rustPath) {
    Write-Host "  [OK] Rust/Cargo installed"
    $cargoVersion = cargo --version 2>&1 | Select-Object -First 1
    Write-Host "         Version: $cargoVersion"
} else {
    Write-Host "  [WARN] Rust not found. Required for building the engine."
}

# Check Node/npm
Write-Host ""
Write-Host "[INFO] Checking Node.js/npm installation..."
$nodePath = Get-Command node -ErrorAction SilentlyContinue
$npmPath = Get-Command npm -ErrorAction SilentlyContinue
if ($nodePath -and $npmPath) {
    Write-Host "  [OK] Node.js/npm installed"
    $nodeVersion = node --version 2>&1 | Select-Object -First 1
    Write-Host "         Version: $nodeVersion"
} else {
    Write-Host "  [WARN] Node.js not found. Dashboard requires npm."
}

Write-Host ""
Write-Host "================================================================================"
Write-Host "                           PRE-FLIGHT COMPLETE"
Write-Host "================================================================================"
Write-Host ""
Write-Host "Next steps:"
Write-Host "  1. If PRIVATE_KEY not set: export PRIVATE_KEY=0xYour64HexKey"
Write-Host "  2. Run: ./deploy.sh        (Deploys contract to Arbitrum Sepolia)"
Write-Host "  3. Run: source .env.testnet && ./run_testnet.sh   (Launches engine)"
Write-Host ""
