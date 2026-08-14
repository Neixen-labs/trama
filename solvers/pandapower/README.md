# trama-pandapower

A TRAMA solver that runs [pandapower](https://pandapower.org) load flows over a `.trama`
container, and — the part worth reading — **a reader for the TRAMA format written in Python from
`docs/SPEC.md` alone**, with nothing imported from `trama-format`.

That reader is why this package exists in the shape it does. Every other solver in this
repository is a Rust crate that links the reference implementation, so until now nothing tested
the claim the whole project rests on: that the format is open enough for someone else to
implement. Writing `trama_pandapower/container.py` from the specification found three things the
specification had not said — the width of a string's length prefix, the bit order of a presence
bitmap, and the layout of column values per type. Those became a SPEC amendment, which is what
should happen.

## Running it

```bash
pip install -e .
trama-solver-pandapower --port 8080
```

Then point a client at it, per `docs/SOLVER_CONTRACT.md` section 6:

```bash
curl -N http://127.0.0.1:8080/solve \
  -H 'content-type: application/json' \
  -d '{"contract_version":"0.3.0",
       "trama":{"url":"https://example.org/grid.trama"},
       "params":{},"t0_seconds":0,"t1_seconds":0}'
```

## What it computes

| Channel | Entity | Unit | Meaning |
|---|---|---|---|
| `voltage` | node | p.u. | Bus voltage magnitude against the bus's own nominal |
| `loading` | edge | % | Line and transformer loading against their rating |

Neither declares a range, because a bus under 0.9 p.u. and a line over 100% are the answers the
study exists to find — see `docs/DECISIONS.md`.

A load flow is one instant, so by default this writes one, at `t0`. Pass `load_scaling` — a list
of multipliers applied to every load — to get a series instead: one real load flow per
multiplier, spread evenly across `[t0, t1]`. The daily curve is the caller's to supply; a solver
that invented one would be reporting a modelling assumption as a measurement.

## Access policy

`docs/SOLVER_CONTRACT.md` section 6 requires a server to document how it decides what it will
fetch. This one:

- `trama.url` must be an absolute **HTTPS** URL, fetched without credentials, capped at 64 MB.
- If `trama.sha256` is given, it is verified before anything is parsed.
- `--allow-http` additionally permits `http://` on **loopback addresses only**, for a developer
  serving a container from their own machine. Any other host is refused with the same
  `invalid_request` as a plain `http://` URL, so the flag cannot become a way into an intranet.
- There is no allowlist beyond that. Deploy this behind one.

## Tests

```bash
pip install -e .[dev]
python -m pytest
```

The fixture is `fixtures/oberrhein.trama`, compiled by `trama compile --importer power` from
pandapower's own `mv_oberrhein`. The tests compare against `pandapower.networks.mv_oberrhein()`
loaded directly rather than against recorded numbers, so what they check is that a network
survives the round trip through the container: same bus voltages, same line loadings, to the
resolution an `f32` delta has.
