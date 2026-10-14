param(
    [Parameter(Mandatory)] [string] $App,        # spike_egui | spike_bevy
    [Parameter(Mandatory)] [string] $Pref,       # low | high
    [int] $N = 408,
    [int] $Provinces = 5000,
    [string] $Tag = "",
    [int] $TimeoutSec = 300,
    [string] $Backend = ""      # "" (default) | vulkan | dx12
)
$exe = "M:\claud_projects\fantasymap\spikes\target\release\$App.exe"
$out = "W:\temp\claude\fantasymap-spike\out"
if (-not $Tag) { $Tag = "${Pref}_n$N" + $(if ($Backend) { "_$Backend" } else { "" }) }
$env:WGPU_POWER_PREF = $Pref
if ($Backend) { $env:WGPU_BACKEND = $Backend } else { Remove-Item Env:WGPU_BACKEND -ErrorAction SilentlyContinue }
# Below-normal from the start; cmd's PID is recorded so only this process tree can ever be killed.
$args = "/c start /belownormal /b /wait `"`" `"$exe`" --bench --n $N --provinces $Provinces --out $out --tag $Tag"
$p = Start-Process -FilePath cmd.exe -ArgumentList $args -PassThru -NoNewWindow `
    -RedirectStandardError "$out\${App}_${Tag}.stderr.txt" -RedirectStandardOutput "$out\${App}_${Tag}.stdout.txt"
Write-Output "started cmd PID $($p.Id)"
if (-not $p.WaitForExit($TimeoutSec * 1000)) {
    Write-Output "TIMEOUT - killing PID $($p.Id) tree"
    taskkill /F /PID $p.Id /T | Out-Null
}
Write-Output "exit code: $($p.ExitCode)"
$prefix = if ($App -eq "spike_egui") { "egui" } else { "bevy" }
$json = "$out\${prefix}_$Tag.json"
if (Test-Path $json) { Get-Content $json } else { Write-Output "no json; stderr tail:"; Get-Content "$out\${App}_${Tag}.stderr.txt" -Tail 30 }
