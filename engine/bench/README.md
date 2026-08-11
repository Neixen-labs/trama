# Frame benchmark

The phase 4 criterion in `KICKOFF.md`: 100k segments with animated state at 60fps.

```bash
uv run --project ../compiler python ../compiler/benchmarks/grid_container.py --side 224 --out /tmp/bench.trama
npm run build
npm run bench -- --container /tmp/bench.trama --screenshot /tmp/frame.png
```

## What it measures, and what it refuses to

`bench/index.html` drives `createLineRenderer` and `StateRing` directly. MapLibre is not
loaded: a number that includes a basemap's frame budget says nothing about this engine.

Each frame advances time, resolves two ring rows and blends them in the shader, then draws
every tile and calls `gl.finish()`. Two numbers come out of that, and the distinction is the
whole point:

- **frame p50/p95** — how long the frame's work took. This is what a budget applies to.
- **cadence and late frames** — how far apart `requestAnimationFrame` delivered them.

Timing only the interval reports the display's 16.7 ms whether the work took 0.5 ms or 16 ms,
which is exactly the mistake this harness is shaped to avoid. `late` counts frames that
arrived more than 1.5 display periods after the previous one, because a cheap frame can still
be late and a late frame is what a user sees.

The run fails outright if no frame bound the state texture. A benchmark that quietly fell back
to the flat-colour path would measure the cheaper one and report it as the expensive one.
`--screenshot` exists for the same reason: a benchmark that draws nothing is very fast.

## The budget

`FRAME_BUDGET_MS` sets it, 16.7 ms by default. The exit code is only a failure when that
variable is set explicitly — a machine with no GPU renders through SwiftShader and missing the
budget there is a fact about the machine, not about the change under review.

CI runs this without setting it: the numbers are recorded, the run gates only on errors. The
mid-range phone in `KICKOFF.md` needs a real device, and no hosted runner is one.

## Measured

| Machine | Segments | Frame p50 | Frame p95 | Late |
|---|---|---|---|---|
| Intel UHD 630, macOS 15, 1280×720 | 103,040 in 63 tiles | 0.6 ms | 0.8 ms | 0 of 300 |

Filling all 16 ring slots for 99,904 edges — 1.6 M deltas — takes about 470 ms, or 290 ns per
delta. That is the number to watch: the drawing has twenty times the headroom it needs, and
the state path is where a real network would first hurt.
