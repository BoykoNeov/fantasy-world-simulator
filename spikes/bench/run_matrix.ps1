param([int[]] $Runs = @(1, 2, 3), [int] $N = 408)
# Runs every app / GPU / upload-mode combination several times, then prints a summary.
$r = "W:\temp\claude\fantasymap-spike\run_bench.ps1"
$out = "W:\temp\claude\fantasymap-spike\out"
$configs = @(
    @{ App = "spike_egui"; P = "egui"; Pref = "high"; Partial = "0"; Name = "egui_high" },
    @{ App = "spike_egui"; P = "egui"; Pref = "low";  Partial = "0"; Name = "egui_low" },
    @{ App = "spike_bevy"; P = "bevy"; Pref = "high"; Partial = "0"; Name = "bevy_high" },
    @{ App = "spike_bevy"; P = "bevy"; Pref = "low";  Partial = "0"; Name = "bevy_low" },
    @{ App = "spike_bevy"; P = "bevy"; Pref = "high"; Partial = "1"; Name = "bevy_high_partial" },
    @{ App = "spike_bevy"; P = "bevy"; Pref = "low";  Partial = "1"; Name = "bevy_low_partial" }
)
foreach ($i in $Runs) {
    foreach ($c in $configs) {
        $env:BEVY_PARTIAL = $c.Partial
        $tag = "m$($N)_$($c.Name)_r$i"
        & $r -App $c.App -Pref $c.Pref -N $N -Tag $tag -TimeoutSec 240 | Out-Null
        $json = "$out\$($c.P)_$tag.json"
        if (Test-Path $json) {
            $j = Get-Content $json -Raw | ConvertFrom-Json
            "{0,-20} r{1} pan {2,6:N2}/{3,6:N2}  zoom {4,6:N2}/{5,6:N2}  brush {6,6:N2}/{7,6:N2}  (p50/p95 ms)" -f $c.Name, $i, $j.pan.p50_ms, $j.pan.p95_ms, $j.zoom.p50_ms, $j.zoom.p95_ms, $j.brush.p50_ms, $j.brush.p95_ms
        } else {
            $err = Select-String -Path "$out\$($c.App)_$tag.stderr.txt" -Pattern "ERROR|panicked" | Select-Object -First 1
            "{0,-20} r{1} FAILED: {2}" -f $c.Name, $i, $(if ($err) { $err.Line.Substring(0, [Math]::Min(160, $err.Line.Length)) } else { "no json, no error line" })
        }
    }
}
Remove-Item Env:BEVY_PARTIAL -ErrorAction SilentlyContinue
