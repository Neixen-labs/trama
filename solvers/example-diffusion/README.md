<!-- SPDX-License-Identifier: LicenseRef-BSL-1.1 -->
# example-diffusion

An example solver implementing the server runtime of [`docs/SOLVER_CONTRACT.md`](../../docs/SOLVER_CONTRACT.md).

It exists to hold the contract to a real implementation. A pulse spreads outward from a seed node
over the graph's own topology, and each edge's value is the crest passing it. It models nothing:
the only thing it knows is that edges connect nodes, which is all the core is allowed to know.

```bash
uv run python -m example_diffusion.server        # http://127.0.0.1:8801/solve

curl -N -X POST http://127.0.0.1:8801/solve -H 'Content-Type: application/json' -d '{
  "contract_version": "0.1.0",
  "trama": {"url": "http://127.0.0.1:8790/fixtures/demo-grid.trama"},
  "params": {"channel": "flow"},
  "t0_seconds": 0,
  "t1_seconds": 600
}'
```

The response is Server-Sent Events: one `ready`, any number of `delta` events whose Base64 payload
decodes to concatenated 18-byte records, and one `complete`. A failure sends exactly one `error`.

## Deviation from the contract

Contract §6 requires `trama.url` to be an absolute HTTPS URL. This example also accepts
`http://localhost` and `http://127.0.0.1` so it can run against the local demo server. A deployed
solver must not, and §6 requires an implementation to document its access policy — this is that
documentation.
