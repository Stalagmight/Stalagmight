# Helper script to start/restart the Tauri dev server (Windows / PowerShell)
# Mirrors scripts/dev.sh for non-Unix environments.

$ErrorActionPreference = 'SilentlyContinue'

# Kill existing dev processes if they exist
Get-Process -Name 'stalagmight' | Stop-Process -Force

# Kill any Vite/Node process bound to the dev port (1420)
$conns = Get-NetTCPConnection -LocalPort 1420 -State Listen -ErrorAction SilentlyContinue
foreach ($c in $conns) {
    Stop-Process -Id $c.OwningProcess -Force -ErrorAction SilentlyContinue
}

Start-Sleep -Seconds 1

# Start the dev server
$ErrorActionPreference = 'Stop'
$env:VITE_IPC_TRACE = 'true'
pnpm tauri dev
