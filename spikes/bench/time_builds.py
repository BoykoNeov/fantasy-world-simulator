"""Time incremental rebuilds (touch main.rs, rebuild) for both spike apps, at below-normal priority."""
import os, subprocess, time

SPIKES = r"M:\claud_projects\fantasymap\spikes"
BELOW_NORMAL = 0x00004000  # Windows BELOW_NORMAL_PRIORITY_CLASS; children inherit it

def build(app, release):
    cmd = ["cargo", "build", "--manifest-path", os.path.join(SPIKES, "Cargo.toml"), "-p", app] + (["--release"] if release else [])
    t0 = time.perf_counter()
    r = subprocess.run(cmd, capture_output=True, text=True, creationflags=BELOW_NORMAL)
    dt = time.perf_counter() - t0
    if r.returncode != 0:
        raise SystemExit(f"build failed for {app}:\n{r.stderr[-2000:]}")
    return dt

for app in ["spike_egui", "spike_bevy"]:
    for release in [False, True]:
        build(app, release)  # make sure everything else is up to date
        times = []
        for _ in range(3):
            os.utime(os.path.join(SPIKES, app, "src", "main.rs"))
            times.append(build(app, release))
        times.sort()
        print(f"{app:11} {'release' if release else 'debug  '} incremental: median {times[1]:5.1f} s  (runs {', '.join(f'{t:.1f}' for t in times)})")
