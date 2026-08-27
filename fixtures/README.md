# Fixtures

Test and demo data. `network.geojson` and `network.trama` are the pair the byte-for-byte
equivalence test compares, so neither changes without a deliberate reason.

## `teruel.trama`

The whole street network of Teruel, Spain — 2,870 nodes, 3,855 edges — shipped **compiled**.
That is the point of it: 256 kB as a container against the 2.1 MB of Overpass JSON it came from,
and the playground opens it without a compile step. The example is the first pillar's claim in
the one place a visitor can weigh it.

A whole small city rather than a slice of a large one, so the questions asked of it are real:
a route across town, how far you get in ten minutes, and which streets are the only way through.
Nothing in it is hydraulic and nothing needed to be invented for it.

It also carries the turns the city forbids: 153 `type=restriction` relations, of which 136 edges
carry a `roads:no_turn` column — including the one whose `via` is a way rather than a node, which
lands as a run of three edges rather than a pair. What is still dropped is two of a kind that
describes priority rather than permission, which are refused rather than guessed at.

**The query asks for `*_link` ways and this matters more than it sounds.** Slip roads are how a
city joins its fast roads, and leaving them out did not merely lose the 68 restrictions that name
one: it left parts of the network reachable only in principle. Including them took the largest
connected component from 95.2% of the streets to 99.0%, and made four of 52 sampled node pairs
routable that previously had no path at all.

The counts move when OSM moves — this file is a snapshot of a live database, and regenerating it
is expected to shift the graph by a street or two.

**© OpenStreetMap contributors**, licensed under the
[Open Database License](https://opendatacommons.org/licenses/odbl/) (ODbL). ODbL governs this
file and any database derived from it; it does not extend to TRAMA's own source, which stays
under the repository's licence. Anything published from this data must keep the attribution.

Regenerate it with `teruel.overpassql` and the compiler, both of which are deterministic:

```bash
curl -s -X POST -d @fixtures/teruel.overpassql https://overpass-api.de/api/interpreter -o teruel.osm.json
cargo run --release -p trama-cli -- compile --importer roads teruel.osm.json fixtures/teruel.trama
```

`core/trama-trace/tests/fixture.rs` checks what the published file has to be true of: one
connected network holding over 90% of the streets, crossable end to end, with critical streets
but not made only of them. The first extract this project shipped was in fragments and rendered
perfectly, which is exactly the failure a screenshot cannot show.

## `cigre-mv.trama`

CIGRE's medium-voltage benchmark, and the playground's only network that can answer a
short-circuit question: `mv_oberrhein` declares no `s_sc_max_mva` for its external grid, which is
what a pandapower file usually looks like, so neither pandapower nor this crate can say anything
about a fault on it. The same network is `core/trama-power/tests/networks/cigre-mv.json`, which is
the fixture the fault calculation is verified against and **is not this file**.

**Its coordinates are a schematic, not a map, and this copy fixes that.** pandapower ships the
benchmark drawn on a grid of whole numbers — longitude 1 to 10, latitude 3 to 16 — which puts a
15-bus distribution network across 1,400 km of the Mediterranean while its own `length_km` column
says the whole thing is 24.95 km. Compiled as-is it came to 324.6 kB in 2,911 tiles, nearly all of
it a section directory for tiles a line merely passes through, and the page drew a single straight
line 12,573 km long. This copy scales the bus coordinates by the ratio the network itself declares
— 0.5019 km per schematic unit — and centres them on Paris, where CIGRE has its seat. The result
is 10.9 kB in 16 tiles.

**Nothing electrical moves, and that is checked rather than asserted.** `r_ohm_per_km`,
`x_ohm_per_km`, `length_km` and `s_sc_max_mva` are columns of their own; no impedance is derived
from the drawing. The fifteen fault currents this file produces are identical to
`cigre-mv.solved.json` — pandapower's own `calc_sc` on the unscaled network — to the last digit an
`f32` holds.

```bash
python3 - <<'PY'
import json, math
net = json.load(open('core/trama-power/tests/networks/cigre-mv.json'))
bus, line = (json.loads(net['_object'][k]['_object']) for k in ('bus', 'line'))
at = bus['columns'].index('geo')
points = {i: json.loads(g)['coordinates'] for i, g in zip(bus['index'], (r[at] for r in bus['data'])) if g}
f, t, L = (line['columns'].index(c) for c in ('from_bus', 'to_bus', 'length_km'))
km_per_unit = sum(r[L] for r in line['data']) / sum(math.dist(points[r[f]], points[r[t]]) for r in line['data'])
cx = sum(p[0] for p in points.values()) / len(points)
cy = sum(p[1] for p in points.values()) / len(points)
LON0, LAT0 = 2.3522, 48.8566
for row in bus['data']:
    x, y = json.loads(row[at])['coordinates']
    row[at] = json.dumps({"coordinates": [
        round(LON0 + (x - cx) * km_per_unit / (111.320 * math.cos(math.radians(LAT0))), 6),
        round(LAT0 + (y - cy) * km_per_unit / 110.574, 6)], "type": "Point"})
net['_object']['bus']['_object'] = json.dumps(bus)
json.dump(net, open('/tmp/cigre-mv.json', 'w'))
PY
cargo run --release -p trama-cli --bin trama -- compile --importer power /tmp/cigre-mv.json fixtures/cigre-mv.trama
```
