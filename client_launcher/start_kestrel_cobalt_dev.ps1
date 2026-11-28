#Requires -Version 5.1
# Thin cross-repo delegator to canonical kestrel-canvas/client_launcher/start_kestrel_cobalt_dev.ps1
$RelayRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $env:COBALT_RELAY_DIR) { $env:COBALT_RELAY_DIR = $RelayRoot }
$Candidates = @(
    $(if ($env:KESTREL_CANVAS_DIR) { Join-Path $env:KESTREL_CANVAS_DIR "client_launcher\start_kestrel_cobalt_dev.ps1" }),
    (Join-Path $RelayRoot "..\kestrel-canvas\client_launcher\start_kestrel_cobalt_dev.ps1")
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }
if (-not $Candidates) {
    throw "Canonical launcher not found. Set KESTREL_CANVAS_DIR to your kestrel-canvas checkout."
}
& $Candidates[0] @args
exit $LASTEXITCODE
